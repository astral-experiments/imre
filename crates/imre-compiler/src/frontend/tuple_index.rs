//! Exact fixed tuples can use a runtime index when every selectable field has the same representation.
use super::*;

impl Checker<'_> {
    pub(super) fn fixed_tuple_item(
        &mut self,
        tuple: Expr,
        fields: &[Type],
        key: &ast::Expr,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        // A known offset keeps its direct field access, including on heterogeneous tuples.
        let (operand, negative) = match key {
            ast::Expr::UnaryOp(op) if op.op == ast::UnaryOp::USub => (&*op.operand, true),
            _ => (key, false),
        };
        let literal = if let ast::Expr::NumberLiteral(number) = operand
            && let ast::Number::Int(n) = &number.value
        {
            Some(n)
        } else {
            None
        };
        if let Some(index) = literal.and_then(|n| n.as_u64()) {
            let offset = if negative && index != 0 {
                fields.len() as i128 - i128::from(index)
            } else {
                i128::from(index)
            };
            if let Ok(i) = usize::try_from(offset)
                && let Some(&ty) = fields.get(i)
            {
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(tuple), i),
                });
            }
        }
        let element = self
            .types
            .homogeneous_tuple_element(tuple.ty)
            .ok_or_else(|| {
                error(
                    self.source,
                    at,
                    if literal.is_some() {
                        "tuple index is out of bounds"
                    } else {
                        "a runtime tuple index requires one concrete element type"
                    },
                )
            })?;
        let key = self.expr(key)?;
        let key = match key.ty {
            Type::Int => key,
            Type::Bool => Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![key]),
            },
            _ => {
                return Err(error(
                    self.source,
                    at,
                    "runtime tuple indexing requires an exact int or bool index",
                ));
            }
        };
        Ok(Expr {
            ty: element,
            kind: ExprKind::Builtin(Builtin::TupleGet, vec![tuple, key]),
        })
    }
}
