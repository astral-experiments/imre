//! Count each yielded key before asking for the next one.

use super::*;
use crate::modules::IntrinsicModule;

fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

fn raise(kind: &str, message: String, mut arguments: Vec<Expr>) -> Expr {
    arguments.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind(kind).unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    });
    Expr {
        ty: Type::None,
        kind: ExprKind::Builtin(Builtin::RaiseException, arguments),
    }
}

impl Checker<'_> {
    /// A rejected argument still evaluates as a value. Empty container layouts
    /// cannot escape from a call which always raises.
    fn count_argument(&mut self, arg: &ast::Expr) -> Result<Expr, String> {
        match arg {
            ast::Expr::Dict(d) if d.items.is_empty() => {
                self.expr_expected(arg, self.types.dict(Type::Int, Type::Int))
            }
            ast::Expr::List(l) if l.elts.is_empty() => self.expr_expected(arg, Type::ListInt),
            _ => self.expr(arg),
        }
    }

    pub(super) fn count_elements(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if let ast::Expr::Attribute(attr) = call.func.as_ref()
            && self.graph.resolve(self.module, &attr.value)
                == Some(Binding::IntrinsicModule(IntrinsicModule::Collections))
        {
            self.graph
                .checked_builtin_attribute(self.module, attr.range());
        }
        if call.arguments.args.len() != 2 || !call.arguments.keywords.is_empty() {
            let message = if call.arguments.keywords.is_empty() {
                format!(
                    "_count_elements expected 2 arguments, got {}",
                    call.arguments.args.len()
                )
            } else {
                "_collections._count_elements() takes no keyword arguments".to_owned()
            };
            let arguments = call
                .arguments
                .args
                .iter()
                .chain(call.arguments.keywords.iter().map(|k| &k.value))
                .map(|arg| self.count_argument(arg))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(raise("TypeError", message, arguments));
        }
        let target = &call.arguments.args[0];
        let source = &call.arguments.args[1];
        let mut scope = calls::ArgumentScope::default();
        let owner = if matches!(target, ast::Expr::Dict(d) if d.items.is_empty()) {
            None
        } else {
            Some(self.expr(target)?)
        };
        let hint = owner
            .as_ref()
            .and_then(|v| self.types.mapping(v.ty))
            .map(|(key, _)| key);
        // Counter.update consumes a mapping's counts; this helper consumes its keys.
        let mut input = self.counter_input(source, hint)?;
        if let Iteration::Container { iterable, .. } = &mut input {
            if matches!(
                iterable.ty,
                Type::None | Type::Bool | Type::Int | Type::Float
            ) {
                let name = if iterable.ty == Type::None {
                    "NoneType".to_owned()
                } else {
                    self.type_name(iterable.ty)
                };
                let mapping = match owner {
                    Some(v) => v,
                    None => self.count_argument(target)?,
                };
                return Ok(raise(
                    "TypeError",
                    format!("'{name}' object is not iterable"),
                    vec![mapping, iterable.clone()],
                ));
            }
            if matches!(iterable.ty, Type::StrLiteral(_)) {
                *iterable = Self::string_value(iterable.clone());
            }
        }
        let input = self.prepare_iteration(input, source, false)?;
        self.hint_empty_iteration(&input, Some(hint.unwrap_or(Type::Str)));
        let key_ty = self.locals[input.value];
        let mapping = match owner {
            Some(value) => value,
            None if self.types.hashable_key(key_ty) => {
                self.expr_expected(target, self.types.dict(key_ty, Type::Int))?
            }
            None => {
                return Err(error(
                    self.source,
                    source,
                    "counting into an empty dictionary requires an immutable key",
                ));
            }
        };
        let mapped = scope.hold(self, mapping);
        let shape = self.types.mapping(mapped.ty);
        if let Some((key, count)) = shape {
            self.expect(Type::Int, count, target)?;
            self.expect(key, key_ty, source)?;
        }
        let class = if let Type::Class(c) = mapped.ty {
            Some(c)
        } else {
            None
        };
        let mut before = Vec::new();
        if shape.is_none() && class.is_none() {
            let name = match mapped.ty {
                Type::None => "NoneType".to_owned(),
                Type::Int | Type::Bool | Type::Float | Type::Str | Type::Bytes => {
                    self.type_name(mapped.ty)
                }
                Type::PlainObject => "object".to_owned(),
                Type::List(_) | Type::ListInt => "list".to_owned(),
                Type::Tuple(_) => "tuple".to_owned(),
                _ => {
                    return Err(error(
                        self.source,
                        target,
                        "counting requires a native mapping or a concrete get and __setitem__",
                    ));
                }
            };
            // CPython acquires the iterator, then looks up get even for an empty source.
            before.push(Statement::Expr(raise(
                "AttributeError",
                format!("'{name}' object has no attribute 'get'"),
                vec![],
            )));
        }
        let body = self.expand_items_after(
            Iteration::Prepared(input),
            source,
            before,
            |checker, key| {
                let get = if let Some((key_type, _)) = shape {
                    let k = checker.coerce(key.clone(), key_type, source)?;
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(
                            Builtin::DictGetDefault,
                            vec![mapped.clone(), k, pipelines::integer(0)],
                        ),
                    }
                } else if class.is_some() {
                    checker.record_primitive(
                        mapped.clone(),
                        "get",
                        &[key.clone(), pipelines::integer(0)],
                        target,
                    )?
                } else {
                    return Ok(vec![]);
                };
                checker.expect(Type::Int, get.ty, target)?;
                let count = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Binary(
                        Binary::Add,
                        Box::new(get),
                        Box::new(pipelines::integer(1)),
                    ),
                };
                Ok(vec![if class.is_some() {
                    Statement::Expr(checker.record_primitive(
                        mapped.clone(),
                        "__setitem__",
                        &[key, count],
                        target,
                    )?)
                } else {
                    Statement::SetItem(mapped.clone(), key, count)
                }])
            },
        )?;
        Ok(scope.finish(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                value: Box::new(none()),
                clear: vec![],
            },
        }))
    }
}
