//! Read-only ZIP archives preserve directory order and own their file descriptor.

use super::*;

impl Checker<'_> {
    pub(super) fn zipfile_constructor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let args = call.arguments.args.as_ref();
        if args.len() > 2 {
            return Err(error(
                self.source,
                call,
                "ZipFile supports a file and a literal read mode",
            ));
        }
        let mut file = args.first();
        let mut mode = args.get(1);
        for keyword in &call.arguments.keywords {
            match keyword.arg.as_ref().map(|arg| arg.as_str()) {
                Some("file") if file.is_none() => file = Some(&keyword.value),
                Some("mode") if mode.is_none() => mode = Some(&keyword.value),
                _ => {
                    return Err(error(
                        self.source,
                        keyword,
                        "unsupported or repeated ZipFile argument",
                    ));
                }
            }
        }
        if let Some(mode) = mode
            && !matches!(mode, ast::Expr::StringLiteral(literal) if literal.value.to_str() == "r")
        {
            return Err(error(
                self.source,
                mode,
                "ZipFile supports literal read mode 'r'",
            ));
        }
        let file = file.ok_or_else(|| error(self.source, call, "ZipFile requires a file"))?;
        Ok(Expr {
            ty: Type::ZipFile,
            kind: ExprKind::Builtin(Builtin::ZipOpen, vec![self.path_argument(file)?]),
        })
    }

    pub(super) fn zipfile_method(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (op, ty) = match (attr.attr.as_str(), call.arguments.is_empty()) {
            ("namelist", true) => (Builtin::ZipNamelist, self.types.list(Type::Str)),
            ("close", true) => (Builtin::ZipClose, Type::None),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "ZipFile supports namelist() and close()",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, vec![receiver]),
        })
    }
}
