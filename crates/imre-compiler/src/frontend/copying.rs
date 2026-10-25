//! Lower copy operations only when their Python dispatch is known.

use super::*;

fn atomic(ty: Type) -> bool {
    matches!(
        ty,
        Type::None
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::StrEnum(_)
            | Type::Enum(_)
    )
}

impl Checker<'_> {
    pub(super) fn shallow_copy(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let source = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([value], []) => value,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|name| name.as_str() == "x") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "copy.copy requires one argument, x",
                ));
            }
        };
        let source = self.expr(source)?;
        let ty = source.ty;
        match ty {
            ty if atomic(ty) => Ok(source),
            Type::Tuple(_)
                if self
                    .types
                    .fields(ty)
                    .is_some_and(|fields| self.types.tuple(fields) == ty) =>
            {
                Ok(source)
            }
            Type::ListInt | Type::List(_) => Ok(self.list_snapshot(source)),
            Type::Dict(_) | Type::Set(_) => Ok(Expr {
                ty,
                kind: ExprKind::Builtin(
                    if matches!(ty, Type::Dict(_)) {
                        Builtin::DictCopy
                    } else {
                        Builtin::KeySet
                    },
                    vec![source],
                ),
            }),
            _ => Err(error(
                self.source,
                call,
                "copy.copy requires a supported atomic type or container",
            )),
        }
    }

    pub(super) fn deep_copy(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let positional = call.arguments.args.as_ref();
        if positional.len() > 2 {
            return Err(error(
                self.source,
                call,
                "deepcopy requires x and an optional memo",
            ));
        }
        let mut x = positional.first();
        let mut memo = positional.get(1);
        for kw in &*call.arguments.keywords {
            let arg = match kw.arg.as_ref().map(|name| name.as_str()) {
                Some("x") => &mut x,
                Some("memo") => &mut memo,
                _ => return Err(error(self.source, kw, "unsupported deepcopy keyword")),
            };
            if arg.replace(&kw.value).is_some() {
                return Err(error(self.source, kw, "duplicate deepcopy argument"));
            }
        }
        let node = x.ok_or_else(|| error(self.source, call, "deepcopy requires x"))?;
        if memo.is_some_and(|m| !matches!(m, ast::Expr::NoneLiteral(_))) {
            return Err(error(
                self.source,
                call,
                "deepcopy requires an implicit memo or literal None",
            ));
        }
        let source = self.expr(node)?;
        if atomic(source.ty) {
            return Ok(source);
        }
        if !self.deep_copy_shape(source.ty) {
            return Err(error(
                self.source,
                node,
                "deepcopy requires a closed container graph with atomic keys and supported leaf types",
            ));
        }
        Ok(Expr {
            ty: source.ty,
            kind: ExprKind::Builtin(Builtin::DeepCopy, vec![source]),
        })
    }

    /// Values may contain mutable containers; dictionary and set keys must be
    /// unchanged by deepcopy. Never interpret a resource or user record as a container.
    fn deep_copy_shape(&self, root: Type) -> bool {
        let mut work = vec![(root, false)];
        let mut seen = HashSet::new();
        while let Some((ty, key)) = work.pop() {
            if atomic(ty) || !seen.insert((ty, key)) {
                continue;
            }
            if let Some(fields) = self.types.fields(ty) {
                if self.types.tuple(fields.clone()) != ty {
                    return false;
                }
                work.extend(fields.into_iter().map(|field| (field, key)));
            } else if let Some(payload) = self.types.payload(ty) {
                work.push((payload, key));
            } else if !key && let Some(element) = self.types.element(ty) {
                work.push((element, false));
            } else if !key && let Some((key, value)) = self.types.mapping(ty) {
                work.extend([(key, true), (value, false)]);
            } else if !key && let Some(element) = self.types.set_element(ty) {
                work.push((element, true));
            } else {
                return false;
            }
        }
        true
    }

    /// A full slice duplicates storage while acquiring each existing element.
    pub(super) fn list_snapshot(&self, list: Expr) -> Expr {
        let absent = Expr {
            ty: self.types.optional(Type::Int),
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        };
        Expr {
            ty: list.ty,
            kind: ExprKind::Builtin(
                Builtin::Slice,
                vec![list, absent.clone(), absent.clone(), absent],
            ),
        }
    }
}
