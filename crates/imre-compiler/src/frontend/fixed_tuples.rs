//! A terminating length guard fixes the storage shape of tuple(list). Between
//! the guard and construction, only checked scalar element replacement is
//! admitted; a resize, aliasing call or rebinding loses the proof.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct Guard {
    slot: usize,
    width: usize,
    element: Type,
}

#[derive(Clone, Copy)]
pub(super) struct Call {
    pub(super) at: TextRange,
    guard: Guard,
}

impl Checker<'_> {
    /// Evaluate the needle and the complete tuple before its first comparison.
    /// Exact scalar equality has no callbacks or identity-only cases such as NaN.
    pub(super) fn tuple_contains(
        &mut self,
        key: Expr,
        tuple: Expr,
        fields: &[Type],
        comparison: &ast::ExprCompare,
    ) -> Result<Expr, String> {
        let key = if let Some(&ty) = fields.first() {
            if !matches!(ty, Type::Str | Type::Int | Type::Bool | Type::Bytes)
                || !fields.iter().all(|&field| field == ty)
            {
                return Err(error(
                    self.source,
                    comparison,
                    "tuple membership requires homogeneous string, bytes, integer or boolean fields",
                ));
            }
            self.coerce(key, ty, &comparison.operands[0])?
        } else {
            key
        };
        let key_slot = self.hidden(key.ty);
        let tuple_slot = self.hidden(tuple.ty);
        let terms = fields
            .iter()
            .enumerate()
            .map(|(index, &ty)| Expr {
                ty: Type::Bool,
                kind: ExprKind::Compare(
                    vec![
                        Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(self.local(tuple_slot)), index),
                        },
                        self.local(key_slot),
                    ],
                    vec![Compare::Equal],
                ),
            })
            .collect::<Vec<_>>();
        let contains = Expr {
            ty: Type::Bool,
            kind: if terms.is_empty() {
                ExprKind::Bool(false)
            } else {
                ExprKind::Boolean(false, terms)
            },
        };
        let value = if comparison.ops[0] == ast::CmpOp::NotIn {
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Not(Box::new(contains)),
            }
        } else {
            contains
        };
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(key_slot, key),
                    Statement::Assign(tuple_slot, tuple),
                ],
                value: Box::new(value),
                clear: vec![key_slot, tuple_slot],
            },
        })
    }

    pub(super) fn list_length_guard(&self, statement: &ast::Stmt) -> Option<Guard> {
        let ast::Stmt::If(branch) = statement else {
            return None;
        };
        if !branch.elif_else_clauses.is_empty()
            || !matches!(
                branch.body.last(),
                Some(
                    ast::Stmt::Raise(_)
                        | ast::Stmt::Return(_)
                        | ast::Stmt::Continue(_)
                        | ast::Stmt::Break(_)
                )
            )
        {
            return None;
        }
        let ast::Expr::Compare(test) = &*branch.test else {
            return None;
        };
        let ([ast::CmpOp::NotEq], [left, right]) = (test.ops.as_ref(), test.operands.as_ref())
        else {
            return None;
        };
        let (call, number) = match (left, right) {
            (ast::Expr::Call(call), ast::Expr::NumberLiteral(n))
            | (ast::Expr::NumberLiteral(n), ast::Expr::Call(call)) => (call, n),
            _ => return None,
        };
        if !self.unshadowed_builtin(&call.func, "len") || !call.arguments.keywords.is_empty() {
            return None;
        }
        let [ast::Expr::Name(name)] = call.arguments.args.as_ref() else {
            return None;
        };
        let ast::Number::Int(width) = &number.value else {
            return None;
        };
        let width = width
            .to_string()
            .parse::<usize>()
            .ok()
            .filter(|&n| n <= 64)?;
        let slot = *self.names.get(name.id.as_str())?;
        let element = self.types.element(self.locals[slot])?;
        if !matches!(
            element,
            Type::Int | Type::Bool | Type::Float | Type::Str | Type::Bytes
        ) {
            return None;
        }
        Some(Guard {
            slot,
            width,
            element,
        })
    }

    fn unshadowed_builtin(&self, func: &ast::Expr, name: &str) -> bool {
        func.as_name_expr().is_some_and(|n| n.id.as_str() == name)
            && !self.names.contains_key(name)
            && self.resolve(func).is_none()
            && self.local_function_binding(func).is_none()
    }

    pub(super) fn guarded_tuple(&self, statement: &ast::Stmt, guard: Guard) -> Option<Call> {
        let value = match statement {
            ast::Stmt::Assign(a) => &*a.value,
            ast::Stmt::AnnAssign(a) => a.value.as_deref()?,
            ast::Stmt::Return(r) => r.value.as_deref()?,
            ast::Stmt::Expr(stmt) => match &*stmt.value {
                ast::Expr::Yield(y) => y.value.as_deref()?,
                _ => return None,
            },
            _ => return None,
        };
        let mut call = value.as_call_expr()?;
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Cast))
            && call.arguments.keywords.is_empty()
        {
            let [ast::Expr::StringLiteral(_), val] = call.arguments.args.as_ref() else {
                return None;
            };
            call = val.as_call_expr()?;
        }
        if !self.unshadowed_builtin(&call.func, "tuple") || !call.arguments.keywords.is_empty() {
            return None;
        }
        let [ast::Expr::Name(name)] = call.arguments.args.as_ref() else {
            return None;
        };
        (self.names.get(name.id.as_str()) == Some(&guard.slot)).then_some(Call {
            at: call.range(),
            guard,
        })
    }

    pub(super) fn materialize_fixed_tuple(&self, call: Call) -> Expr {
        let Guard {
            slot,
            width,
            element,
        } = call.guard;
        let values = (0..width)
            .map(|index| Expr {
                ty: element,
                kind: ExprKind::Builtin(
                    Builtin::ListGet,
                    vec![
                        self.local(slot),
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int(index.to_string()),
                        },
                    ],
                ),
            })
            .collect();
        Expr {
            ty: self.types.tuple(vec![element; width]),
            kind: ExprKind::Tuple(values),
        }
    }
}

pub(super) fn preserves(guard: Guard, statements: &[Statement]) -> bool {
    if !matches!(statements, [Statement::SetItem(receiver, _, _)] if matches!(receiver.kind, ExprKind::Local(slot) if slot == guard.slot))
    {
        return false;
    }
    let mut pure = true;
    ir::walk_expressions(statements, &mut |expr| {
        pure &= match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Bytes(_) | ExprKind::None | ExprKind::Local(_) | ExprKind::Binary(..) | ExprKind::Negate(_) => true,
            ExprKind::Builtin(b,args) => b.contract(args).preserves_list_storage(),
            ExprKind::Scope { body, clear, .. } => !clear.contains(&guard.slot)
                && body.iter().all(|s| matches!(s, Statement::Assign(slot, _) | Statement::Clear(slot) if *slot != guard.slot)),
            _ => false,
        };
    });
    pure
}
