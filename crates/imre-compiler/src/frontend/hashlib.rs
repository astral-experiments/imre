//! The sealed SHA-256 object owns its state. Reading a digest leaves it usable.
use super::*;

impl Checker<'_> {
    pub(super) fn sha256_constructor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [
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
        // Both boolean values select SHA-256 here; still evaluate an explicit
        // flag and the input bytes in the caller's order.
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::Sha256New, Type::Sha256, args))
    }

    pub(super) fn sha256_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        if attr.attr.as_str() == "hexdigest"
            && call.arguments.args.is_empty()
            && let ExprKind::Builtin(Builtin::Sha256New, inputs) = receiver.kind
        {
            // An immediately consumed constructor needs no mutable state object.
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Sha256Hex, inputs),
            });
        }
        let mut args = vec![receiver];
        let (op, ty) = match (attr.attr.as_str(), call.arguments.args.as_ref()) {
            ("update", [data]) => {
                args.push(self.expr_expected(data, Type::Bytes)?);
                (Builtin::Sha256Update, Type::None)
            }
            ("hexdigest", []) => (Builtin::Sha256Digest { hex: true }, Type::Str),
            ("digest", []) => (Builtin::Sha256Digest { hex: false }, Type::Bytes),
            ("copy", []) => (Builtin::Sha256Copy, Type::Sha256),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "SHA-256 supports update(bytes), digest(), hexdigest(), and copy()",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, args),
        })
    }

    pub(super) fn sha256_attribute(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let value = match attr.attr.as_str() {
            "name" => Expr {
                ty: Type::Str,
                kind: ExprKind::Str("sha256".into()),
            },
            "digest_size" => Expr {
                ty: Type::Int,
                kind: ExprKind::Int("32".into()),
            },
            "block_size" => Expr {
                ty: Type::Int,
                kind: ExprKind::Int("64".into()),
            },
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "SHA-256 exposes name, digest_size, and block_size",
                ));
            }
        };
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Expr(receiver)],
                value: Box::new(value),
                clear: Vec::new(),
            },
        })
    }
}
