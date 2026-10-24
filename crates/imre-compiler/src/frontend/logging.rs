//! Logging keeps the logger's name and accepts closed scalar substitutions.

use super::*;

impl Checker<'_> {
    pub(super) fn logging_method(
        &mut self,
        logger: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let level = match attr.attr.as_str() {
            "debug" => 10,
            "info" => 20,
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "supported logger methods are debug and info",
                ));
            }
        };
        let (format, values) = call
            .arguments
            .args
            .split_first()
            .ok_or_else(|| error(self.source, call, "logging requires a message"))?;
        let message = if values.is_empty() {
            self.expr_expected(format, Type::Str)?
        } else {
            let ast::Expr::StringLiteral(format) = format else {
                return Err(error(
                    self.source,
                    format,
                    "logging with substitutions requires a literal format",
                ));
            };
            check_string(format, self.source)?;
            let mut pieces = Vec::new();
            let mut values = values.iter();
            let mut chars = format.value.to_str().chars();
            let mut literal = String::new();
            while let Some(ch) = chars.next() {
                if ch != '%' {
                    literal.push(ch);
                    continue;
                }
                match chars.next() {
                    Some('%') => literal.push('%'),
                    Some('s') => {
                        if !literal.is_empty() {
                            pieces.push(Expr {
                                ty: Type::Str,
                                kind: ExprKind::Str(std::mem::take(&mut literal)),
                            });
                        }
                        let value = values
                            .next()
                            .ok_or_else(|| error(self.source, call, "too few logging arguments"))?;
                        let value = self.expr(value)?;
                        if !matches!(
                            value.ty,
                            Type::Str
                                | Type::StrLiteral(_)
                                | Type::StrEnum(_)
                                | Type::Path
                                | Type::Int
                                | Type::Bool
                                | Type::Float
                                | Type::None
                        ) {
                            return Err(error(
                                self.source,
                                call,
                                "logging substitutions require scalar values or paths",
                            ));
                        }
                        pieces.push(Expr {
                            ty: Type::Str,
                            kind: ExprKind::Builtin(Builtin::Str, vec![value]),
                        });
                    }
                    _ => {
                        return Err(error(
                            self.source,
                            call,
                            "logging supports %s and %% in literal formats",
                        ));
                    }
                }
            }
            if values.next().is_some() {
                return Err(error(self.source, call, "too many logging arguments"));
            }
            if !literal.is_empty() {
                pieces.push(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(literal),
                });
            }
            pieces
                .into_iter()
                .reduce(|left, right| Expr {
                    ty: Type::Str,
                    kind: ExprKind::Binary(Binary::Concat, Box::new(left), Box::new(right)),
                })
                .expect("at least one substitution")
        };
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::LogWrite(level), vec![logger, message]),
        })
    }
}
