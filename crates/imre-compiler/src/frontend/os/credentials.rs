//! UID/GID argument conversion happens after all supplied expressions execute.

use super::*;

impl Checker<'_> {
    pub(in crate::frontend) fn credential_call(
        &mut self,
        call: &ast::ExprCall,
        name: &'static str,
    ) -> Result<Expr, String> {
        let size = crate::modules::credential_arity(name);
        let returns = if size != 0 {
            Type::None
        } else if name.starts_with("getres") {
            self.types.tuple(vec![Type::Int; 3])
        } else {
            Type::Int
        };
        self.posix_positional(call, name, size, returns, |checker, scope, supplied| {
            let mut args = Vec::new();
            for value in supplied {
                let index =
                    checker.credential_index(value, name.ends_with("gid"), &call.func, None)?;
                args.push(scope.hold(checker, index));
            }
            Ok(Expr {
                ty: returns,
                kind: ExprKind::Builtin(Builtin::Process(ProcessOperation::Credential(name)), args),
            })
        })
    }

    fn credential_index(
        &mut self,
        value: Expr,
        group: bool,
        at: &ast::Expr,
        index_owner: Option<&str>,
    ) -> Result<Expr, String> {
        if let Some((slot, test, first, second)) = self.protocol_alternatives(value.ty) {
            let yes = self.credential_index(first, group, at, index_owner)?;
            let mut no = self.credential_index(second, group, at, index_owner)?;
            // Each additional layout needs its own type name in the converter's
            // error; in particular, str | bytes | Path can reach its third arm.
            for member in self.types.members(value.ty).into_iter().flatten().skip(2) {
                let actual = self.local(slot);
                let test = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::IsInstance(vec![ir::InstanceType::Exact(member)]),
                        vec![actual.clone()],
                    ),
                };
                let arm = Expr {
                    ty: member,
                    kind: ExprKind::Builtin(Builtin::UnionProject, vec![actual]),
                };
                no = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Conditional(
                        Box::new(test),
                        Box::new(self.credential_index(arm, group, at, index_owner)?),
                        Box::new(no),
                    ),
                };
            }
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
                    }),
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
    posix_type_error(Type::Int, format!("{id} should be integer, not {owner}"))
}
