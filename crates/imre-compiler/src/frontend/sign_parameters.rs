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
        let packed = closed_calls::keyword_field(argument);
        if !sign_only && !packed {
            return self.expr_expected(argument, ty);
        }
        let value = self.expr(argument)?;
        if packed && self.types.payload(value.ty) == Some(ty) {
            return Ok(self.require_keyword_payload(value, ty));
        }
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
