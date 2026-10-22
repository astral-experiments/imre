//! Nominal member identity and the finite set of values accepted by a match.

use super::*;

impl Checker<'_> {
    pub(super) fn enum_member(&self, class: usize, name: &str) -> Option<Expr> {
        let members = self.classes.declarations.definitions[class]
            .enum_members
            .as_ref()?;
        let &member = members.names.get(name)?;
        Some(Expr {
            ty: Type::StrEnum(class),
            kind: ExprKind::Builtin(Builtin::StrEnumMember { class, member }, Vec::new()),
        })
    }

    pub(super) fn qualified_member(&self, value: &ast::Expr) -> Option<Expr> {
        let ast::Expr::Attribute(attr) = value else {
            return None;
        };
        self.enum_member(self.class_reference(&attr.value)?, attr.attr.as_str())
    }

    pub(super) fn enum_text(value: Expr) -> Expr {
        if matches!(value.ty, Type::StrEnum(_)) {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::StrEnumValue, vec![value]),
            }
        } else {
            value
        }
    }

    /// Only use where the operation observes string contents, never nominal identity.
    pub(super) fn string_value(value: Expr) -> Expr {
        if matches!(value.ty, Type::StrLiteral(_)) {
            literals::view(value, Type::Str)
        } else {
            Self::enum_text(value)
        }
    }

    pub(super) fn enum_identity(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        op: ast::CmpOp,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let left = self.expr(left)?;
        let right = self.expr(right)?;
        if ![left.ty, right.ty]
            .into_iter()
            .all(|ty| matches!(self.types.payload(ty).unwrap_or(ty), Type::StrEnum(_)))
        {
            return Err(error(
                self.source,
                node,
                "identity comparisons require None or nominal StrEnum members",
            ));
        }
        Ok(Self::identity_result(
            vec![left, right],
            op == ast::CmpOp::IsNot,
        ))
    }

    pub(super) fn identity_result(values: Vec<Expr>, negate: bool) -> Expr {
        let identity = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::StrEnumIdentity, values),
        };
        if negate {
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Not(Box::new(identity)),
            }
        } else {
            identity
        }
    }

    /// Guards can fail. Only unconditional cases count toward a finite match.
    pub(super) fn enum_exhaustive(&self, ty: Type, cases: &[ast::MatchCase]) -> bool {
        let payload = self.types.payload(ty);
        let Type::StrEnum(class) = payload.unwrap_or(ty) else {
            return false;
        };
        let members = self.classes.declarations.definitions[class]
            .enum_members
            .as_ref()
            .unwrap();
        let mut remaining = vec![true; members.values.len()];
        let mut none = payload.is_some();
        let mut patterns: Vec<_> = cases
            .iter()
            .filter(|case| case.guard.is_none())
            .map(|case| &case.pattern)
            .collect();
        while let Some(pattern) = patterns.pop() {
            match pattern {
                ast::Pattern::MatchValue(value) => {
                    if let Some(Expr {
                        kind:
                            ExprKind::Builtin(
                                Builtin::StrEnumMember {
                                    class: owner,
                                    member,
                                },
                                _,
                            ),
                        ..
                    }) = self.qualified_member(&value.value)
                        && class == owner
                    {
                        remaining[member] = false;
                    }
                }
                ast::Pattern::MatchSingleton(value) if value.value == ast::Singleton::None => {
                    none = false
                }
                ast::Pattern::MatchAs(value) if let Some(inner) = &value.pattern => {
                    patterns.push(inner)
                }
                ast::Pattern::MatchOr(or) => patterns.extend(&or.patterns),
                _ => {}
            }
        }
        !none && !remaining.contains(&true)
    }
}

#[cfg(test)]
mod tests {
    const PREFIX: &str = "from enum import StrEnum, auto\nclass State(StrEnum):\n    OPEN = auto()\n    ALIAS = OPEN\n    SAME = 'open'\n    CLOSED = 'off'\n";

    fn check(body: &str) -> Result<(), String> {
        crate::check(&format!("{PREFIX}{body}"))
    }

    #[test]
    fn exhaustive_unguarded_members_and_none_close_only_the_actual_domain() {
        check("def result(value: State | None) -> int:\n    match value:\n        case None:\n            return 0\n        case State.ALIAS | State.CLOSED:\n            return 1\n").unwrap();
        check("def narrowed(value: State | None) -> int:\n    if value is None:\n        return 0\n    match value:\n        case State.OPEN | State.CLOSED:\n            return 1\n").unwrap();
        for body in [
            "def missing(value: State) -> int:\n    match value:\n        case State.OPEN | State.ALIAS:\n            return 1\n",
            "def guarded(value: State, ready: bool) -> int:\n    match value:\n        case State.OPEN if ready:\n            return 1\n        case State.CLOSED:\n            return 2\n",
            "def nullable(value: State | None) -> int:\n    match value:\n        case State.OPEN | State.CLOSED:\n            return 2\n",
        ] {
            assert!(check(body).is_err(), "{body}");
        }
    }

    #[test]
    fn nominal_stores_preserve_identity_and_require_explicit_string_projection() {
        let echo = "def echo(value: str) -> str:\n    return repr(value)\n";
        let rejected = check(&format!("{echo}print(echo(State.OPEN))\n")).unwrap_err();
        assert!(
            rejected.contains("str(member) or member.value"),
            "{rejected}"
        );
        check(&format!(
            "{echo}print(echo(State.OPEN.value))\nprint(echo(str(State.CLOSED)))\n"
        ))
        .unwrap();
        for source in [
            "def wrong() -> State:\n    return 'open'\n",
            "class Other(StrEnum):\n    OPEN = 'open'\ndef wrong() -> State:\n    return Other.OPEN\n",
            "def mutate(value: State) -> None:\n    value.name = 'different'\n",
            "def wrong(value: State) -> bool:\n    return value is 'open'\n",
            "def bad(value: str) -> State:\n    return State(value)\n",
        ] {
            assert!(check(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_dynamic_class_definitions_and_shadowed_auto() {
        for declaration in [
            "class Invalid(StrEnum):\n    ITEM = 1\n",
            "class Invalid(StrEnum):\n    ITEM = 'a' + 'b'\n",
            "class Invalid(StrEnum):\n    ITEM = auto(1)\n",
            "class Invalid(StrEnum):\n    mro = 'reserved'\n",
            "class Invalid(StrEnum):\n    ITEM = auto()\n    ITEM = 'other'\n",
            "class Invalid(StrEnum):\n    auto = 'member'\n    ITEM = auto()\n",
            "class Invalid(StrEnum):\n    ITEM = 'x'\n    def __str__(self) -> str:\n        return 'y'\n",
            "class Invalid(StrEnum):\n    pass\n",
        ] {
            assert!(
                crate::check(&format!("from enum import StrEnum, auto\n{declaration}")).is_err(),
                "{declaration}"
            );
        }
    }
}
