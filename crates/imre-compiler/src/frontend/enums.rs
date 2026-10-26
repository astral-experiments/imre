//! Nominal member identity and the finite set of values accepted by a match.

use super::*;

impl Checker<'_> {
    pub(super) fn assert_never(
        &mut self,
        call: &ast::ExprCall,
        exhausted_pair: bool,
    ) -> Result<Statement, String> {
        let [arg] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "assert_never requires one exhausted argument",
            ));
        };
        let val = self.expr(arg)?;
        let enum_local = matches!(arg, ast::Expr::Name(name) if self.names.contains_key(name.id.as_str()))
            && matches!(
                self.types.payload(val.ty).unwrap_or(val.ty),
                Type::StrEnum(_)
            );
        if !call.arguments.keywords.is_empty() || !(enum_local || exhausted_pair) {
            return Err(error(
                self.source,
                call,
                "assert_never requires a StrEnum local or an exhausted boolean pair",
            ));
        }
        // A finally suite also receives pending returns, breaks and exceptions.
        // ty's ordinary fallthrough narrowing does not cover all those entries.
        if self.in_finally {
            return Err(error(
                self.source,
                call,
                "assert_never inside finally is unsupported",
            ));
        }
        if !exhausted_pair && !self.graph.session.is_never(self.module, arg) {
            return Err(error(
                self.source,
                arg,
                "assert_never requires an exhausted StrEnum domain",
            ));
        }
        // Keep the defensive Python exception even when the checker proves the
        // argument uninhabited. typing truncates repr at 100 Unicode code points.
        let repr = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Repr, vec![val]),
        };
        let slot = self.hidden(Type::Str);
        let limit = pipelines::integer(100);
        let index_type = self.types.optional(Type::Int);
        let absent = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            index_type,
            call,
        )?;
        let stop = self.coerce(limit.clone(), index_type, call)?;
        let len = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![self.local(slot)]),
        };
        let prefix = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::Slice,
                vec![self.local(slot), absent.clone(), stop, absent],
            ),
        };
        let clipped = Expr {
            ty: Type::Str,
            kind: ExprKind::Conditional(
                Box::new(pipelines::compare(Compare::Greater, len, limit)),
                Box::new(unpacking::concat(prefix, unpacking::string("..."))),
                Box::new(self.local(slot)),
            ),
        };
        let message = Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, repr)],
                value: Box::new(unpacking::concat(
                    unpacking::string("Expected code to be unreachable, but got: "),
                    clipped,
                )),
                clear: vec![slot],
            },
        };
        Ok(Statement::Raise(Some(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("AssertionError").unwrap(),
                message: Some(Box::new(message)),
            },
        })))
    }

    pub(super) fn enum_member(&self, class: usize, name: &str) -> Option<Expr> {
        let members = self.classes.declarations.definitions[class]
            .enum_members
            .as_ref()?;
        let &member = members.names.get(name)?;
        Some(Expr {
            ty: self.classes.declarations.instance(class),
            kind: ExprKind::Builtin(Builtin::StrEnumMember { class, member }, Vec::new()),
        })
    }

    /// A direct iterator can read the sealed enum without exposing a mutable mapping.
    /// Enum iteration visits distinct values; __members__.values() also visits aliases.
    pub(super) fn enum_iteration(&self, expression: &ast::Expr) -> Option<Iteration> {
        let (class, aliases) = if let ast::Expr::Call(call) = expression {
            let ast::Expr::Attribute(values) = &*call.func else {
                return None;
            };
            let ast::Expr::Attribute(members) = &*values.value else {
                return None;
            };
            if !call.arguments.is_empty()
                || values.attr.as_str() != "values"
                || members.attr.as_str() != "__members__"
            {
                return None;
            }
            (self.class_reference(&members.value)?, true)
        } else {
            (self.class_reference(expression)?, false)
        };
        let members = self.classes.declarations.definitions[class]
            .enum_members
            .as_ref()?;
        let ty = self.classes.declarations.instance(class);
        let member = |member| Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::StrEnumMember { class, member }, vec![]),
        };
        let values = if aliases {
            members
                .declared
                .iter()
                .map(|name| member(members.names[name]))
                .collect()
        } else {
            (0..members.values.len()).map(member).collect()
        };
        Some(Iteration::Container {
            iterable: Expr {
                ty: self.types.list(ty),
                kind: ExprKind::List(values),
            },
            items: false,
            start: None,
        })
    }

    /// Only this subscript sees the temporary mapping. Aliases retain their canonical tags.
    pub(super) fn enum_name_lookup(
        &mut self,
        sub: &ast::ExprSubscript,
    ) -> Result<Option<Expr>, String> {
        let Some(class) = self.class_reference(&sub.value) else {
            return Ok(None);
        };
        let Some(members) = &self.classes.declarations.definitions[class].enum_members else {
            return Ok(None);
        };
        let ty = self.classes.declarations.instance(class);
        if let ast::Expr::StringLiteral(name) = &*sub.slice
            && let Some(member) = self.enum_member(class, name.value.to_str())
        {
            return Ok(Some(member));
        }
        let entries = members
            .declared
            .iter()
            .map(|name| {
                (
                    unpacking::string(name),
                    self.enum_member(class, name).unwrap(),
                )
            })
            .collect();
        let mapping = Expr {
            ty: self.types.dict(Type::Str, ty),
            kind: ExprKind::Dict {
                key: Type::Str,
                value: ty,
                entries,
            },
        };
        let key = self.expr_expected(&sub.slice, Type::Str)?;
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::DictGet, vec![mapping, key]),
        }))
    }

    pub(super) fn qualified_member(&self, value: &ast::Expr) -> Option<Expr> {
        let ast::Expr::Attribute(attr) = value else {
            return None;
        };
        self.enum_member(self.class_reference(&attr.value)?, attr.attr.as_str())
    }

    pub(super) fn enum_integer(value: Expr) -> Expr {
        if matches!(value.ty, Type::IntEnum(_)) {
            Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::IntEnumValue, vec![value]),
            }
        } else {
            value
        }
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

    pub(super) fn nominal_identity(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        op: ast::CmpOp,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if let Some(value) = self.context_missing_test(left, right, op == ast::CmpOp::IsNot)? {
            return Ok(value);
        }
        let (left, right) = match (self.class_reference(left), self.class_reference(right)) {
            (Some(class), None) | (None, Some(class)) => {
                let class_first = self.class_reference(left).is_some();
                let (literal, other) = if class_first {
                    (left, right)
                } else {
                    (right, left)
                };
                let other = self.expr(other)?;
                let value = if let Some(base) = self.types.constructor_base(other.ty) {
                    self.record_constructor_value(class, base, other.ty, literal)?
                } else {
                    self.expr(literal)?
                };
                if class_first {
                    (value, other)
                } else {
                    (other, value)
                }
            }
            _ => (self.expr(left)?, self.expr(right)?),
        };
        if left.ty == Type::SourceModule && right.ty == Type::SourceModule {
            return Ok(Self::identity_result(
                vec![left, right],
                op == ast::CmpOp::IsNot,
            ));
        }
        let operands = [&left, &right];
        let tuple = |e: &Expr| {
            matches!(e.ty, Type::Tuple(_))
                || matches!(
                    e.kind,
                    ExprKind::Builtin(
                        Builtin::ExceptionArgs
                            | Builtin::ImportErrorField(0)
                            | Builtin::ExceptionTupleView,
                        _
                    )
                )
        };
        if operands.iter().any(|e| e.ty == Type::ObjectValue)
            && operands
                .iter()
                .all(|e| matches!(e.ty, Type::ObjectValue | Type::Tuple(_)))
            && operands.iter().any(|e| tuple(e))
        {
            // One proven tuple rules out unsupported scalar identity, even when
            // the other operand is an alias stored through a general object view.
            let view = |e: Expr| {
                if e.ty == Type::ObjectValue {
                    e
                } else {
                    Expr {
                        ty: Type::ObjectValue,
                        kind: ExprKind::Builtin(Builtin::ExceptionTupleView, vec![e]),
                    }
                }
            };
            let same = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(
                    Builtin::ExceptionTupleIdentity,
                    vec![view(left), view(right)],
                ),
            };
            return Ok(if op == ast::CmpOp::IsNot {
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Not(Box::new(same)),
                }
            } else {
                same
            });
        }
        let identity = match (left.ty, right.ty) {
            (Type::ObjectValue, Type::ListInt | Type::List(_)) => {
                Some(Builtin::ExceptionFieldIdentity { reversed: false })
            }
            (Type::ListInt | Type::List(_), Type::ObjectValue) => {
                Some(Builtin::ExceptionFieldIdentity { reversed: true })
            }
            (Type::ObjectValue, Type::Class(class)) if self.boxed_dataclass(class) => {
                Some(Builtin::ObjectRecordIdentity {
                    class,
                    reversed: false,
                })
            }
            (Type::Class(class), Type::ObjectValue) if self.boxed_dataclass(class) => {
                Some(Builtin::ObjectRecordIdentity {
                    class,
                    reversed: true,
                })
            }
            _ => None,
        };
        if let Some(identity) = identity {
            let same = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(identity, vec![left, right]),
            };
            return Ok(if op == ast::CmpOp::IsNot {
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Not(Box::new(same)),
                }
            } else {
                same
            });
        }
        if [left.ty, right.ty].iter().all(|t| {
            matches!(
                t,
                Type::FunctionRef | Type::Callable(_) | Type::OverloadStub
            )
        }) {
            return Ok(Self::identity_result(
                vec![left, right],
                op == ast::CmpOp::IsNot,
            ));
        }
        // A comparison with a bool singleton only observes whether the other
        // operand is that same singleton; integer object identity is irrelevant.
        if let Some(union) = [left.ty, right.ty]
            .into_iter()
            .find(|&ty| self.types.is_bool_or_int(ty))
            && (left.ty == Type::Bool || right.ty == Type::Bool)
        {
            return Ok(Self::identity_result(
                vec![
                    self.coerce(left, union, node)?,
                    self.coerce(right, union, node)?,
                ],
                op == ast::CmpOp::IsNot,
            ));
        }
        // Python's empty built-in tuple is a singleton. Each operand still
        // executes exactly once, even when it raises before returning the tuple.
        if [left.ty, right.ty] == [self.types.tuple(Vec::new()); 2] {
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(left), Statement::Expr(right)],
                    value: Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(op == ast::CmpOp::Is),
                    }),
                    clear: Vec::new(),
                },
            });
        }
        // Python has exactly two bool singletons. Restrict this to exact bools:
        // False and the integer zero can compare equal without being identical.
        if left.ty == Type::Bool && right.ty == Type::Bool {
            return Ok(Self::identity_result(
                vec![left, right],
                op == ast::CmpOp::IsNot,
            ));
        }
        if matches!(
            (left.ty, right.ty),
            (Type::IntEnum(_), Type::Int) | (Type::Int, Type::IntEnum(_))
        ) {
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(left), Statement::Expr(right)],
                    value: Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(op == ast::CmpOp::IsNot),
                    }),
                    clear: vec![],
                },
            });
        }
        let types = [left.ty, right.ty].map(|ty| self.types.payload(ty).unwrap_or(ty));
        if matches!((left.ty, right.ty), (Type::Callable(_), Type::Callable(_)))
            && [left.ty, right.ty].iter().all(|&ty| {
                let signature = self.types.wrapped_signature(ty).unwrap_or(ty);
                self.types
                    .signature(signature)
                    .is_some_and(|(params, result)| {
                        matches!(params.as_slice(), [arg] if self.types.is_len_argument(*arg))
                            && result == Type::Int
                    })
            })
        {
            return Ok(Self::identity_result(
                vec![left, right],
                op == ast::CmpOp::IsNot,
            ));
        }
        if types.iter().any(|&ty| self.types.is_value_struct(ty)) {
            return Err(error(
                self.source,
                node,
                "imre.struct values have no observable identity; compare them with ==",
            ));
        }
        if let Some(&ty) = types.iter().find(|t| matches!(t, Type::AnnotationValue(_)))
            && types.iter().all(|t| {
                matches!(
                    t,
                    Type::AnnotationValue(_)
                        | Type::ForwardRef(_)
                        | Type::BuiltinClass
                        | Type::None
                        | Type::Str
                )
            })
        {
            let expr = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(
                    Builtin::AnnotationIs,
                    vec![
                        self.box_annotation(left, ty),
                        self.box_annotation(right, ty),
                    ],
                ),
            };
            return Ok(if op == ast::CmpOp::IsNot {
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Not(Box::new(expr)),
                }
            } else {
                expr
            });
        }
        if let Some((union, members)) = types
            .iter()
            .find_map(|&ty| self.types.members(ty).map(|members| (ty, members)))
            && members.iter().all(|ty| {
                matches!(
                    ty,
                    Type::Bool | Type::LogRecord | Type::Str | Type::Bytes | Type::Path
                )
            })
            && types.iter().all(|ty| *ty == union || members.contains(ty))
        {
            // A member's pointer and its tagged union word can differ. Put both
            // operands in the same representation while preserving None and bool.
            let target = if [left.ty, right.ty] == types {
                union
            } else {
                self.types.optional(union)
            };
            return Ok(Self::identity_result(
                vec![
                    self.coerce(left, target, node)?,
                    self.coerce(right, target, node)?,
                ],
                op == ast::CmpOp::IsNot,
            ));
        }
        let wrapper_attributes = matches!((left.ty, right.ty), (Type::Tuple(_), Type::Tuple(_)))
            && [&left, &right].iter().any(|value| {
                matches!(
                    value.kind,
                    ExprKind::Builtin(Builtin::WrapperAttributes(_), _)
                ) || self.is_partial_args(value)
            });
        // Namespace owners and their backing dictionaries are distinct rooted
        // objects. Optional storage preserves their pointer words and None tag.
        let namespace = types
            .iter()
            .any(|ty| matches!(ty, Type::SimpleNamespace(_)))
            && types
                .iter()
                .all(|ty| matches!(ty, Type::SimpleNamespace(_) | Type::Dict(_) | Type::None));
        if wrapper_attributes || namespace || types.iter().all(|ty| matches!(ty, Type::CmpKey(..)))
        {
            return Ok(Self::identity_result(
                vec![left, right],
                op == ast::CmpOp::IsNot,
            ));
        }
        if types == [Type::BuiltinClass; 2] {
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Compare(
                    vec![left, right],
                    vec![if op == ast::CmpOp::Is {
                        Compare::Equal
                    } else {
                        Compare::NotEqual
                    }],
                ),
            });
        }
        if types.iter().any(|ty| {
            matches!(
                ty,
                Type::PlainObject | Type::StrDescriptor(_) | Type::Placeholder(_)
            )
        }) && (types == [left.ty, right.ty] || types[0] == types[1])
        {
            if types[0] == types[1] {
                return Ok(Self::identity_result(
                    vec![left, right],
                    op == ast::CmpOp::IsNot,
                ));
            }
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(left), Statement::Expr(right)],
                    value: Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(op == ast::CmpOp::IsNot),
                    }),
                    clear: vec![],
                },
            });
        }
        if types.contains(&Type::StringIterable) && types == [left.ty, right.ty] {
            // Both operands must reach identity with the same pointer tag.
            let left = self.coerce(left, Type::StringIterable, node)?;
            let right = self.coerce(right, Type::StringIterable, node)?;
            return Ok(Self::identity_result(
                vec![left, right],
                op == ast::CmpOp::IsNot,
            ));
        }
        if let (Type::Class(a), Type::Class(b)) = (left.ty, right.ty)
            && self.native_mock(a)
            && self.native_mock(b)
        {
            return Ok(Self::identity_result(
                vec![self.mock_node(left, a), self.mock_node(right, b)],
                op == ast::CmpOp::IsNot,
            ));
        }
        if !(types
            .iter()
            .all(|ty| matches!(ty, Type::IntEnum(_) | Type::StrEnum(_) | Type::Enum(_)))
            || types.iter().all(|ty| {
                matches!(
                    ty,
                    Type::Exception
                        | Type::ImportException
                        | Type::TimeoutExpired
                        | Type::NamedException(_)
                        | Type::None
                )
            })
            || types == [Type::NativeModule; 2]
            || types == [Type::ModuleNamespace; 2]
            || types
                .iter()
                .all(|ty| matches!(ty, Type::ForwardRef(_) | Type::Callable(_)))
            || types
                .iter()
                .all(|ty| matches!(ty, Type::Str | Type::StrLiteral(_) | Type::Bytes))
            || types.iter().all(|ty| {
                matches!(
                    ty,
                    Type::ListInt
                        | Type::List(_)
                        | Type::Dict(_)
                        | Type::ChainMap(_)
                        | Type::Set(_)
                        | Type::FrozenStrings
                        | Type::Deque(_)
                        | Type::Class(_)
                        | Type::ModuleSpec(_)
                )
            })
            || types == [Type::Str; 2]
            || types.iter().all(|ty| {
                matches!(
                    ty,
                    Type::Context | Type::ContextVar(_) | Type::ContextToken(_)
                )
            })
            || (types.contains(&Type::ContextMissing)
                && types
                    .iter()
                    .all(|ty| matches!(ty, Type::ContextMissing | Type::ObjectValue)))
            || types == [Type::LogRecord; 2]
            || types.iter().all(|ty| matches!(ty, Type::Cursor(_)))
            || types == [Type::Path; 2]
            || (types.iter().all(|ty| matches!(ty, Type::Tuple(_)))
                && types
                    .iter()
                    .any(|ty| self.types.tuple_metadata(*ty).is_some()))
            || types == [Type::SysImplementation; 2]
            || types == [Type::Logger; 2]
            || types
                .iter()
                .all(|ty| matches!(ty, Type::Popen(..) | Type::ActivePopen))
            || types == [Type::LogManager; 2]
            || types == [Type::StringIO; 2]
            || types == [Type::HandlerLock; 2]
            || types == [Type::TimedFileHandler; 2]
            || types == [Type::ClockTime; 2]
            || types.iter().all(|&t| {
                matches!(t, Type::Calendar(_))
                    || self
                        .types
                        .payload(t)
                        .is_some_and(|p| matches!(p, Type::Calendar(_)))
            })
            || types == [Type::PathInfo; 2]
            || types == [Type::CodecInfo; 2]
            || types == [Type::JsonDecoder; 2]
            || types == [Type::Poll; 2]
            || types == [Type::Socket; 2]
            || types == [Type::ProcessSelectorClass; 2]
            || types == [Type::CodecWriterClass; 2]
            || types.iter().all(|ty| {
                matches!(
                    ty,
                    Type::TextOutput | Type::CodecWriter | Type::TextIO | Type::StringIO
                )
            })
            || types == [Type::BinaryIO; 2]
            || types == [Type::Sha256; 2])
        {
            return Err(error(
                self.source,
                node,
                "identity comparisons require supported stable object identities",
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
            kind: ExprKind::Builtin(Builtin::NominalIdentity, values),
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
        let (Type::IntEnum(class) | Type::StrEnum(class) | Type::Enum(class)) =
            payload.unwrap_or(ty)
        else {
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
    fn assert_never_requires_a_semantic_proof_even_when_ty_diagnostics_are_ignored() {
        let imported = "from typing import assert_never as exhausted\n";
        let skipped = "  # ty: ignore[type-assertion-failure]\n";
        for body in [
            "def f(v: State) -> None:\n    if v == State.OPEN or v == State.ALIAS:\n        return\n    exhausted(v)",
            "def f(v: State | None) -> None:\n    if v == State.OPEN or v == State.CLOSED:\n        return\n    exhausted(v)",
            "def f(v: State, set_open: bool) -> None:\n    if v == State.OPEN:\n        return\n    if set_open:\n        v = State.OPEN\n    if v == State.CLOSED:\n        return\n    exhausted(v)",
            "def f(v: State, next_state: State, steps: list[int]) -> None:\n    if v == State.OPEN:\n        return\n    for i in steps:\n        if v == State.CLOSED:\n            v = next_state\n            continue\n        exhausted(v)",
        ] {
            let err = crate::check(&format!("{imported}{PREFIX}{body}{skipped}")).unwrap_err();
            assert!(
                err.contains("assert_never requires an exhausted StrEnum domain"),
                "{err}"
            );
        }
        let body = "def f(v: State) -> None:\n    try:\n        if v == State.OPEN:\n            return\n    finally:\n        if v == State.CLOSED:\n            return\n        exhausted(v)\n";
        let two_members = "from enum import StrEnum\nclass State(StrEnum):\n    OPEN = 'open'\n    CLOSED = 'closed'\n";
        let err = crate::check(&format!("{imported}{two_members}{body}")).unwrap_err();
        assert!(
            err.contains("assert_never inside finally is unsupported"),
            "{err}"
        );
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
