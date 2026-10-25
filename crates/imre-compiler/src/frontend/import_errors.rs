//! Source-ordered construction of ImportError's argument tuple and keyword fields.
use super::*;

impl Checker<'_> {
    pub(super) fn exception_tuple_slot(&self, class: usize) -> bool {
        self.classes.declarations.definitions[class]
            .fields
            .iter()
            .any(|field| field.name == crate::classes::EXCEPTION_ARGS_SLOT)
    }

    pub(super) fn is_import_error(&self, kind: u32) -> bool {
        let base = self
            .types
            .exception_declarations()
            .iter()
            .find(|d| d.id == kind)
            .map_or(kind, |d| d.base);
        matches!(base, 41 | 42)
    }

    pub(super) fn boxable_value(&self, ty: Type) -> bool {
        matches!(
            ty,
            Type::None | Type::Bool | Type::Int | Type::Str | Type::StrLiteral(_) | Type::Bytes
        ) || self
            .types
            .payload(ty)
            .is_some_and(|ty| self.boxable_value(ty))
            || self
                .types
                .fields(ty)
                .is_some_and(|items| items.into_iter().all(|item| self.boxable_value(item)))
    }

    pub(super) fn import_error(
        &mut self,
        call: &ast::ExprCall,
        kind: u32,
        mut receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        // Typeshed omits name_from and treats unknown keywords as static errors;
        // our resolved builtin supplies the CPython constructor or its exact TypeError.
        self.graph.checked_keywords(
            self.module,
            call.arguments.keywords.iter().map(Ranged::range),
        );
        let mut fields = [None; 3];
        let mut rejected_reset = None;
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
                    if rejected_reset.is_none() {
                        let message =
                            format!("ImportError() got an unexpected keyword argument '{key}'");
                        let Some(target) = receiver.take() else {
                            return self.sys_type_error(call, Type::ImportException, message);
                        };
                        rejected_reset = Some((target, message));
                    }
                    continue;
                }
            };
            fields[slot] = Some(index + 1);
        }
        let mut value = |node: &ast::Expr| {
            let arg = self.expr(node)?;
            if !self.boxable_value(arg.ty) {
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
        if let Some((target, message)) = rejected_reset {
            let mut operands = vec![target, self.import_args_tuple(args)];
            for kw in &call.arguments.keywords {
                operands.push(self.expr(&kw.value)?);
            }
            operands.push(unpacking::string(message));
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::ImportErrorRejectReset, operands),
            });
        }
        let kwargs = call
            .arguments
            .keywords
            .iter()
            .map(|kw| value(&kw.value))
            .collect::<Result<Vec<_>, _>>()?;
        let new = self.import_error_value(kind, args, kwargs, fields);
        Ok(match receiver {
            Some(old) => Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::ImportErrorReset, vec![old, new]),
            },
            None => new,
        })
    }

    /// Retain the positional tuple independently of the import-specific fields.
    fn import_args_tuple(&self, args: Vec<Expr>) -> Expr {
        Expr {
            ty: self.types.tuple(args.iter().map(|arg| arg.ty).collect()),
            kind: ExprKind::Tuple(args),
        }
    }

    pub(super) fn import_error_value(
        &self,
        kind: u32,
        args: Vec<Expr>,
        kwargs: Vec<Expr>,
        fields: [Option<usize>; 3],
    ) -> Expr {
        let mut values = vec![self.import_args_tuple(args)];
        values.extend(kwargs);
        Expr {
            ty: Type::ImportException,
            kind: ExprKind::Builtin(Builtin::ImportErrorNew { kind, fields }, values),
        }
    }
}
