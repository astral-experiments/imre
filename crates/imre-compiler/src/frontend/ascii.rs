//! Python's ascii() calls repr once, then escapes non-ASCII characters.
use super::*;

impl Checker<'_> {
    pub(super) fn ascii_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if let ([value], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            return self.ascii_expr(value);
        }
        self.one_positional_error(call, "ascii", "obj", Type::Str)
    }

    pub(super) fn ascii_expr(&mut self, argument: &ast::Expr) -> Result<Expr, String> {
        // Empty literal containers have no element operations to constrain their layout.
        let value = match argument {
            ast::Expr::List(list) if list.elts.is_empty() => {
                self.expr_expected(argument, Type::ListInt)?
            }
            ast::Expr::Dict(dict) if dict.items.is_empty() => {
                self.expr_expected(argument, self.types.dict(Type::Int, Type::Int))?
            }
            _ => self.expr(argument)?,
        };
        let shown = match value.ty {
            Type::Exception | Type::NamedException(_) | Type::ImportException => Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::ExceptionRepr, vec![value]),
            },
            Type::Class(id)
                if self.classes.concrete_layouts(id).iter().all(|&class| {
                    self.classes.has_repr_override(class) || {
                        let definition = &self.classes.declarations.definitions[class];
                        definition.dataclass.is_some_and(|options| options.repr)
                            && definition
                                .fields
                                .iter()
                                .zip(&self.classes.fields[class])
                                .all(|(field, &ty)| !field.repr || self.has_builtin_repr(ty))
                    }
                }) =>
            {
                self.record_repr(value, argument)?
            }
            ty if self.has_builtin_repr(ty) => Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
            },
            _ => {
                return Err(error(
                    self.source,
                    argument,
                    "ascii requires a supported representation with known nested value layouts",
                ));
            }
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::AsciiEscape, vec![shown]),
        })
    }

    // Descriptor traversal of container contents never invokes Python methods.
    // Only admit shapes whose entire contents use built-in repr.
    pub(super) fn has_builtin_repr(&self, ty: Type) -> bool {
        match ty {
            Type::None
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::ListInt => true,
            Type::Optional(_) | Type::List(_) | Type::Dict(_) | Type::Tuple(_)
                if !self.types.is_counter(ty)
                    && !self.types.is_default_list_dict(ty)
                    && !self.types.is_partial_frame(ty)
                    && self.types.tuple_metadata(ty).is_none() =>
            {
                self.types
                    .children(ty)
                    .into_iter()
                    .all(|child| self.has_builtin_repr(child))
            }
            _ => false,
        }
    }
}
