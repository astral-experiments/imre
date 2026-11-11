//! Truth contexts need no heterogeneous value layout. Operand-returning boolean
//! expressions instead join only concrete types and their nullable forms.

use super::*;
use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;

impl Checker<'_> {
    /// Consumes truth directly so mixed operands such as `items and len(items)`
    /// do not require a general union. Every operand still receives type checks.
    pub(super) fn condition(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let ast::Expr::If(value) = expression {
            return self.conditional_with(value, false, Self::condition);
        }
        if let ast::Expr::BoolOp(boolean) = expression {
            let and = boolean.op == ast::BoolOp::And;
            let mut values = Vec::new();
            let mut exits = assignments::BooleanExits::new(self, boolean);
            for (index, value) in boolean.values.iter().enumerate() {
                values.push(self.condition(value)?);
                if index + 1 < boolean.values.len() {
                    exits.short_circuit(self, value, !and);
                    self.apply_facts(value, and);
                }
            }
            exits.finish(self, true);
            if and && let Some(test) = guarded_string_boundary(&values) {
                return Ok(test);
            }
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(and, values),
            });
        }
        let value = self.expr(expression)?;
        self.test_truth(value, expression)
    }

    fn needs_truth_dispatch(&self, ty: Type) -> bool {
        let ty = self.types.payload(ty).unwrap_or(ty);
        match ty {
            Type::Class(id) => self.classes.truth_dispatch.contains(&id),
            Type::Union(_) => self
                .types
                .members(ty)
                .unwrap()
                .into_iter()
                .any(|member| self.needs_truth_dispatch(member)),
            _ => false,
        }
    }

    /// Value layout alone cannot predict the truth of a base with concrete overrides.
    pub(super) fn known_truth(&self, ty: Type) -> Option<bool> {
        if self.needs_truth_dispatch(ty) {
            None
        } else {
            self.types.truthiness(ty)
        }
    }

    pub(super) fn test_truth(&mut self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        if self.registered.is_some() {
            // A recorded parameter type does not replace the object's live truth protocol.
            return Ok(registered::truth(value));
        }
        let value = Self::weak_target(value);
        if !self.needs_truth_dispatch(value.ty) {
            return Ok(self.truth_value(value));
        }
        if let Type::Class(class) = value.ty {
            return self.record_truth(class, value, at);
        }
        // An optional or tagged value holds the receiver until the chosen arm finishes.
        let (slot, tag, first, second) = self
            .protocol_alternatives(value.ty)
            .ok_or_else(|| error(self.source, at, "unsupported record union in a truth test"))?;
        let yes = self.test_truth(first, at)?;
        let no = self.test_truth(second, at)?;
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value)],
                value: Box::new(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Conditional(Box::new(tag), Box::new(yes), Box::new(no)),
                }),
                clear: vec![slot],
            },
        })
    }

    fn bool_method(&self, ty: Type) -> Option<usize> {
        let Type::Class(id) = self.types.payload(ty).unwrap_or(ty) else {
            return None;
        };
        self.classes.methods[id]
            .get("__bool__")
            .map(|method| method.id)
    }

    /// Make record protocols ordinary compiler-visible calls so exceptions,
    /// recursion checks, and cleanup share the usual call path.
    pub(super) fn truth_value(&mut self, value: Expr) -> Expr {
        if value.ty == Type::Bool {
            return value;
        }
        if matches!(value.ty, Type::Document(_)) {
            return documents::op(ir::DocumentOp::Truth, Type::Bool, vec![value]);
        }
        let Some(method) = self.bool_method(value.ty) else {
            return Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::Bool, vec![value]),
            };
        };
        self.calls.insert(method);
        let call = |receiver| Expr {
            ty: Type::Bool,
            kind: ExprKind::Call(method, ir::CallArgs::positional(vec![receiver])),
        };
        let Some(payload) = self.types.payload(value.ty) else {
            return call(value);
        };
        let slot = self.hidden(value.ty);
        let local = self.local(slot);
        let absent = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
        };
        let actual = call(Expr {
            ty: payload,
            kind: ExprKind::Unwrap(Box::new(local)),
        });
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value)],
                value: Box::new(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Conditional(
                        Box::new(absent),
                        Box::new(Expr::boolean(false)),
                        Box::new(actual),
                    ),
                }),
                clear: vec![slot],
            },
        }
    }

    pub(super) fn boolean_value(
        &mut self,
        boolean: &ast::ExprBoolOp,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        self.boolean_result(boolean, expected, false)
    }

    pub(super) fn class_skip_condition(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let ast::Expr::BoolOp(boolean) = expression
            && !boolean
                .values
                .iter()
                .any(|v| matches!(v, ast::Expr::BoolOp(_)))
        {
            return self.boolean_result(boolean, None, true);
        }
        self.expr(expression)
    }

    fn boolean_result(
        &mut self,
        boolean: &ast::ExprBoolOp,
        mut expected: Option<Type>,
        class_skip: bool,
    ) -> Result<Expr, String> {
        let and = boolean.op == ast::BoolOp::And;
        let (last, prefix) = boolean
            .values
            .split_last()
            .expect("boolean operands are nonempty");
        let mut operands = Vec::with_capacity(prefix.len());
        let mut exits = assignments::BooleanExits::new(self, boolean);
        let mut reachable = true;
        for first in prefix {
            let left = self.boolean_operand(first, expected)?;
            let known = match left.kind {
                ExprKind::Bool(value) => Some(value),
                _ => self.known_truth(left.ty),
            };
            // A present record with a false override can be the selected operand of "and".
            let selected = if and && self.needs_truth_dispatch(left.ty) {
                left.ty
            } else {
                self.types.truth_branch(left.ty, !and)
            };
            if reachable && known != Some(and) {
                exits.short_circuit(self, first, !and);
                reachable = known != Some(!and);
            }
            self.apply_facts(first, and);
            expected = expected
                .or_else(|| (selected != Type::None && known != Some(and)).then_some(selected));
            operands.push((first, left, known, selected));
        }
        let mut right = self.boolean_operand(last, expected)?;
        exits.finish(self, reachable);

        // Exact strings and booleans have immutable, non-raising truth. The
        // decorator observes the same flag after its reason is evaluated.
        let immutable_truth = |ty| matches!(ty, Type::Str | Type::Bool);
        // Exact bool operands already are their truth values, including in a return expression.
        let booleans = right.ty == Type::Bool
            && operands
                .iter()
                .all(|(_, value, _, _)| value.ty == Type::Bool);
        let skip_truth = class_skip
            && immutable_truth(right.ty)
            && operands
                .iter()
                .all(|(_, value, _, _)| immutable_truth(value.ty))
            && operands.iter().any(|(_, value, _, _)| value.ty != right.ty);
        if booleans || skip_truth {
            let mut values = operands
                .into_iter()
                .map(|(_, value, _, _)| self.truth_value(value))
                .collect::<Vec<_>>();
            values.push(self.truth_value(right));
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(and, values),
            });
        }

        // A skipped right operand can still affect type checking. Build each
        // right-hand result before its enclosing short-circuit expression.
        for (first, left, known, selected) in operands.into_iter().rev() {
            let ty = if known == Some(and) {
                right.ty
            } else if known == Some(!and) {
                selected
            } else {
                self.types.join_results(selected, right.ty).ok_or_else(|| {
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
            let predicate =
                if self.bool_method(left.ty).is_some() || self.needs_truth_dispatch(left.ty) {
                    let receiver = self.hidden(left.ty);
                    let condition = self.test_truth(self.local(receiver), first)?;
                    Some(ir::TruthPredicate {
                        receiver,
                        condition: Box::new(condition),
                    })
                } else {
                    None
                };
            right = Expr {
                ty,
                kind: ExprKind::BooleanValue(and, Box::new(left), Box::new(right), predicate),
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
                return self.conditional_with(value, false, |checker, arm| {
                    checker.boolean_operand(arm, hint)
                });
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

/// A nonempty string's first or last character equals a single-character literal
/// exactly when it has that prefix or suffix. Both tests are false on empty input.
fn guarded_string_boundary(values: &[Expr]) -> Option<Expr> {
    let [guard, test] = values else { return None };
    let ExprKind::Builtin(Builtin::Bool, args) = &guard.kind else {
        return None;
    };
    let [source] = args.as_slice() else {
        return None;
    };
    let (Type::Str, ExprKind::Local(slot)) = (source.ty, &source.kind) else {
        return None;
    };
    let boundary = |expr: &Expr| {
        let ExprKind::Compare(operands, ops) = &expr.kind else {
            return None;
        };
        let ([left, right], [CmpOp::Eq]) = (operands.as_slice(), ops.as_slice()) else {
            return None;
        };
        let (args, literal, text) = match (&left.kind, &right.kind) {
            (ExprKind::Builtin(Builtin::StringGet, args), ExprKind::Str(text)) => {
                (args, right, text)
            }
            (ExprKind::Str(text), ExprKind::Builtin(Builtin::StringGet, args)) => {
                (args, left, text)
            }
            _ => return None,
        };
        let [receiver, index] = args.as_slice() else {
            return None;
        };
        if !matches!(&receiver.kind, ExprKind::Local(other) if other == slot)
            || text.chars().next()?.len_utf8() != text.len()
        {
            return None;
        }
        let op = match &index.kind {
            ExprKind::Int(index) if index == "0" => ir::TextOperation::StartsWith,
            ExprKind::Int(index) if index == "-1" => ir::TextOperation::EndsWith,
            ExprKind::Unary(UnaryOp::USub, one) if matches!(&one.kind, ExprKind::Int(index) if index == "1") => {
                ir::TextOperation::EndsWith
            }
            _ => return None,
        };
        Some(Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::Text(op), vec![receiver.clone(), literal.clone()]),
        })
    };
    if let ExprKind::Boolean(and, tests) = &test.kind {
        let tests = tests.iter().map(boundary).collect::<Option<Vec<_>>>()?;
        Some(Expr {
            ty: Type::Bool,
            kind: ExprKind::Boolean(*and, tests),
        })
    } else {
        boundary(test)
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
