//! UID/GID argument conversion happens after all supplied expressions execute.

use super::*;

impl Checker<'_> {
    pub(in crate::frontend) fn credential_call(
        &mut self,
        call: &ast::ExprCall,
        name: &'static str,
    ) -> Result<Expr, String> {
        let getter = name.starts_with("get");
        let size = if getter {
            0
        } else if name.starts_with("setres") {
            3
        } else if name.starts_with("setre") {
            2
        } else {
            1
        };
        let returns = if !getter {
            Type::None
        } else if name.starts_with("getres") {
            self.types.tuple(vec![Type::Int; 3])
        } else {
            Type::Int
        };
        let mut scope = calls::ArgumentScope::default();
        let mut supplied = Vec::new();
        for arg in &call.arguments.args {
            if let ast::Expr::Starred(spread) = arg {
                supplied.extend(scope.spread(self, spread)?);
            } else {
                let value = self.expr(arg)?;
                supplied.push(scope.hold(self, value));
            }
        }
        for keyword in &call.arguments.keywords {
            if keyword.arg.is_none() {
                return Err(error(
                    self.source,
                    keyword,
                    "expanded keyword arguments are unsupported",
                ));
            }
            let value = self.expr(&keyword.value)?;
            scope.hold(self, value);
        }
        let count = supplied.len();
        let message = if !call.arguments.keywords.is_empty() {
            Some(format!("posix.{name}() takes no keyword arguments"))
        } else if size == count {
            None
        } else {
            Some(match size {
                0 => format!("posix.{name}() takes no arguments ({count} given)"),
                1 => format!("posix.{name}() takes exactly one argument ({count} given)"),
                _ => format!("{name} expected {size} arguments, got {count}"),
            })
        };
        if let Some(message) = message {
            self.graph
                .checked_positional_binding(self.module, call, size);
            return Ok(scope.finish(id_error(returns, message)));
        }
        let mut args = Vec::new();
        for value in supplied {
            let index = self.credential_index(value, name.ends_with("gid"), &call.func, None)?;
            args.push(scope.hold(self, index));
        }
        Ok(scope.finish(Expr {
            ty: returns,
            kind: ExprKind::Builtin(Builtin::Process(ProcessOperation::Credential(name)), args),
        }))
    }

    fn credential_index(
        &mut self,
        value: Expr,
        group: bool,
        at: &ast::Expr,
        index_owner: Option<&str>,
    ) -> Result<Expr, String> {
        let payload = self.types.payload(value.ty);
        let variants = self.types.members(value.ty);
        if payload.is_some() || variants.is_some() {
            let slot = self.hidden(value.ty);
            let mut cases = Vec::new();
            if let Some(payload) = payload {
                let condition = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(slot)]),
                };
                cases.push((
                    Some(condition),
                    Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    },
                ));
                cases.push((
                    None,
                    Expr {
                        ty: payload,
                        kind: ExprKind::Unwrap(Box::new(self.local(slot))),
                    },
                ));
            } else if let Some(types) = variants {
                for (i, &ty) in types.iter().enumerate() {
                    let condition = (i + 1 < types.len()).then(|| Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(
                            Builtin::IsInstance(vec![ir::InstanceType::Exact(ty)]),
                            vec![self.local(slot)],
                        ),
                    });
                    cases.push((
                        condition,
                        Expr {
                            ty,
                            kind: ExprKind::Builtin(Builtin::UnionProject, vec![self.local(slot)]),
                        },
                    ));
                }
            }
            let mut arms = cases.into_iter().rev();
            let (_, last) = arms.next().expect("a checked union has members");
            let mut result = self.credential_index(last, group, at, index_owner)?;
            for (condition, arg) in arms {
                result = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Conditional(
                        Box::new(condition.unwrap()),
                        Box::new(self.credential_index(arg, group, at, index_owner)?),
                        Box::new(result),
                    ),
                };
            }
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(result),
                    clear: vec![slot],
                },
            });
        }
        if let Some(owner) = index_owner {
            // The result of __index__ must itself be an integer. Never invoke
            // a second object's index protocol while checking the result.
            return Ok(credential_integer(value, group, owner));
        }
        if let Type::Class(id) = value.ty
            && self.classes.has_concrete_subclasses(id)
        {
            return self.dispatch_record(id, value, at, |checker, _, receiver| {
                checker.credential_exact(receiver, group, at)
            });
        }
        self.credential_exact(value, group, at)
    }

    fn credential_exact(
        &mut self,
        value: Expr,
        group: bool,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        let actual = match value.ty {
            Type::None => "NoneType".to_owned(),
            Type::List(_) | Type::ListInt | Type::ByteOutput => "list".to_owned(),
            Type::Dict(_) => "dict".to_owned(),
            Type::Set(_) => "set".to_owned(),
            Type::Tuple(_) => "tuple".to_owned(),
            Type::Class(id) => self.classes.declarations.definitions[id].name.clone(),
            other => self.type_name(other),
        };
        if let Type::Class(id) = value.ty {
            let Some(method) = self.classes.methods[id].get("__index__") else {
                return Ok(invalid_id(group, &actual));
            };
            let result = Expr {
                ty: method.returns,
                kind: ExprKind::Call(method.id, ir::CallArgs::positional(vec![value])),
            };
            self.calls.insert(method.id);
            let slot = self.hidden(result.ty);
            let integer = self.credential_index(self.local(slot), group, at, Some(&actual))?;
            // _Py_Uid_Converter and _Py_Gid_Converter replace any exception
            // raised by __index__ with their own TypeError naming the input.
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Scope {
                    body: vec![Statement::Try {
                        body: vec![Statement::Assign(slot, result)],
                        handlers: vec![ir::ExceptHandler {
                            kinds: Vec::new(),
                            name: None,
                            body: vec![Statement::Expr(invalid_id(group, &actual))],
                        }],
                        orelse: vec![],
                        finalbody: vec![],
                    }],
                    value: Box::new(integer),
                    clear: vec![slot],
                },
            });
        }
        Ok(credential_integer(value, group, &actual))
    }
}

fn credential_integer(mut value: Expr, group: bool, owner: &str) -> Expr {
    if value.ty == Type::Bool {
        value = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Int, vec![value]),
        };
    } else if value.ty != Type::Int {
        return invalid_id(group, owner);
    }
    Expr {
        ty: Type::Int,
        kind: ExprKind::Builtin(
            Builtin::Process(ProcessOperation::CredentialId { group }),
            vec![value],
        ),
    }
}

fn invalid_id(group: bool, owner: &str) -> Expr {
    let id = if group { "gid" } else { "uid" };
    id_error(Type::Int, format!("{id} should be integer, not {owner}"))
}

fn id_error(ty: Type, message: String) -> Expr {
    let exception = Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    };
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RaiseException, vec![exception]),
    }
}
