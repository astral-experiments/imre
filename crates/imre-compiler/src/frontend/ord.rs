//! Builtin character ordinals and their exact, catchable call errors.

use super::*;

impl Checker<'_> {
    pub(super) fn ordinal_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        if let ([argument], []) = (args.args.as_ref(), args.keywords.as_ref()) {
            let value = self.expr(argument)?;
            self.graph.checked_overload(self.module, call.range());
            return self.ordinal(value, argument);
        }
        self.graph
            .checked_missing_argument(self.module, call.range());
        for kw in &args.keywords {
            if kw.arg.as_deref() == Some("c") {
                self.graph
                    .checked_positional_keyword(self.module, kw.range());
            }
        }
        self.graph
            .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
        for argument in args.args.iter().skip(1) {
            self.graph
                .checked_excess_argument(self.module, argument.range());
        }
        let mut values = Vec::new();
        for argument in &args.args {
            values.push(self.expr(argument)?);
        }
        for kw in &args.keywords {
            values.push(self.expr(&kw.value)?);
        }
        let message = if args.keywords.is_empty() {
            format!(
                "ord() takes exactly one argument ({} given)",
                args.args.len()
            )
        } else {
            "ord() takes no keyword arguments".to_owned()
        };
        Ok(Self::ordinal_error(values, message))
    }

    /// Prepare one conversion; the same expression runs on each pull inside map.
    pub(super) fn ordinal(&self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        let value = Self::string_value(value);
        let ty = self.types.payload(value.ty).unwrap_or(value.ty);
        if matches!(ty, Type::Str | Type::StrLiteral(_) | Type::Bytes)
            || self.types.members(ty).is_some_and(|members| {
                members
                    .iter()
                    .all(|member| matches!(member, Type::Str | Type::Bytes))
            })
        {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Ord, vec![value]),
            });
        }
        let actual = match value.ty {
            Type::None => "NoneType",
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Float => "float",
            Type::PlainObject => "object",
            Type::List(_) | Type::ListInt | Type::ByteOutput => "list",
            Type::Set(_) => "set",
            Type::Dict(_)
                if !self.types.is_counter(value.ty)
                    && !self.types.is_default_list_dict(value.ty) =>
            {
                "dict"
            }
            Type::Tuple(_) if self.types.namedtuple_definition(value.ty).is_none() => "tuple",
            Type::Class(id)
                if self
                    .types
                    .concrete_subclasses(id)
                    .iter()
                    .all(|child| *child == Type::Class(id)) =>
            {
                &self.classes.declarations.definitions[id].name
            }
            Type::Enum(id) => &self.classes.declarations.definitions[id].name,
            _ => {
                return Err(error(
                    self.source,
                    at,
                    "ord requires a supported character or an exact argument type",
                ));
            }
        };
        // CPython's %.200s type-name display discards a partial UTF-8 code point.
        let actual = &actual[..actual.floor_char_boundary(200)];
        let message = format!("ord() expected string of length 1, but {actual} found");
        Ok(Self::ordinal_error(vec![value], message))
    }

    fn ordinal_error(mut values: Vec<Expr>, message: String) -> Expr {
        values.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        });
        Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::RaiseException, values),
        }
    }
}
