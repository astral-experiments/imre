//! Lower reflection on known native declarations. Annotation dictionaries are
//! new allocations on every read, even when their metadata is constant.
use super::*;
use ruff_python_codegen::{Generator, Mode};

/// A declaration consisting of docstrings, pass and ellipses always returns None.
/// Its Python return annotation remains visible to reflection.
pub(super) fn inert_body(function: &ast::StmtFunctionDef) -> bool {
    function.body.iter().all(|stmt| {
        matches!(stmt, ast::Stmt::Pass(_))
            || matches!(stmt, ast::Stmt::Expr(e) if matches!(&*e.value,
            ast::Expr::EllipsisLiteral(_) | ast::Expr::StringLiteral(_)))
    })
}

/// Builtin classes form a closed nominal domain, independent of the values they construct.
pub(super) fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "bytes"
            | "bytearray"
            | "complex"
            | "dict"
            | "float"
            | "frozenset"
            | "int"
            | "list"
            | "memoryview"
            | "object"
            | "range"
            | "set"
            | "slice"
            | "str"
            | "tuple"
            | "type"
    )
}

pub(super) fn builtin_type(name: &str) -> Option<Expr> {
    (is_builtin_type(name) || super::warnings::is_warning_class(name)).then(|| Expr {
        ty: Type::BuiltinClass,
        kind: ExprKind::Builtin(Builtin::ClassNameView, vec![unpacking::string(name)]),
    })
}

pub(super) fn type_name(value: Expr) -> Expr {
    Expr {
        ty: Type::Str,
        kind: ExprKind::Builtin(Builtin::ClassNameView, vec![value]),
    }
}

fn function_entries(function: &ast::StmtFunctionDef) -> Vec<(&str, &ast::Expr)> {
    // CPython emits ordinary arguments before the separate positional-only group.
    let p = &function.parameters;
    p.args
        .iter()
        .chain(&p.posonlyargs)
        .map(|p| &p.parameter)
        .chain(p.vararg.as_deref())
        .chain(p.kwonlyargs.iter().map(|p| &p.parameter))
        .chain(p.kwarg.as_deref())
        .filter_map(|p| p.annotation.as_deref().map(|a| (p.name.as_str(), a)))
        .chain(function.returns.as_deref().map(|a| ("return", a)))
        .collect()
}

/// Undeclared names may still be provided by the Python 3.14 builtin namespace.
fn potential_forward_name(name: &str) -> bool {
    !ruff_python_stdlib::builtins::is_python_builtin(name, 14, false)
        && !ruff_python_stdlib::builtins::python_magic_globals(14).any(|n| n == name)
}

fn field_entries(body: &[ast::Stmt]) -> Vec<(&str, &ast::Expr)> {
    body.iter()
        .filter_map(|s| {
            if let ast::Stmt::AnnAssign(a) = s
                && a.simple
                && let ast::Expr::Name(n) = &*a.target
            {
                Some((n.id.as_str(), &*a.annotation))
            } else {
                None
            }
        })
        .collect()
}

fn spelling(annotation: &ast::Expr) -> String {
    fn items(items: &[ast::Expr]) -> String {
        items.iter().map(spelling).collect::<Vec<_>>().join(", ")
    }
    match annotation {
        ast::Expr::Subscript(s) => {
            let slice = match &*s.slice {
                ast::Expr::Tuple(t) if !t.elts.is_empty() => {
                    let mut args = items(&t.elts);
                    if t.elts.len() == 1 {
                        args.push(',');
                    }
                    args
                }
                expr => spelling(expr),
            };
            let mut base = spelling(&s.value);
            if matches!(&*s.value, ast::Expr::BinOp(_)) {
                base = format!("({base})");
            }
            return format!("{base}[{slice}]");
        }
        ast::Expr::List(l) => return format!("[{}]", items(&l.elts)),
        ast::Expr::Tuple(t) => {
            return format!(
                "({}{})",
                items(&t.elts),
                if t.elts.len() == 1 { "," } else { "" }
            );
        }
        ast::Expr::BinOp(b) if b.op == ast::Operator::BitOr && symbolic(annotation) => {
            let mut right = spelling(&b.right);
            if matches!(&*b.right, ast::Expr::BinOp(_)) {
                right = format!("({right})");
            }
            return format!("{} | {right}", spelling(&b.left));
        }
        _ => {}
    }
    Generator::new(&Default::default(), Default::default())
        .with_mode(Mode::AstUnparse)
        .expr(annotation)
}

// The STRING protocol evaluates expressions in a symbolic namespace. Only
// forms whose spelling survives that evaluation can use the AST unparser.
fn symbolic(value: &ast::Expr) -> bool {
    match value {
        ast::Expr::Name(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::EllipsisLiteral(_) => true,
        ast::Expr::Attribute(a) => symbolic(&a.value),
        ast::Expr::Subscript(s) => symbolic(&s.value) && symbolic(&s.slice),
        ast::Expr::Tuple(t) => t.elts.iter().all(symbolic),
        ast::Expr::List(l) => l.elts.iter().all(symbolic),
        ast::Expr::BinOp(b) if b.op == ast::Operator::BitOr => {
            symbolic(&b.left)
                && symbolic(&b.right)
                && (matches!(
                    &*b.left,
                    ast::Expr::Name(_)
                        | ast::Expr::Subscript(_)
                        | ast::Expr::Attribute(_)
                        | ast::Expr::BinOp(_)
                ) || matches!(
                    &*b.right,
                    ast::Expr::Name(_) | ast::Expr::Subscript(_) | ast::Expr::Attribute(_)
                ))
        }
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnnotationScope {
    Namespace,
    ModuleFunction,
    LocalFunction,
    UnavailableLocal,
}

impl Checker<'_> {
    pub(super) fn annotation_type_repr(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [arg] = call.arguments.args.as_ref() else {
            return Err(error(self.source, call, "type_repr expects one value"));
        };
        // Function names include the defining module; the spelling at the import site is irrelevant.
        if let Some(Binding::Definition(id)) = self.resolve(arg) {
            let module = &self.graph.modules[id.module];
            let name = match &module.parsed.syntax().body[id.statement] {
                ast::Stmt::FunctionDef(f) => Some(&f.name),
                ast::Stmt::ClassDef(c) => Some(&c.name),
                _ => None,
            };
            if let Some(name) = name {
                let prefix = if id.module == self.graph.entry {
                    "__main__"
                } else {
                    &module.name
                };
                return Ok(unpacking::string(format!("{prefix}.{name}")));
            }
        }
        let value = self.expr(arg)?;
        if value.ty == Type::BuiltinClass {
            Ok(type_name(value))
        } else if dataclass_values::can_format(value.ty, self.types, self.classes) {
            Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
            })
        } else {
            Err(error(
                self.source,
                arg,
                "type_repr needs a native representation for this object",
            ))
        }
    }

    pub(super) fn get_annotations(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let mut obj = match call.arguments.args.as_ref() {
            [] => None,
            [obj] => Some(obj),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "get_annotations expects one object",
                ));
            }
        };
        let mut format = 1;
        let mut eval_str = false;
        for kw in &call.arguments.keywords {
            match kw.arg.as_ref().map(|a| a.as_str()) {
                Some("obj") if obj.is_none() => obj = Some(&kw.value),
                Some("format") => {
                    format = match self.resolve(&kw.value) {
                        Some(Binding::Intrinsic(Intrinsic::AnnotationFormat(f))) => f,
                        _ => match &kw.value {
                            ast::Expr::NumberLiteral(n) if let ast::Number::Int(i) = &n.value => {
                                i.to_string().parse().map_err(|_| {
                                    error(self.source, n, "annotation format is out of range")
                                })?
                            }
                            _ => {
                                return Err(error(
                                    self.source,
                                    kw,
                                    "get_annotations requires a statically known format",
                                ));
                            }
                        },
                    }
                }
                Some("eval_str") if let ast::Expr::BooleanLiteral(b) = &kw.value => {
                    eval_str = b.value
                }
                Some("globals" | "locals") if matches!(kw.value, ast::Expr::NoneLiteral(_)) => {}
                _ => {
                    return Err(error(
                        self.source,
                        kw,
                        "unsupported get_annotations keyword or dynamic evaluation namespace",
                    ));
                }
            }
        }
        let obj =
            obj.ok_or_else(|| error(self.source, call, "get_annotations requires an object"))?;
        // Source references below are ordinary loads; do not elide an effectful
        // object expression just because the selected format will raise.
        let binding = self.resolve(obj);
        let wrapped = if !matches!(binding, Some(Binding::Definition(_) | Binding::Module(_)))
            && (self.local_function_binding(obj).is_none()
                || self.local_cached_value(obj).is_some())
        {
            let target = self.expr(obj)?;
            let layout = self.types.cached_function(target.ty).ok_or_else(|| error(self.source, obj,
                "get_annotations requires a known native function, class, module, or cache wrapper"))?;
            let origin = layout.annotations.ok_or_else(|| {
                error(
                    self.source,
                    obj,
                    "the cached callback's annotation declaration is not statically known",
                )
            })?;
            Some((self.hidden(target.ty), target, origin))
        } else {
            None
        };
        let wrapped_local =
            if let Some((_, _, crate::types::AnnotationOrigin::Local(id))) = &wrapped {
                self.helpers.local_annotations.borrow().get(id).cloned()
            } else {
                None
            };
        let local_owner = self
            .local_function_binding(obj)
            .and_then(|b| b.value_slot)
            .map(|slot| self.local(slot));
        let named_function = match self.resolve(obj) {
            Some(Binding::Definition(d)) if self.functions.contains_key(&d) => Some(d),
            _ => None,
        };
        let scope = if let Some((_, _, crate::types::AnnotationOrigin::Local(id))) = &wrapped {
            if wrapped_local.as_ref().is_some_and(|(_, f)| {
                self.local_functions
                    .get(f.name.as_str())
                    .and_then(|b| b.target(self.source, obj).ok())
                    .is_some_and(|f| f.signature.id == *id)
            }) {
                AnnotationScope::LocalFunction
            } else {
                AnnotationScope::UnavailableLocal
            }
        } else if local_owner.is_some() {
            AnnotationScope::LocalFunction
        } else if named_function.is_some() || wrapped.is_some() {
            AnnotationScope::ModuleFunction
        } else {
            AnnotationScope::Namespace
        };
        let owner = if let Some((slot, _, _)) = &wrapped {
            Some(self.local(*slot))
        } else if let Some(d) = named_function
            && format == 3
            && let ast::Stmt::FunctionDef(f) = &self.graph.modules[d.module].parsed.syntax().body[d.statement]
            && function_entries(f).into_iter().any(|(_, a)| matches!(a, ast::Expr::Name(n)
                if potential_forward_name(n.id.as_str()) && !self.graph.modules[d.module].bindings.contains_key(n.id.as_str()))) {
            Some(self.expr(obj)?)
        } else {
            local_owner
        };
        let declaration = match &wrapped {
            Some((_, _, crate::types::AnnotationOrigin::Module(d))) => {
                Some(Binding::Definition(*d))
            }
            _ => binding,
        };
        let (id, entries) = match declaration {
            Some(Binding::Definition(d)) => {
                let m = &self.graph.modules[d.module];
                let entries = match &m.parsed.syntax().body[d.statement] {
                    ast::Stmt::FunctionDef(f) => function_entries(f),
                    ast::Stmt::ClassDef(c) => field_entries(&c.body),
                    _ => {
                        return Err(error(
                            self.source,
                            obj,
                            "object has no static annotation declaration",
                        ));
                    }
                };
                (d.module, entries)
            }
            Some(Binding::Module(id)) => (
                id,
                field_entries(&self.graph.modules[id].parsed.syntax().body),
            ),
            _ if let Some((id, function)) = &wrapped_local => (*id, function_entries(function)),
            _ => {
                let binding = self.local_function_binding(obj).ok_or_else(|| {
                    error(
                        self.source,
                        obj,
                        "get_annotations requires a known native function, class, or module",
                    )
                })?;
                let f = binding
                    .generic
                    .as_deref()
                    .or_else(|| {
                        binding
                            .target(self.source, obj)
                            .ok()
                            .map(|f| &*f.definition)
                    })
                    .ok_or_else(|| {
                        error(
                            self.source,
                            obj,
                            "local annotation declaration is unavailable",
                        )
                    })?;
                (self.module, function_entries(f))
            }
        };
        let result =
            self.declared_annotations(obj, id, entries, (format, eval_str), scope, owner)?;
        Ok(if let Some((slot, target, _)) = wrapped {
            Expr {
                ty: result.ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, target)],
                    value: Box::new(result),
                    clear: vec![slot],
                },
            }
        } else {
            result
        })
    }

    fn declared_annotations(
        &self,
        obj: &ast::Expr,
        id: ModuleId,
        entries: Vec<(&str, &ast::Expr)>,
        options: (i64, bool),
        scope: AnnotationScope,
        owner: Option<Expr>,
    ) -> Result<Expr, String> {
        let (format, eval_str) = options;
        let dict = |types: &Types, value: Type, entries| Expr {
            ty: types.dict(Type::Str, value),
            kind: ExprKind::Dict {
                key: Type::Str,
                value,
                entries,
            },
        };
        let error_ty = self.types.dict(Type::Str, Type::Str);

        let raise = |kind: &str, message: String| Expr {
            ty: error_ty,
            kind: ExprKind::Builtin(
                Builtin::RaiseException,
                vec![Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind(kind).unwrap(),
                        message: Some(Box::new(unpacking::string(message))),
                    },
                }],
            ),
        };
        if eval_str && format != 1 {
            return Ok(raise(
                "ValueError",
                "eval_str=True is only supported with format=Format.VALUE".into(),
            ));
        }
        if format == 2 {
            return Ok(raise(
                "ValueError",
                "The VALUE_WITH_FAKE_GLOBALS format is for internal use only".into(),
            ));
        }
        if !matches!(format, 1 | 3 | 4) {
            return Ok(raise("ValueError", format!("Unsupported format {format}")));
        }
        let module = &self.graph.modules[id];
        let postponed = module
            .bindings
            .values()
            .any(|b| *b == Binding::Intrinsic(Intrinsic::FutureAnnotations));
        let mut lowered = Vec::new();
        let mut members = Vec::new();
        for (name, a) in entries {
            let value = if postponed && !eval_str {
                unpacking::string(spelling(a))
            } else if format == 4 {
                match a {
                    ast::Expr::StringLiteral(s) => unpacking::string(s.value.to_str()),
                    _ if symbolic(a) => unpacking::string(spelling(a)),
                    _ => {
                        return Err(error(
                            &module.source,
                            a,
                            "STRING format needs to evaluate this annotation expression",
                        ));
                    }
                }
            } else {
                match a {
                    ast::Expr::Name(n)
                        if !module.bindings.contains_key(n.id.as_str())
                            && let Some(ty) = builtin_type(n.id.as_str()) =>
                    {
                        ty
                    }
                    ast::Expr::StringLiteral(s) if !eval_str || postponed => {
                        unpacking::string(s.value.to_str())
                    }
                    ast::Expr::StringLiteral(s)
                        if !module.bindings.contains_key(s.value.to_str())
                            && let Some(ty) = builtin_type(s.value.to_str()) =>
                    {
                        ty
                    }
                    ast::Expr::NoneLiteral(_) => Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    },
                    ast::Expr::Name(n)
                        if !module.bindings.contains_key(n.id.as_str())
                            && potential_forward_name(n.id.as_str())
                            && scope != AnnotationScope::Namespace =>
                    {
                        if scope == AnnotationScope::UnavailableLocal {
                            return Err(error(
                                &module.source,
                                a,
                                "deferred annotation requires the original enclosing scope",
                            ));
                        }
                        if scope == AnnotationScope::LocalFunction
                            && let Some(&slot) = self.names.get(n.id.as_str())
                            && self.assigned.contains(&slot)
                        {
                            self.local(slot)
                        } else if format == 1 {
                            return Ok(raise(
                                "NameError",
                                format!("name '{}' is not defined", n.id),
                            ));
                        } else {
                            let owner = owner
                                .as_ref()
                                .ok_or_else(|| {
                                    error(
                                        self.source,
                                        obj,
                                        "unresolved annotation needs its native function owner",
                                    )
                                })?
                                .clone();
                            let module_type = self.types.optional(Type::Str);
                            Expr {
                                ty: self.types.forward_ref(owner.ty),
                                kind: ExprKind::Builtin(
                                    Builtin::NewForwardRef,
                                    vec![
                                        unpacking::string(n.id.as_str()),
                                        Expr {
                                            ty: module_type,
                                            kind: ExprKind::Absent(Box::new(none_value())),
                                        },
                                        owner,
                                        bool_value(false),
                                        bool_value(false),
                                    ],
                                ),
                            }
                        }
                    }
                    _ => {
                        return Err(error(
                            &module.source,
                            a,
                            "annotation needs a native evaluator for this expression",
                        ));
                    }
                }
            };
            if !members.contains(&value.ty) {
                members.push(value.ty);
            }
            lowered.push((unpacking::string(name), value));
        }
        let element = match members.as_slice() {
            [] => Type::Str,
            [member] if !matches!(member, Type::ForwardRef(_)) => *member,
            _ if members.iter().all(|t| {
                matches!(
                    t,
                    Type::Str | Type::None | Type::BuiltinClass | Type::ForwardRef(_)
                )
            }) =>
            {
                members.sort_by_key(|t| match t {
                    Type::None => 0,
                    Type::Str => 3,
                    Type::BuiltinClass => 11,
                    _ => 12,
                });
                let Type::Tuple(id) = self.types.tuple(members) else {
                    unreachable!()
                };
                let element = Type::AnnotationValue(id);
                lowered = lowered
                    .into_iter()
                    .map(|(key, value)| (key, self.box_annotation(value, element)))
                    .collect();
                element
            }
            _ => {
                return Err(error(
                    self.source,
                    obj,
                    "annotation dictionary values need a common native representation",
                ));
            }
        };
        Ok(dict(self.types, element, lowered))
    }

    pub(super) fn box_annotation(&self, value: Expr, ty: Type) -> Expr {
        if matches!(value.ty, Type::AnnotationValue(_)) {
            return value;
        }
        let Type::Tuple(layout) = self.types.tuple(vec![value.ty]) else {
            unreachable!()
        };
        Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::BoxAnnotation(layout), vec![value]),
        }
    }

    pub(super) fn annotation_attribute(
        &self,
        value: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let Some(layout) = self
            .types
            .children(value.ty)
            .into_iter()
            .find_map(|ty| match ty {
                Type::ForwardRef(id) => Some(id),
                _ => None,
            })
        else {
            return Err(error(
                self.source,
                attr,
                "annotation has no known ForwardRef member",
            ));
        };
        let reference = Expr {
            ty: Type::ForwardRef(layout),
            kind: ExprKind::Builtin(
                Builtin::AnnotationRef,
                vec![value, unpacking::string(attr.attr.as_str())],
            ),
        };
        self.forward_ref_attribute(layout, reference, attr)
    }
}

fn none_value() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

fn bool_value(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

impl Checker<'_> {
    /// Bind the real constructor signature before building the immutable owned record.
    pub(super) fn forward_ref(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let module = self.types.optional(Type::Str);
        let params = [
            ("arg", Type::Str, None, ParameterKind::PositionalOrKeyword),
            (
                "module",
                module,
                Some(Expr {
                    ty: module,
                    kind: ExprKind::Absent(Box::new(none_value())),
                }),
                ParameterKind::KeywordOnly,
            ),
            (
                "owner",
                Type::None,
                Some(none_value()),
                ParameterKind::KeywordOnly,
            ),
            (
                "is_argument",
                Type::Bool,
                Some(bool_value(true)),
                ParameterKind::KeywordOnly,
            ),
            (
                "is_class",
                Type::Bool,
                Some(bool_value(false)),
                ParameterKind::KeywordOnly,
            ),
        ]
        .map(|(name, ty, default, kind)| Parameter {
            name: name.into(),
            ty,
            default,
            kind,
            sign_only: false,
        });
        let args =
            self.arguments_with(call, &params, None, |checker, index, node| match index {
                0 | 2 => checker.expr(node),
                _ => checker.expr_expected(node, params[index].ty),
            })?;
        let ty = self.types.forward_ref(args.values[args.order[2]].ty);
        let argument = &args.values[args.order[0]];
        if argument.ty != Type::Str {
            let pos = args.order[0];
            let slot = self.hidden(argument.ty);
            let mut body = vec![];
            for (i, value) in args.values.into_iter().enumerate() {
                if i == pos {
                    body.push(Statement::Assign(slot, value));
                } else {
                    body.push(Statement::Expr(value));
                }
            }
            let val = self.local(slot);
            if !dataclass_values::can_format(val.ty, self.types, self.classes) {
                return Err(error(
                    self.source,
                    call,
                    "ForwardRef requires a string or a representable argument for its TypeError",
                ));
            }
            let message = unpacking::concat(
                unpacking::string("Forward reference must be a string -- got "),
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Repr, vec![val]),
                },
            );
            let exc = Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("TypeError").unwrap(),
                    message: Some(Box::new(message)),
                },
            };
            return Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body,
                    clear: vec![slot],
                    value: Box::new(Expr {
                        ty,
                        kind: ExprKind::Builtin(Builtin::RaiseException, vec![exc]),
                    }),
                },
            });
        }
        Ok(self.ordered_builtin(Builtin::NewForwardRef, ty, args))
    }

    /// Expose only ForwardRef's public compatibility attributes, never tuple operations.
    pub(super) fn forward_ref_attribute(
        &self,
        layout: usize,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let index = match attr.attr.as_str() {
            "__forward_arg__" => 0,
            "__forward_module__" => 1,
            "__owner__" => 2,
            "__forward_is_argument__" => 3,
            "__forward_is_class__" => 4,
            _ => return Err(error(self.source, attr, "unsupported ForwardRef attribute")),
        };
        let record = Type::Tuple(layout);
        let ty = self.types.fields(record).unwrap()[index];
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::ForwardRefField(index), vec![receiver]),
        })
    }
}
