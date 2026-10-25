//! Lower copy.copy only for types whose Python copy behavior is known.

use super::*;

impl Checker<'_> {
    pub(super) fn shallow_copy(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let source = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([value], []) => value,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|name| name.as_str() == "x") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "copy.copy requires one argument, x",
                ));
            }
        };
        let source = self.expr(source)?;
        let ty = source.ty;
        match ty {
            Type::None
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::StrEnum(_)
            | Type::Enum(_) => Ok(source),
            Type::Tuple(_)
                if self
                    .types
                    .fields(ty)
                    .is_some_and(|fields| self.types.tuple(fields) == ty) =>
            {
                Ok(source)
            }
            Type::ListInt | Type::List(_) => Ok(self.list_snapshot(source)),
            Type::Dict(_) | Type::Set(_) => Ok(Expr {
                ty,
                kind: ExprKind::Builtin(
                    if matches!(ty, Type::Dict(_)) {
                        Builtin::DictCopy
                    } else {
                        Builtin::KeySet
                    },
                    vec![source],
                ),
            }),
            _ => Err(error(
                self.source,
                call,
                "copy.copy requires a supported atomic type or container",
            )),
        }
    }

    /// A full slice duplicates storage while acquiring each existing element.
    pub(super) fn list_snapshot(&self, list: Expr) -> Expr {
        let absent = Expr {
            ty: self.types.optional(Type::Int),
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        };
        Expr {
            ty: list.ty,
            kind: ExprKind::Builtin(
                Builtin::Slice,
                vec![list, absent.clone(), absent.clone(), absent],
            ),
        }
    }
}
