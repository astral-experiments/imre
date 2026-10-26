//! Native ChainMap instances share their entries with the input dictionaries.

use super::*;

impl Checker<'_> {
    /// Bind Python's positional or named key and default in source order. An
    /// exact default gives an exact result; None and optional defaults stay nullable.
    pub(super) fn chainmap_get(
        &mut self,
        call: &ast::ExprCall,
        owner: Expr,
        key: Type,
        value: Type,
    ) -> Result<Expr, String> {
        let optional = self.types.optional(value);
        let none = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            optional,
            call,
        )?;
        let parameter = |name: &str, ty, default| Parameter {
            name: name.to_owned(),
            ty,
            kind: ParameterKind::PositionalOrKeyword,
            default,
            sign_only: false,
        };
        let parameters = [
            parameter("key", key, None),
            parameter("default", optional, Some(none)),
        ];
        let mut returns = optional;
        let args = self.arguments_with(
            call,
            &parameters,
            Some(owner),
            |checker, index, argument| {
                if index == 0 {
                    return checker.expr_expected(argument, key);
                }
                let default = checker.expr_expected(argument, optional)?;
                if value != optional
                    && let ExprKind::Present(inner) = default.kind
                {
                    returns = value;
                    Ok(*inner)
                } else {
                    Ok(default)
                }
            },
        )?;
        Ok(self.ordered_builtin(Builtin::ChainMapGet(value), returns, args))
    }

    /// Infer one concrete dictionary shape across the input mappings. An empty
    /// literal can get its type from a later argument without evaluating that
    /// argument early: the generated list retains the original argument order.
    pub(super) fn chainmap(
        &mut self,
        call: &ast::ExprCall,
        mut shape: Option<(Type, Type)>,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let mut mappings = Vec::new();
        for arg in &call.arguments.args {
            if matches!(arg, ast::Expr::Dict(dict) if dict.items.is_empty()) {
                mappings.push(None);
                continue;
            }
            let mapping = if let Some((key, value)) = shape {
                self.expr_expected(arg, self.types.dict(key, value))?
            } else {
                let mapping = self.expr(arg)?;
                let (key, value) = self.types.mapping(mapping.ty).ok_or_else(|| {
                    error(
                        self.source,
                        arg,
                        "ChainMap arguments require typed dictionaries",
                    )
                })?;
                shape = Some((key, value));
                self.coerce(mapping, self.types.dict(key, value), arg)?
            };
            mappings.push(Some(mapping));
        }
        if mappings.is_empty() {
            mappings.push(None);
        }
        let (key, value) = shape.unwrap_or((Type::Str, Type::Int));
        let dictionary = self.types.dict(key, value);
        let maps = mappings
            .into_iter()
            .map(|mapping| {
                mapping.unwrap_or_else(|| Expr {
                    ty: dictionary,
                    kind: ExprKind::Dict {
                        key,
                        value,
                        entries: Vec::new(),
                    },
                })
            })
            .collect();
        Ok(Expr {
            ty: self.types.chainmap(key, value),
            kind: ExprKind::Builtin(
                Builtin::ChainMapNew,
                vec![Expr {
                    ty: self.types.list(dictionary),
                    kind: ExprKind::List(maps),
                }],
            ),
        })
    }
}
