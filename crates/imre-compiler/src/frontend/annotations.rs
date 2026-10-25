//! Lower reflection on known native declarations. Annotation dictionaries are
//! new allocations on every read, even when their metadata is constant.
use super::*;
use ruff_python_codegen::{Generator, Mode};

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
    is_builtin_type(name).then(|| class_value(name))
}

/// Class objects use the pooled name for identity, including its module for
/// types outside builtins. This does not require constructing an instance.
pub(super) fn class_value(name: &str) -> Expr {
    Expr {
        ty: Type::BuiltinClass,
        kind: ExprKind::Builtin(Builtin::ClassNameView, vec![unpacking::string(name)]),
    }
}

/// Both annotations describe the same item interface; their class values differ.
pub(super) fn sequence_annotation(binding: Option<Binding>) -> bool {
    matches!(
        binding,
        Some(Binding::Intrinsic(
            Intrinsic::Sequence | Intrinsic::NominalClass("collections.abc.Sequence")
        ))
    )
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

impl Checker<'_> {
    /// Handle class objects from the sealed builtin domain. Fixed-layout
    /// source classes use their own class dispatch.
    pub(super) fn nominal_issubclass(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        let [child, parent] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        if self.class_reference(child).is_some() || self.class_reference(parent).is_some() {
            return Ok(None);
        }
        let child = self.expr(child)?;
        let parent = self.expr(parent)?;
        if child.ty != Type::BuiltinClass || parent.ty != Type::BuiltinClass {
            return Ok(None);
        }
        self.reject_builtin_keywords(call)?;
        Ok(Some(Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::ClassIsSubclass, vec![child, parent]),
        }))
    }

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
        let dict = |types: &Types, value: Type, entries| Expr {
            ty: types.dict(Type::Str, value),
            kind: ExprKind::Dict {
                key: Type::Str,
                value,
                entries,
            },
        };
        let raise = |message: String| Expr {
            ty: self.types.dict(Type::Str, Type::Str),
            kind: ExprKind::Builtin(
                Builtin::RaiseException,
                vec![Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("ValueError").unwrap(),
                        message: Some(Box::new(unpacking::string(message))),
                    },
                }],
            ),
        };
        // Source references below are ordinary loads; do not elide an effectful
        // object expression just because the selected format will raise.
        let (id, entries) = match self.resolve(obj) {
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
        if eval_str && format != 1 {
            return Ok(raise(
                "eval_str=True is only supported with format=Format.VALUE".into(),
            ));
        }
        if format == 2 {
            return Ok(raise(
                "The VALUE_WITH_FAKE_GLOBALS format is for internal use only".into(),
            ));
        }
        if !matches!(format, 1 | 3 | 4) {
            return Ok(raise(format!("Unsupported format {format}")));
        }
        let module = &self.graph.modules[id];
        let postponed = module
            .bindings
            .values()
            .any(|b| *b == Binding::Intrinsic(Intrinsic::FutureAnnotations));
        let mut lowered = Vec::new();
        let mut element = None;
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
                    _ => {
                        return Err(error(
                            &module.source,
                            a,
                            "annotation needs a native evaluator for this expression",
                        ));
                    }
                }
            };
            if element
                .replace(value.ty)
                .is_some_and(|previous| previous != value.ty)
            {
                return Err(error(
                    self.source,
                    obj,
                    "annotation dictionary values need a common native representation",
                ));
            }
            lowered.push((unpacking::string(name), value));
        }
        Ok(dict(self.types, element.unwrap_or(Type::Str), lowered))
    }
}
