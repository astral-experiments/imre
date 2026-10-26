//! Source-ordered construction of ImportError's argument tuple and keyword fields.
use super::*;

mod keywords;

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
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        // Typeshed omits name_from and treats unknown keywords as static errors;
        // our resolved builtin supplies the CPython constructor or its exact TypeError.
        self.graph.checked_keywords(
            self.module,
            call.arguments.keywords.iter().map(Ranged::range),
        );
        let callable = if receiver.is_some() {
            "ImportError.__init__".to_owned()
        } else if let Some(Binding::Definition(def)) = self.resolve(&call.func)
            && let Some(decl) = self
                .types
                .exception_declarations()
                .iter()
                .find(|d| d.id == kind)
        {
            let module = if def.module == self.graph.entry {
                "__main__"
            } else {
                &self.graph.modules[def.module].name
            };
            format!("{module}.{}", decl.name)
        } else if kind == 42 {
            "ModuleNotFoundError".to_owned()
        } else {
            "ImportError".to_owned()
        };
        let keywords = keywords::bind(self.source, call, &callable)?;
        let args = &call.arguments.args;
        if let Some(message) = keywords.binding_error {
            let ty = if receiver.is_some() {
                Type::None
            } else {
                Type::ImportException
            };
            let mut operands: Vec<_> = receiver.into_iter().collect();
            for node in args.iter().chain(keywords.values) {
                operands.push(self.expr(node)?);
            }
            return Ok(Self::type_error_after(operands, ty, message));
        }
        if let Some(name) = keywords.unexpected {
            let message = format!("ImportError() got an unexpected keyword argument '{name}'");
            if let Some(target) = receiver {
                let args = args
                    .iter()
                    .map(|n| self.import_argument(n))
                    .collect::<Result<_, _>>()?;
                let mut operands = vec![target, self.import_args_tuple(args)];
                for node in keywords.values {
                    operands.push(self.expr(node)?);
                }
                operands.push(unpacking::string(message));
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::ImportErrorRejectReset, operands),
                });
            }
            let operands = args
                .iter()
                .chain(keywords.values)
                .map(|n| self.expr(n))
                .collect::<Result<_, _>>()?;
            return Ok(Self::type_error_after(
                operands,
                Type::ImportException,
                message,
            ));
        }
        let args = args
            .iter()
            .map(|n| self.import_argument(n))
            .collect::<Result<_, _>>()?;
        let kwargs = keywords
            .values
            .into_iter()
            .enumerate()
            .map(|(i, n)| {
                if keywords.fields.contains(&Some(i + 1)) {
                    self.import_argument(n)
                } else {
                    self.expr(n)
                }
            })
            .collect::<Result<_, _>>()?;
        let new = self.import_error_value(kind, args, kwargs, keywords.fields);
        Ok(match receiver {
            Some(old) => Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::ImportErrorReset, vec![old, new]),
            },
            None => new,
        })
    }

    /// Convert a value when the native argument tuple or a keyword field retains it.
    fn import_argument(&mut self, node: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(node)?;
        if !self.boxable_value(value.ty) {
            return Err(error(
                self.source,
                node,
                "import exception arguments support None, bool, int, str, bytes, and fixed tuples of these values",
            ));
        }
        Ok(Self::string_value(value))
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
