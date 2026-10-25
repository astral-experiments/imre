//! A SimpleNamespace is distinct from the ordered dictionary which it owns.
//! Its backing dict has the same invariant value layout as any native dict.
use super::*;

pub(super) fn backing(value: Expr, id: usize) -> Expr {
    Expr {
        ty: Type::Dict(id),
        kind: ExprKind::Builtin(Builtin::NamespaceDict, vec![value]),
    }
}

// Unboxed floats cannot retain the identity shortcut for a shared NaN in two
// dictionaries. Other unsupported layouts may invoke user equality or repr.
fn storable(types: &Types, item: Type) -> bool {
    let mut pending = vec![item];
    let mut seen = HashSet::new();
    while let Some(item) = pending.pop() {
        if !seen.insert(item) {
            continue;
        }
        match item {
            Type::Int
            | Type::Bool
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::None
            | Type::ListInt => {}
            Type::Optional(_)
            | Type::List(_)
            | Type::Dict(_)
            | Type::Tuple(_)
            | Type::SimpleNamespace(_) => pending.extend(types.children(item)),
            _ => return false,
        }
    }
    true
}

impl Checker<'_> {
    pub(super) fn namespace_attr_name(&self, attr: &ast::ExprAttribute) -> Result<Expr, String> {
        if attr.attr.starts_with("__") {
            return Err(error(
                self.source,
                attr,
                "this SimpleNamespace class descriptor is unsupported",
            ));
        }
        Ok(unpacking::string(attr.attr.as_str()))
    }

    pub(super) fn namespace_attribute(
        &self,
        receiver: Expr,
        id: usize,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        if attr.attr.as_str() == "__dict__" {
            return Ok(backing(receiver, id));
        }
        Ok(Expr {
            ty: self.types.mapping(Type::Dict(id)).unwrap().1,
            kind: ExprKind::Builtin(
                Builtin::NamespaceGet,
                vec![receiver, self.namespace_attr_name(attr)?],
            ),
        })
    }

    pub(super) fn simple_namespace(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let positional = match call.arguments.args.as_ref() {
            [] => None,
            [arg] => Some(arg),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "SimpleNamespace requires at most one positional mapping",
                ));
            }
        };
        let mut scope = calls::ArgumentScope::default();
        let mut source = None;
        let mut entries = Vec::new();
        let mut value_ty = None;
        let spreads = call
            .arguments
            .keywords
            .iter()
            .filter(|kw| kw.arg.is_none())
            .count();
        if spreads != 0 && (positional.is_some() || call.arguments.keywords.len() != 1) {
            return Err(error(
                self.source,
                call,
                "combine explicit keywords with a positional dictionary; mixed dynamic keyword expansions are unsupported",
            ));
        }
        let map = positional.or_else(|| {
            call.arguments
                .keywords
                .first()
                .filter(|kw| kw.arg.is_none())
                .map(|kw| &kw.value)
        });
        if let Some(input) = map {
            let value = match input {
                ast::Expr::Dict(dict) if dict.items.is_empty() => None,
                input => Some(self.expr(input)?),
            };
            if let Some(value) = value {
                let Some((Type::Str, item)) = self.types.mapping(value.ty) else {
                    return Err(error(
                        self.source,
                        input,
                        "SimpleNamespace requires an exact string-keyed dictionary",
                    ));
                };
                if value.ty != self.types.dict(Type::Str, item) {
                    return Err(error(
                        self.source,
                        input,
                        "SimpleNamespace requires a plain dictionary",
                    ));
                }
                value_ty = Some(item);
                source = Some(scope.hold(self, value));
            }
        }
        for kw in &call.arguments.keywords {
            if let Some(name) = &kw.arg {
                let value = self.expr(&kw.value)?;
                let value = if let Some(item) = value_ty {
                    self.coerce(value, item, &kw.value)?
                } else {
                    value
                };
                value_ty = Some(value.ty);
                entries.push((unpacking::string(name.as_str()), scope.hold(self, value)));
            }
        }
        let item = value_ty.unwrap_or(Type::Int);
        if !storable(self.types, item) {
            return Err(error(
                self.source,
                call,
                "this SimpleNamespace value cannot preserve Python identity, equality, and representation",
            ));
        }
        let ty = self.types.dict(Type::Str, item);
        let Type::Dict(id) = ty else { unreachable!() };
        let dictionary = if let Some(source) = source {
            let slot = self.hidden(ty);
            let mut body = vec![Statement::Assign(
                slot,
                Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::DictCopy, vec![source]),
                },
            )];
            body.extend(
                entries
                    .into_iter()
                    .map(|(name, value)| Statement::SetItem(self.local(slot), name, value)),
            );
            Expr {
                ty,
                kind: ExprKind::Scope {
                    body,
                    value: Box::new(self.local(slot)),
                    clear: vec![slot],
                },
            }
        } else {
            Expr {
                ty,
                kind: ExprKind::Dict {
                    key: Type::Str,
                    value: item,
                    entries,
                },
            }
        };
        Ok(scope.finish(Expr {
            ty: Type::SimpleNamespace(id),
            kind: ExprKind::Builtin(Builtin::NamespaceWrap, vec![dictionary]),
        }))
    }
}
