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
        let value = self.expr(file)?;
        let (op, value) = if matches!(value.ty, Type::BinaryIO | Type::BytesIO) {
            (Builtin::ZipOpenFile, value)
        } else {
            (Builtin::ZipOpen, self.path_text(value, file)?)
        };
        Ok(Expr {
            ty: Type::ZipFile,
            kind: ExprKind::Builtin(op, vec![value]),
        })
    }

    pub(super) fn zip_attribute(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        use ir::ZipField::*;
        let (op, ty) = match (receiver.ty, attr.attr.as_str()) {
            (Type::ZipFile, "filename") => (Builtin::ZipFilename, self.types.optional(Type::Str)),
            (Type::ZipInfo, "filename") => (Builtin::ZipField(Name), Type::Str),
            (Type::ZipInfo, "file_size") => (Builtin::ZipField(Size), Type::Int),
            (Type::ZipInfo, "compress_size") => (Builtin::ZipField(CompressedSize), Type::Int),
            (Type::ZipInfo, "CRC") => (Builtin::ZipField(Crc), Type::Int),
            (Type::ZipInfo, "compress_type") => (Builtin::ZipField(Method), Type::Int),
            (Type::ZipInfo, "external_attr") => (Builtin::ZipField(ExternalAttr), Type::Int),
            _ => return Err(error(self.source, attr, "unsupported ZIP attribute")),
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, vec![receiver]),
        })
    }

    pub(super) fn zipfile_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let method = attr.attr.as_str();
        let import_data = method == "_read_import_data"
            && self.graph.modules[self.module].is_bundled("zipimport");
        let mut args = vec![receiver];
        let (op, ty) = if matches!(method, "open" | "read" | "getinfo") || import_data {
            let positional = call.arguments.args.as_ref();
            if positional.len() > (if method == "open" { 2 } else { 1 }) {
                return Err(error(self.source, call, "unsupported ZIP member arguments"));
            }
            let mut name = positional.first();
            let mut mode = positional.get(1);
            for kw in &call.arguments.keywords {
                match kw.arg.as_ref().map(|a| a.as_str()) {
                    Some("name") if name.is_none() => name = Some(&kw.value),
                    Some("mode") if method == "open" && mode.is_none() => mode = Some(&kw.value),
                    _ => {
                        return Err(error(
                            self.source,
                            kw,
                            "unsupported or repeated ZIP member argument",
                        ));
                    }
                }
            }
            if mode.is_some_and(
                |m| !matches!(m, ast::Expr::StringLiteral(s) if s.value.to_str() == "r"),
            ) {
                return Err(error(
                    self.source,
                    call,
                    "ZipFile.open supports literal read mode 'r'",
                ));
            }
            let name = name.ok_or_else(|| error(self.source, call, "missing ZIP member name"))?;
            let value = self.expr(name)?;
            let (op, ty) = match (method, value.ty) {
                ("_read_import_data", Type::Str) => (Builtin::ZipImportData, Type::Bytes),
                ("getinfo", Type::Str) => (Builtin::ZipGetInfo, Type::ZipInfo),
                ("read", Type::Str | Type::ZipInfo) => (Builtin::ZipRead, Type::Bytes),
                ("open", Type::Str | Type::ZipInfo) => (Builtin::ZipMember, Type::BinaryIO),
                _ => return Err(error(self.source, name, "unsupported ZIP member key")),
            };
            args.push(value);
            (op, ty)
        } else {
            match (method, call.arguments.is_empty()) {
                ("namelist", true) => (Builtin::ZipNamelist, self.types.list(Type::Str)),
                ("infolist", true) => (Builtin::ZipInfoList, self.types.list(Type::ZipInfo)),
                ("testzip", true) => (Builtin::ZipTest, self.types.optional(Type::Str)),
                ("close", true) => (Builtin::ZipClose, Type::None),
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "unsupported read-only ZipFile method or arguments",
                    ));
                }
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, args),
        })
    }
}
