//! Native inspect predicates and static attribute reads. Passive attribute
//! lookup does not run descriptors or user attribute hooks.
use super::*;

pub(super) fn marks_function(
    graph: &SourceGraph,
    module: ModuleId,
    function: &ast::StmtFunctionDef,
) -> bool {
    matches!(function.decorator_list.as_slice(), [decorator] if graph.resolve(module, &decorator.expression)
        == Some(Binding::Intrinsic(Intrinsic::InspectCoroutine { mark: true })))
}

pub(super) fn is_sentinel(graph: &SourceGraph, module: ModuleId, value: &ast::Expr) -> bool {
    graph.resolve(module, value) == Some(Binding::Intrinsic(Intrinsic::InspectSentinel))
}

pub(super) fn sentinel() -> Expr {
    Expr {
        ty: Type::PlainObject,
        kind: ExprKind::Builtin(Builtin::InspectSentinel, vec![]),
    }
}

pub(super) fn descriptor(method: ir::StrDescriptor) -> Expr {
    Expr {
        ty: Type::StrDescriptor(method),
        kind: ExprKind::Builtin(Builtin::StrDescriptor(method), vec![]),
    }
}

fn evaluated(value: Expr, result: Expr) -> Expr {
    Expr {
        ty: result.ty,
        kind: ExprKind::Scope {
            body: vec![Statement::Expr(value)],
            value: Box::new(result),
            clear: vec![],
        },
    }
}

/// Plain object instances and builtin method descriptors do not override equality.
pub(super) fn equality(left: &Expr, right: &Expr, op: ast::CmpOp) -> Option<Expr> {
    let identity = |ty| {
        matches!(
            ty,
            Type::PlainObject | Type::StrDescriptor(_) | Type::Placeholder(_)
        )
    };
    if !matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
        || !(identity(left.ty) && (identity(right.ty) || right.ty == Type::None)
            || identity(right.ty) && left.ty == Type::None)
    {
        return None;
    }
    if left.ty == right.ty {
        return Some(Checker::identity_result(
            vec![left.clone(), right.clone()],
            op == ast::CmpOp::NotEq,
        ));
    }
    Some(evaluated(
        left.clone(),
        evaluated(
            right.clone(),
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(op == ast::CmpOp::NotEq),
            },
        ),
    ))
}

// Builtin types have immutable dictionaries. Keep the existing public names
// apart from absent attributes so an unsupported descriptor never becomes a default.
fn builtin_plain_name(ty: Type, name: &str) -> bool {
    let names = match ty {
        Type::Int | Type::Bool => {
            "as_integer_ratio bit_count bit_length conjugate denominator from_bytes imag is_integer numerator real to_bytes"
        }
        Type::Str | Type::StrLiteral(_) => concat!(
            "capitalize casefold center count encode endswith expandtabs find format format_map index ",
            "isalnum isalpha isascii isdecimal isdigit isidentifier islower isnumeric isprintable isspace ",
            "istitle isupper join ljust lower lstrip maketrans partition removeprefix removesuffix replace ",
            "rfind rindex rjust rpartition rsplit rstrip split splitlines startswith strip swapcase title ",
            "translate upper zfill",
        ),
        _ => unreachable!(),
    };
    names
        .split_ascii_whitespace()
        .any(|existing| existing == name)
}

fn class_has_name(base: &crate::classes::Class, name: &str) -> bool {
    base.static_bindings.contains_key(name)
        || base.node.body.iter().any(|statement| match statement {
            ast::Stmt::Assign(assign) => assign
                .targets
                .iter()
                .any(|target| target.as_name_expr().is_some_and(|n| n.id == name)),
            ast::Stmt::AnnAssign(assign) => {
                assign.target.as_name_expr().is_some_and(|n| n.id == name)
            }
            ast::Stmt::FunctionDef(function) => function.name.as_str() == name,
            ast::Stmt::ClassDef(class) => class.name.as_str() == name,
            _ => false,
        })
}

fn missing(types: &Types, name: &str, default: Option<Expr>) -> Expr {
    let error = Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("AttributeError").unwrap(),
            message: Some(Box::new(unpacking::string(name))),
        },
    };
    let raise = Expr {
        ty: Type::None,
        kind: ExprKind::Builtin(Builtin::RaiseException, vec![error]),
    };
    let (body, value) = match default {
        None => return raise,
        Some(value) if types.payload(value.ty).unwrap_or(value.ty) == Type::PlainObject => {
            // The optional representation preserves an object's identity word;
            // its None tag cannot equal the sentinel's pointer.
            let is_sentinel = Checker::identity_result(vec![value.clone(), sentinel()], false);
            (
                vec![Statement::If(
                    is_sentinel,
                    vec![Statement::Expr(raise)],
                    vec![],
                )],
                value,
            )
        }
        Some(value) => return value,
    };
    Expr {
        ty: value.ty,
        kind: ExprKind::Scope {
            body,
            value: Box::new(value),
            clear: vec![],
        },
    }
}

impl Checker<'_> {
    pub(super) fn inspect_coroutine(
        &mut self,
        call: &ast::ExprCall,
        mark: bool,
    ) -> Result<Expr, String> {
        let parameter = if mark { "func" } else { "obj" };
        let node = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some(parameter) => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "coroutine inspection requires one argument",
                ));
            }
        };
        let value = self.expr(node)?;
        if mark {
            // Only a module definition has one stable function for the program's
            // lifetime. Each invocation of an enclosing function can create a
            // new local function with the same adapter and a different marker.
            if let ExprKind::Builtin(Builtin::StaticFunction(id), _) = &value.kind
                && matches!(self.resolve(node), Some(Binding::Definition(_)))
            {
                return Ok(Expr {
                    ty: value.ty,
                    kind: ExprKind::Builtin(Builtin::MarkCoroutine(*id), vec![value]),
                });
            }
            return Err(error(
                self.source,
                node,
                "marking this coroutine function requires a dynamic function dictionary",
            ));
        }
        if matches!(value.ty, Type::Callable(_)) {
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsCoroutineFunction, vec![value]),
            });
        }
        if functools::noncallable_type(value.ty).is_some()
            || matches!(
                value.ty,
                Type::PlainObject
                    | Type::StrDescriptor(_)
                    | Type::StrLiteral(_)
                    | Type::BuiltinClass
                    | Type::Coroutine(_)
            )
        {
            return Ok(evaluated(
                value,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                },
            ));
        }
        Err(error(
            self.source,
            node,
            "coroutine inspection requires runtime attributes for this object",
        ))
    }

    pub(super) fn descriptor_attribute(
        &self,
        receiver: Expr,
        method: ir::StrDescriptor,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let text = match attr.attr.as_str() {
            "__objclass__" => {
                return Ok(evaluated(
                    receiver,
                    annotations::builtin_type("str").unwrap(),
                ));
            }
            "__name__" => method.name().to_owned(),
            "__qualname__" => format!("str.{}", method.name()),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "unsupported method descriptor attribute",
                ));
            }
        };
        Ok(evaluated(receiver, unpacking::string(text)))
    }

    /// The supported descriptor values come from builtin type dictionaries.
    /// Other exact scalar values have no descriptor protocol.
    pub(super) fn is_method_descriptor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let arg = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some("object") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "inspect.ismethoddescriptor requires one object",
                ));
            }
        };
        let value = self.expr(arg)?;
        let result = match value.ty {
            Type::StrDescriptor(_) => true,
            Type::PlainObject
            | Type::BuiltinClass
            | Type::Int
            | Type::Bool
            | Type::Float
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::None => false,
            _ => {
                return Err(error(
                    self.source,
                    arg,
                    "the native descriptor protocol for this type is unknown",
                ));
            }
        };
        Ok(evaluated(
            value,
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(result),
            },
        ))
    }

    pub(super) fn invoke_descriptor(
        &mut self,
        call: &ast::ExprCall,
        callable: Expr,
        method: ir::StrDescriptor,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let Some((instance, tail)) = call.arguments.args.split_first() else {
            self.graph
                .checked_missing_argument(self.module, call.range());
            let message = format!("unbound method str.{}() needs an argument", method.name());
            return self.descriptor_call_error(call, method, vec![callable], &[], message);
        };
        let receiver = self.expr(instance)?;
        if !matches!(receiver.ty, Type::Str | Type::StrLiteral(_)) {
            let name = self.exact_argument_type_name(receiver.ty).ok_or_else(|| {
                error(
                    self.source,
                    instance,
                    "the unbound str descriptor needs an exact receiver type",
                )
            })?;
            let name = &name[..name.floor_char_boundary(100)];
            let message = format!(
                "descriptor '{}' for 'str' objects doesn't apply to a '{name}' object",
                method.name()
            );
            return self.descriptor_call_error(
                call,
                method,
                vec![callable, receiver],
                tail,
                message,
            );
        }
        let mut bound = call.clone();
        bound.arguments.args = tail.iter().cloned().collect();
        let receiver = Self::string_value(receiver);
        let result = match method {
            ir::StrDescriptor::IsAlnum => {
                self.splitlines_or_predicate(receiver, method.name(), &bound)?
            }
            _ => self.text_method(receiver, method.name(), &bound)?,
        };
        Ok(evaluated(callable, result))
    }

    fn descriptor_call_error(
        &mut self,
        call: &ast::ExprCall,
        method: ir::StrDescriptor,
        mut values: Vec<Expr>,
        tail: &[ast::Expr],
        message: String,
    ) -> Result<Expr, String> {
        self.graph.checked_overload(self.module, call.range());
        for arg in tail {
            self.graph.checked_excess_argument(self.module, arg.range());
            values.push(self.expr(arg)?);
        }
        for kw in &call.arguments.keywords {
            if kw.arg.as_deref() == Some("self") {
                self.graph
                    .checked_positional_keyword(self.module, kw.range());
            }
            values.push(self.expr(&kw.value)?);
        }
        self.graph.checked_keywords(
            self.module,
            call.arguments.keywords.iter().map(Ranged::range),
        );
        let ty = if method == ir::StrDescriptor::IsAlnum {
            Type::Bool
        } else {
            Type::Str
        };
        Ok(calls::type_error(ty, values, message))
    }

    pub(super) fn static_attribute(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        // A default is evaluated even on a successful lookup. Keywords can place
        // that evaluation before the object, so root each argument as supplied.
        let mut scope = calls::ArgumentScope::default();
        let mut receiver = None;
        let mut name = None;
        let mut default = None;
        let mut seen = [false; 3];
        let positional = call.arguments.args.iter().enumerate();
        let mut arguments = positional.collect::<Vec<_>>();
        for keyword in &call.arguments.keywords {
            let index = match keyword.arg.as_deref() {
                Some("obj") => 0,
                Some("attr") => 1,
                Some("default") => 2,
                _ => {
                    return Err(error(
                        self.source,
                        keyword,
                        "unsupported inspect.getattr_static keyword",
                    ));
                }
            };
            arguments.push((index, &keyword.value));
        }
        for (index, node) in arguments {
            if index >= 3 || std::mem::replace(&mut seen[index], true) {
                return Err(error(
                    self.source,
                    node,
                    "invalid inspect.getattr_static arguments",
                ));
            }
            if index == 1 {
                let ast::Expr::StringLiteral(label) = node else {
                    return Err(error(
                        self.source,
                        node,
                        "static attribute lookup requires a literal name",
                    ));
                };
                name = Some(label.value.to_string());
            } else {
                let value = self.expr(node)?;
                let local = scope.hold(self, value);
                if index == 0 {
                    receiver = Some(local)
                } else {
                    default = Some(local)
                }
            }
        }
        let (Some(receiver), Some(name)) = (receiver, name) else {
            return Err(error(
                self.source,
                call,
                "inspect.getattr_static requires obj and attr",
            ));
        };
        // Special names may be descriptors on object even when no declaration
        // appears in the source class. Their actual descriptor is not a field.
        if name.starts_with("__") {
            return Err(error(
                self.source,
                call,
                "this descriptor has no native static value",
            ));
        }
        let value = match receiver.ty {
            Type::PlainObject => missing(self.types, &name, default),
            Type::Str | Type::StrLiteral(_) if ir::StrDescriptor::named(&name).is_some() => {
                descriptor(ir::StrDescriptor::named(&name).unwrap())
            }
            Type::Int | Type::Bool | Type::Str | Type::StrLiteral(_) => {
                if builtin_plain_name(receiver.ty, &name) {
                    return Err(error(
                        self.source,
                        call,
                        "this builtin descriptor has no native static value",
                    ));
                }
                missing(self.types, &name, default)
            }
            Type::Class(class) => {
                let declaration = &self.classes.declarations.definitions[class];
                let lineage = || {
                    std::iter::once(class)
                        .chain(declaration.bases.iter().copied())
                        .map(|id| &self.classes.declarations.definitions[id])
                };
                if self.classes.has_concrete_subclasses(class)
                    || self.classes.methods[class].contains_key(&name)
                    || lineage().any(|base| {
                        base.dataclass.is_some_and(|opts| opts.slots || !opts.init)
                            || class_has_name(base, "__dict__")
                            || class_has_name(base, "__slots__")
                    })
                {
                    return Err(error(
                        self.source,
                        call,
                        "this class hierarchy or descriptor needs a native static value",
                    ));
                }
                if let Some(index) = declaration
                    .fields
                    .iter()
                    .position(|field| field.name == name)
                {
                    Expr {
                        ty: self.classes.fields[class][index],
                        kind: ExprKind::Field(Box::new(receiver), index),
                    }
                } else {
                    if lineage().any(|base| class_has_name(base, &name)) {
                        return Err(error(
                            self.source,
                            call,
                            "this class attribute has no native static value",
                        ));
                    }
                    missing(self.types, &name, default)
                }
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "static attribute lookup requires an exact native instance",
                ));
            }
        };
        Ok(scope.finish(value))
    }
}
