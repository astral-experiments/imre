//! Membership of immutable bytes with statically known needle representations.

use super::*;

impl Checker<'_> {
    pub(super) fn bytes_membership(
        &self,
        mut needle: Expr,
        haystack: Expr,
        op: ast::CmpOp,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        let builtin = match needle.ty {
            Type::Bytes => Builtin::BytesContains,
            Type::Int => Builtin::BytesContainsInt,
            Type::Bool => {
                needle = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int, vec![needle]),
                };
                Builtin::BytesContainsInt
            }
            ty => {
                // Only statically exact Python types: a record can provide
                // __index__/the buffer protocol, and a union needs narrowing.
                let name = match ty {
                    Type::None => "NoneType",
                    Type::Float => "float",
                    Type::Str | Type::StrLiteral(_) => "str",
                    Type::ListInt | Type::List(_) => "list",
                    Type::Set(_) => "set",
                    Type::Dict(_) if self.types.is_counter(ty) => "Counter",
                    Type::Dict(_) if self.types.is_ordered_dict(ty) => "collections.OrderedDict",
                    Type::Dict(_) if self.types.is_default_list_dict(ty) => {
                        "collections.defaultdict"
                    }
                    Type::Dict(_) => "dict",
                    Type::Tuple(_)
                        if self.types.namedtuple_definition(ty).is_none()
                            && !self.types.is_partial_frame(ty) =>
                    {
                        "tuple"
                    }
                    _ => {
                        return Err(error(
                            self.source,
                            at,
                            "bytes membership requires a concrete bytes or integer needle",
                        ));
                    }
                };
                let exception = Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("TypeError").unwrap(),
                        message: Some(Box::new(unpacking::string(format!(
                            "a bytes-like object is required, not '{name}'"
                        )))),
                    },
                };
                // CPython evaluates both operands before validating the needle.
                // A failure producing the haystack wins over this TypeError.
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::RaiseException,
                        vec![needle, haystack, exception],
                    ),
                });
            }
        };
        let found = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(builtin, vec![needle, haystack]),
        };
        Ok(if op == ast::CmpOp::NotIn {
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Not(Box::new(found)),
            }
        } else {
            found
        })
    }
}
