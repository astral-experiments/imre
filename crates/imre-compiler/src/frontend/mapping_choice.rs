//! Mapping access shared by a real dictionary and a record implementing the same operation.
use super::*;

impl Checker<'_> {
    pub(super) fn dict_or_record(&self, ty: Type) -> Option<(Type, usize)> {
        match self.types.members(ty)?.as_slice() {
            [dict @ Type::Dict(_), Type::Class(class)] => Some((*dict, *class)),
            _ => None,
        }
    }

    /// Read one branch after evaluating the receiver, preserving missing keys and method results.
    fn mapping_choice(
        &mut self,
        receiver: Expr,
        at: &impl Ranged,
        lower: impl FnOnce(&mut Self, Expr, Expr) -> Result<(Expr, Expr), String>,
    ) -> Result<Expr, String> {
        let (slot, test, dict, record) = self.protocol_alternatives(receiver.ty).unwrap();
        let (left, right) = lower(self, dict, record)?;
        let ty = self.types.join(left.ty, right.ty).ok_or_else(|| {
            error(
                self.source,
                at,
                "mapping alternatives need compatible return types",
            )
        })?;
        let left = self.coerce(left, ty, at)?;
        let right = self.coerce(right, ty, at)?;
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, receiver)],
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Conditional(Box::new(test), Box::new(left), Box::new(right)),
                }),
                clear: vec![slot],
            },
        })
    }

    pub(super) fn dict_or_record_item(
        &mut self,
        receiver: Expr,
        key: &ast::Expr,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        self.mapping_choice(receiver, at, |me, dict, record| {
            let (k, v) = me.types.mapping(dict.ty).unwrap();
            let key = me.expr_expected(key, k)?;
            let right =
                me.record_primitive(record, "__getitem__", std::slice::from_ref(&key), at)?;
            Ok((
                Expr {
                    ty: v,
                    kind: ExprKind::Builtin(Builtin::DictGet, vec![dict, key]),
                },
                right,
            ))
        })
    }

    pub(super) fn dict_or_record_get(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        self.mapping_choice(receiver, call, |me, dict, record| {
            let (k, v) = me.types.mapping(dict.ty).unwrap();
            let (builtin, ty, args) = match call.arguments.args.as_slice() {
                [key] => (
                    Builtin::DictTryGet(v),
                    me.types.optional(v),
                    vec![dict, me.expr_expected(key, k)?],
                ),
                [key, default] => {
                    let key = me.expr_expected(key, k)?;
                    let default = me.expr(default)?;
                    if default.ty == Type::None {
                        (
                            Builtin::DictTryGet(v),
                            me.types.optional(v),
                            vec![dict, key, default],
                        )
                    } else {
                        (
                            Builtin::DictGetDefault,
                            v,
                            vec![dict, key, me.coerce(default, v, call)?],
                        )
                    }
                }
                _ => {
                    return Err(error(
                        me.source,
                        call,
                        "a mapping get requires a key and an optional default",
                    ));
                }
            };
            let Type::Class(class) = record.ty else {
                unreachable!()
            };
            let right = me.iteration_method_call(call, attr, class, record)?;
            Ok((
                Expr {
                    ty,
                    kind: ExprKind::Builtin(builtin, args),
                },
                right,
            ))
        })
    }
}
