//! Truth contexts need no heterogeneous value layout. Operand-returning boolean
//! expressions instead join only concrete types and their nullable forms.

use super::*;

impl Checker<'_> {
    /// Consumes truth directly so mixed operands such as `items and len(items)`
    /// do not require a general union. Every operand still receives type checks.
    pub(super) fn condition(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let ast::Expr::If(value) = expression {
            return self.conditional_with(value, Self::condition);
        }
        if let ast::Expr::BoolOp(boolean) = expression {
            let and = boolean.op == ast::BoolOp::And;
            let mut values = Vec::new();
            let mut exit = None;
            for (index, value) in boolean.values.iter().enumerate() {
                values.push(self.condition(value)?);
                if index + 1 == boolean.values.len() {
                    self.merge_exit(&mut exit);
                } else {
                    let after = self.local_state();
                    self.apply_facts(value, !and);
                    self.merge_exit(&mut exit);
                    self.restore_state(after);
                    self.apply_facts(value, and);
                }
            }
            self.restore_state(exit.expect("boolean conditions have operands"));
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(and, values),
            });
        }
        let value = self.expr(expression)?;
        Ok(if value.ty == Type::Bool {
            value
        } else {
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::Bool, vec![value]),
            }
        })
    }

    pub(super) fn boolean_value(
        &mut self,
        boolean: &ast::ExprBoolOp,
        mut expected: Option<Type>,
    ) -> Result<Expr, String> {
        let and = boolean.op == ast::BoolOp::And;
        let (last, prefix) = boolean
            .values
            .split_last()
            .expect("boolean operands are nonempty");
        let mut operands = Vec::with_capacity(prefix.len());
        let mut exit = None;
        let mut reachable = true;
        for first in prefix {
            let left = self.boolean_operand(first, expected)?;
            let known = self.types.truthiness(left.ty);
            let selected = self.types.truth_branch(left.ty, !and);
            if reachable && known != Some(and) {
                let after = self.local_state();
                self.apply_facts(first, !and);
                self.merge_exit(&mut exit);
                self.restore_state(after);
                reachable = known != Some(!and);
            }
            self.apply_facts(first, and);
            expected = expected
                .or_else(|| (selected != Type::None && known != Some(and)).then_some(selected));
            operands.push((first, left, known, selected));
        }
        let mut right = self.boolean_operand(last, expected)?;
        if let Some(mut exit) = exit {
            if reachable {
                exit.retain_common(&self.assigned, &self.narrowed);
            }
            self.restore_state(exit);
        }

        // A skipped right operand can still affect type checking. Build each
        // right-hand result before its enclosing short-circuit expression.
        for (first, left, known, selected) in operands.into_iter().rev() {
            let ty = if known == Some(and) {
                right.ty
            } else if known == Some(!and) {
                selected
            } else {
                self.types.join(selected, right.ty).ok_or_else(|| {
                    error(
                        self.source,
                        first,
                        "and/or results require the same concrete type or its optional form",
                    )
                })?
            };
            if known.is_none() {
                right = self.coerce(right, ty, last)?;
            }
            right = Expr {
                ty,
                kind: ExprKind::BooleanValue(and, Box::new(left), Box::new(right)),
            };
        }
        Ok(right)
    }

    /// A result hint determines empty container literals without forcing an
    /// optional guard or other independently typed operand into the result type.
    pub(super) fn boolean_operand(
        &mut self,
        expression: &ast::Expr,
        hint: Option<Type>,
    ) -> Result<Expr, String> {
        match expression {
            ast::Expr::BoolOp(value) => return self.boolean_value(value, hint),
            ast::Expr::If(value) => {
                return self
                    .conditional_with(value, |checker, arm| checker.boolean_operand(arm, hint));
            }
            ast::Expr::Named(value) => return self.named_expression(value, hint),
            _ => {}
        }
        if let Some(hint) = hint {
            let concrete = self.types.payload(hint).unwrap_or(hint);
            let contextual = match expression {
                ast::Expr::Lambda(_) => self.types.signature(concrete).is_some(),
                ast::Expr::List(_) | ast::Expr::ListComp(_) => {
                    self.types.element(concrete).is_some()
                }
                ast::Expr::Tuple(_) => self.types.fields(concrete).is_some(),
                ast::Expr::Dict(_) | ast::Expr::DictComp(_) => {
                    self.types.mapping(concrete).is_some()
                }
                ast::Expr::Set(_) | ast::Expr::SetComp(_) => {
                    self.types.set_element(concrete).is_some()
                }
                ast::Expr::Subscript(value) if matches!(&*value.slice, ast::Expr::Slice(_)) => {
                    matches!(concrete, Type::Str | Type::Bytes)
                        || self.types.element(concrete).is_some()
                }
                ast::Expr::BinOp(value) if value.op == ast::Operator::Mult => {
                    self.types.element(concrete).is_some()
                }
                ast::Expr::Call(call) => matches!(&*call.func, ast::Expr::Name(name)
                    if (name.id == "list" && self.types.element(concrete).is_some())
                        || (name.id == "dict" && self.types.mapping(concrete).is_some())
                        || (name.id == "set" && self.types.set_element(concrete).is_some())),
                _ => false,
            };
            if contextual {
                return self.expr_expected(expression, concrete);
            }
        }
        self.expr(expression)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn joins_only_reachable_boolean_value_exits() {
        let falls = "def check(flag: bool) -> bool:\n    result = None or (written := flag) or flag\n    return written\n";
        crate::check(falls).unwrap();
        let stops = "def check(flag: bool) -> bool:\n    result = (first := flag) and None and (late := flag)\n    return first\n";
        crate::check(stops).unwrap();
        assert!(crate::check(&stops.replace("return first", "return late")).is_err());
    }
}
