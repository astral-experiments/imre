//! Native ChainMap instances share their entries with the input dictionaries.

use super::*;

impl Checker<'_> {
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
