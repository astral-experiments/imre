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
            let mut exits = Vec::new();
            for (index, value) in boolean.values.iter().enumerate() {
                values.push(self.condition(value)?);
                let after = self.local_state();
                if index + 1 == boolean.values.len() {
                    exits.push(after);
                } else {
                    self.apply_facts(value, !and);
                    exits.push(self.local_state());
                    self.restore_state(after);
                    self.apply_facts(value, and);
                }
            }
            self.join_states(exits);
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
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        self.boolean_operands(&boolean.values, boolean.op == ast::BoolOp::And, expected)
    }

    /// The left result is captured before any RHS writes. Its selected type is
    /// narrowed from that value, independently of the local's later contents.
    fn boolean_operands(
        &mut self,
        values: &[ast::Expr],
        and: bool,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        let (first, rest) = values.split_first().expect("boolean operands are nonempty");
        let left = self.boolean_operand(first, expected)?;
        if rest.is_empty() {
            return Ok(left);
        }
        let known = self.types.truthiness(left.ty);
        let selected = self.types.truth_branch(left.ty, !and);
        let after = self.local_state();
        self.apply_facts(first, !and);
        let stopped = self.local_state();
        self.restore_state(after);
        self.apply_facts(first, and);
        let hint =
            expected.or_else(|| (selected != Type::None && known != Some(and)).then_some(selected));
        let right = self.boolean_operands(rest, and, hint)?;
        let ty = if known == Some(and) {
            right.ty
        } else if known == Some(!and) {
            self.restore_state(stopped);
            selected
        } else {
            self.join_states([stopped, self.local_state()]);
            self.types.join(selected, right.ty).ok_or_else(|| {
                error(
                    self.source,
                    first,
                    "and/or results require the same concrete type or its optional form",
                )
            })?
        };
        let right = if known.is_none() {
            self.coerce(right, ty, rest.last().unwrap())?
        } else {
            right
        };
        Ok(Expr {
            ty,
            kind: ExprKind::BooleanValue(and, Box::new(left), Box::new(right)),
        })
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
