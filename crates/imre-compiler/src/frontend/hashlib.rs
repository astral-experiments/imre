//! A sealed hash object owns its state. Reading a digest leaves it usable.
use super::*;

impl Checker<'_> {
    pub(super) fn hmac_compare_digest(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [left, right] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "compare_digest requires two positional arguments",
            ));
        };
        let left = self.evaluated_argument(left)?;
        let right = self.evaluated_argument(right)?;
        let text = match (left.ty, right.ty) {
            (Type::Bytes, Type::Bytes) => false,
            (Type::Str | Type::StrLiteral(_), Type::Str | Type::StrLiteral(_)) => true,
            (a, b) => {
                let (Some(a_name), Some(b_name)) =
                    (self.known_nonbuffer_type(a), self.known_nonbuffer_type(b))
                else {
                    return Err(error(
                        self.source,
                        call,
                        "compare_digest requires concrete bytes or ASCII string operands",
                    ));
                };
                let message = if a == Type::Bytes || b == Type::Bytes {
                    let other = if a == Type::Bytes { b_name } else { a_name };
                    format!("a bytes-like object is required, not '{other}'")
                } else {
                    format!(
                        "unsupported operand types(s) or combination of types: '{a_name}' and '{b_name}'"
                    )
                };
                self.graph.checked_overload(self.module, call.range());
                return Ok(calls::type_error(Type::Bool, vec![left, right], message));
            }
        };
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::HmacCompareDigest { text }, vec![left, right]),
        })
    }

    pub(super) fn hash_constructor(
        &mut self,
        call: &ast::ExprCall,
        constructor: Intrinsic,
    ) -> Result<Expr, String> {
        let mut params = vec![
            Parameter {
                name: "data".into(),
                ty: Type::Bytes,
                kind: ParameterKind::PositionalOrKeyword,
                default: Some(Expr {
                    ty: Type::Bytes,
                    kind: ExprKind::Bytes(Vec::new()),
                }),
                sign_only: false,
            },
            Parameter {
                name: "usedforsecurity".into(),
                ty: Type::Bool,
                kind: ParameterKind::KeywordOnly,
                default: Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                }),
                sign_only: false,
            },
        ];
        // Both security flags select the same provider. Evaluate an explicit
        // flag and the input bytes in the caller's order.
        let op = if constructor == Intrinsic::HashNew {
            params.insert(
                0,
                Parameter {
                    name: "name".into(),
                    ty: Type::Str,
                    kind: ParameterKind::PositionalOrKeyword,
                    default: None,
                    sign_only: false,
                },
            );
            Builtin::HashNew
        } else if constructor == Intrinsic::Sha1 {
            Builtin::Sha1New
        } else {
            Builtin::Sha256New
        };
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(op, Type::Hash, args))
    }

    pub(super) fn hash_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        if attr.attr.as_str() == "hexdigest"
            && call.arguments.args.is_empty()
            && let ExprKind::Builtin(constructor @ (Builtin::Sha1New | Builtin::Sha256New), inputs) =
                receiver.kind
        {
            // An immediately consumed constructor needs no mutable state object.
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    if matches!(constructor, Builtin::Sha1New) {
                        Builtin::Sha1Hex
                    } else {
                        Builtin::Sha256Hex
                    },
                    inputs,
                ),
            });
        }
        let mut args = vec![receiver];
        let (op, ty) = match (attr.attr.as_str(), call.arguments.args.as_ref()) {
            ("update", [data]) => {
                args.push(self.expr_expected(data, Type::Bytes)?);
                (Builtin::HashUpdate, Type::None)
            }
            ("hexdigest", []) => (Builtin::HashDigest { hex: true }, Type::Str),
            ("digest", []) => (Builtin::HashDigest { hex: false }, Type::Bytes),
            ("copy", []) => (Builtin::HashCopy, Type::Hash),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "hash objects support update(bytes), digest(), hexdigest(), and copy()",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, args),
        })
    }

    pub(super) fn hash_attribute(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (op, ty) = match attr.attr.as_str() {
            "name" => (Builtin::HashName, Type::Str),
            "digest_size" => (Builtin::HashDigestSize, Type::Int),
            "block_size" => {
                return Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Scope {
                        body: vec![Statement::Expr(receiver)],
                        value: Box::new(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("64".into()),
                        }),
                        clear: Vec::new(),
                    },
                });
            }
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "hash objects expose name, digest_size, and block_size",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, vec![receiver]),
        })
    }
}
