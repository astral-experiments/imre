//! Direct calls may pass an exact integer sign to a float parameter whose
//! observable uses are exclusively comparisons with literal integer zero.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

pub(super) fn only_compares_zero(function: &ast::StmtFunctionDef, parameter: &str) -> bool {
    if narrowing::assigned_names(&function.body).contains(parameter) {
        return false;
    }
    let mut reads = Reads {
        parameter,
        compared: false,
        other: false,
    };
    reads.visit_body(&function.body);
    reads.compared && !reads.other
}

struct Reads<'a> {
    parameter: &'a str,
    compared: bool,
    other: bool,
}

impl<'a> Visitor<'a> for Reads<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if let ast::Stmt::FunctionDef(nested) = stmt {
            self.other |= comprehensions::function_reads(nested).contains(self.parameter);
            self.visit_parameters(&nested.parameters);
        } else {
            walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let ast::Expr::Compare(test) = expr
            && matches!(
                test.ops.as_ref(),
                [ast::CmpOp::Eq
                    | ast::CmpOp::NotEq
                    | ast::CmpOp::Lt
                    | ast::CmpOp::LtE
                    | ast::CmpOp::Gt
                    | ast::CmpOp::GtE]
            )
            && let [ast::Expr::Name(name), ast::Expr::NumberLiteral(number)]
            | [ast::Expr::NumberLiteral(number), ast::Expr::Name(name)] = test.operands.as_ref()
            && name.id == self.parameter
            && matches!(&number.value, ast::Number::Int(value) if value.as_i64() == Some(0))
        {
            self.compared = true;
            return;
        }
        match expr {
            ast::Expr::Name(name) => self.other |= name.id == self.parameter,
            ast::Expr::Lambda(_)
            | ast::Expr::Generator(_)
            | ast::Expr::ListComp(_)
            | ast::Expr::SetComp(_)
            | ast::Expr::DictComp(_) => {
                self.other |=
                    comprehensions::free_reads(expr, HashSet::new()).contains(self.parameter);
            }
            _ => walk_expr(self, expr),
        }
    }
}

impl Checker<'_> {
    pub(super) fn direct_argument(
        &mut self,
        argument: &ast::Expr,
        ty: Type,
        sign_only: bool,
    ) -> Result<Expr, String> {
        if !sign_only {
            return self.expr_expected(argument, ty);
        }
        let value = self.expr(argument)?;
        self.direct_value(value, argument, ty, true)
    }

    pub(super) fn direct_value(
        &self,
        value: Expr,
        argument: &ast::Expr,
        ty: Type,
        sign_only: bool,
    ) -> Result<Expr, String> {
        if sign_only && value.ty == Type::Int {
            Ok(Expr {
                ty: Type::Float,
                kind: ExprKind::Builtin(Builtin::IntSignAsFloat, vec![value]),
            })
        } else {
            self.coerce(value, ty, argument)
        }
    }
}

impl Checker<'_> {
    /// Evaluate both operands before handling nullable integers at the addition.
    pub(super) fn nullable_int_sum(&mut self, left: Expr, right: Expr) -> Expr {
        let mut scope = calls::ArgumentScope::default();
        let left = scope.hold(self, left);
        let right = scope.hold(self, right);
        let integer = |v: &Expr| {
            if v.ty == Type::Int {
                v.clone()
            } else {
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Unwrap(Box::new(v.clone())),
                }
            }
        };
        let if_none = |v: &Expr, absent: Expr, present: Expr| {
            if v.ty == Type::Int {
                present
            } else {
                let test = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![v.clone()]),
                };
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Conditional(
                        Box::new(test),
                        Box::new(absent),
                        Box::new(present),
                    ),
                }
            }
        };
        let error = |l, r| {
            calls::type_error(
                Type::Int,
                Vec::new(),
                format!("unsupported operand type(s) for +: '{l}' and '{r}'"),
            )
        };
        let sum = Expr {
            ty: Type::Int,
            kind: ExprKind::Binary(
                Binary::Add,
                Box::new(integer(&left)),
                Box::new(integer(&right)),
            ),
        };
        let absent = if_none(
            &right,
            error("NoneType", "NoneType"),
            error("NoneType", "int"),
        );
        let present = if_none(&right, error("int", "NoneType"), sum);
        scope.finish(if_none(&left, absent, present))
    }
}
