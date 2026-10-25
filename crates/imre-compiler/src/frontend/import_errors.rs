//! Source-ordered construction of ImportError's argument tuple and keyword fields.
use super::*;

impl Checker<'_> {
    pub(super) fn is_import_error(&self, kind: u32) -> bool {
        let base = self
            .types
            .exception_declarations()
            .iter()
            .find(|d| d.id == kind)
            .map_or(kind, |d| d.base);
        matches!(base, 41 | 42)
    }

    pub(super) fn import_argument_supported(&self, ty: Type) -> bool {
        matches!(
            ty,
            Type::None | Type::Bool | Type::Int | Type::Str | Type::StrLiteral(_) | Type::Bytes
        ) || self.types.fields(ty).is_some_and(|items| {
            items
                .into_iter()
                .all(|item| self.import_argument_supported(item))
        })
    }

    pub(super) fn import_error(&mut self, call: &ast::ExprCall, kind: u32) -> Result<Expr, String> {
        // Typeshed omits name_from and treats unknown keywords as static errors;
        // our resolved builtin supplies the CPython constructor or its exact TypeError.
        self.graph.checked_keywords(
            self.module,
            call.arguments.keywords.iter().map(Ranged::range),
        );
        let mut fields = [None; 3];
        for (index, kw) in call.arguments.keywords.iter().enumerate() {
            let Some(key) = &kw.arg else {
                return Err(error(
                    self.source,
                    kw,
                    "expanded import exception keywords require a fixed signature",
                ));
            };
            let slot = match key.as_str() {
                "name" => 0,
                "path" => 1,
                "name_from" => 2,
                _ => {
                    return self.sys_type_error(
                        call,
                        Type::ImportException,
                        format!("ImportError() got an unexpected keyword argument '{key}'"),
                    );
                }
            };
            fields[slot] = Some(index + 1);
        }
        let mut value = |node: &ast::Expr| {
            let arg = self.expr(node)?;
            if !self.import_argument_supported(arg.ty) {
                return Err(error(
                    self.source,
                    node,
                    "import exception arguments support None, bool, int, str, bytes, and fixed tuples of these values",
                ));
            }
            Ok(Self::string_value(arg))
        };
        let args = call
            .arguments
            .args
            .iter()
            .map(&mut value)
            .collect::<Result<Vec<_>, _>>()?;
        let kwargs = call
            .arguments
            .keywords
            .iter()
            .map(|kw| value(&kw.value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.import_error_value(kind, args, kwargs, fields))
    }

    pub(super) fn import_error_value(
        &self,
        kind: u32,
        args: Vec<Expr>,
        kwargs: Vec<Expr>,
        fields: [Option<usize>; 3],
    ) -> Expr {
        let ty = self.types.tuple(args.iter().map(|arg| arg.ty).collect());
        let mut values = vec![Expr {
            ty,
            kind: ExprKind::Tuple(args),
        }];
        values.extend(kwargs);
        Expr {
            ty: Type::ImportException,
            kind: ExprKind::Builtin(Builtin::ImportErrorNew { kind, fields }, values),
        }
    }
}
