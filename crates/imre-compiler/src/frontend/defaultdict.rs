//! Explicit missing-key calls and the factory on a closed list-valued defaultdict.

use super::*;

impl Checker<'_> {
    /// The class descriptor also accepts a defaultdict viewed through its base dict type.
    pub(super) fn defaultdict_missing(
        &mut self,
        call: &ast::ExprCall,
        receiver: Expr,
        args: &[ast::Expr],
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [key] = args else {
            return Err(error(
                self.source,
                call,
                "defaultdict.__missing__ requires one key",
            ));
        };
        let (key_type, value) = self.types.mapping(receiver.ty).ok_or_else(|| {
            error(
                self.source,
                call,
                "defaultdict.__missing__ requires a dictionary receiver",
            )
        })?;
        let key = self.expr_expected(key, key_type)?;
        Ok(Expr {
            ty: value,
            kind: ExprKind::Builtin(Builtin::DefaultDictMissing, vec![receiver, key]),
        })
    }

    /// Read the current factory on the shared mapping. Return the builtin class object or None.
    pub(super) fn default_factory(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let ty = self.types.optional(Type::BuiltinClass);
        let enabled = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::DefaultFactoryEnabled, vec![receiver]),
        };
        let list = self.coerce(annotations::builtin_type("list").unwrap(), ty, attr)?;
        let absent = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            ty,
            attr,
        )?;
        Ok(Expr {
            ty,
            kind: ExprKind::Conditional(Box::new(enabled), Box::new(list), Box::new(absent)),
        })
    }
}
