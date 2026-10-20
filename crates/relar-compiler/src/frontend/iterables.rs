//! Concrete eager consumers share ordinary iteration and own all private roots.

use super::*;

impl Checker<'_> {
    pub(super) fn list_call(
        &mut self,
        call: &ast::ExprCall,
        mut element: Option<Type>,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [source] = call.arguments.args.as_ref() else {
            if call.arguments.args.is_empty() {
                return Ok(Expr {
                    ty: self.types.list(element.unwrap_or(Type::Int)),
                    kind: ExprKind::List(Vec::new()),
                });
            }
            return Err(error(
                self.source,
                call,
                "list accepts zero or one positional iterable",
            ));
        };
        let (input, convert) = self.consumer_input(source, element)?;
        if convert
            && element.is_none_or(|ty| ty == Type::Int)
            && let Iteration::Container {
                iterable,
                items: false,
                start: None,
            } = &input
            && iterable.ty == Type::Str
        {
            // The immutable source has no callback or observable iterator
            // identity. Keep conversion eager and preserve its first error.
            return Ok(Expr {
                ty: Type::ListInt,
                kind: ExprKind::Builtin(Builtin::StringIntList, vec![iterable.clone()]),
            });
        }
        let result = self.hidden(Type::None);
        let mut body = self.expand_items(input, call, |checker, value| {
            let value = checker.consumer_value(value, convert, call)?;
            checker.expansion_insert(result, &mut element, value, false, call)
        })?;
        let ty = self.types.list(element.unwrap_or(Type::Int));
        self.locals[result] = ty;
        body.insert(
            0,
            Statement::Assign(
                result,
                Expr {
                    ty,
                    kind: ExprKind::List(Vec::new()),
                },
            ),
        );
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear: vec![result],
            },
        })
    }

    /// `map` acquires its source iterator before `start` is evaluated, but
    /// conversion is lazy. Ordinary `sum` acquires its iterator afterward.
    pub(super) fn sum_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if !(1..=2).contains(&call.arguments.args.len())
            || call.arguments.keywords.len() > 1
            || call
                .arguments
                .keywords
                .iter()
                .any(|keyword| keyword.arg.as_ref().map(|name| name.as_str()) != Some("start"))
            || (call.arguments.args.len() == 2 && !call.arguments.keywords.is_empty())
        {
            return Err(error(
                self.source,
                call,
                "sum requires an integer iterable and an optional integer start",
            ));
        }
        let (input, convert) = self.consumer_input(&call.arguments.args[0], Some(Type::Int))?;
        let start = call.arguments.args.get(1).or_else(|| {
            call.arguments
                .keywords
                .first()
                .map(|keyword| &keyword.value)
        });
        let start = match start {
            Some(start) => self.expr_expected(start, Type::Int)?,
            None => Expr {
                ty: Type::Int,
                kind: ExprKind::Int("0".into()),
            },
        };
        let result = self.hidden(Type::Int);
        let mut body = Vec::new();
        let mut clear = vec![result];
        let fixed_tuple = matches!(
            &input,
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if matches!(iterable.ty, Type::Tuple(_))
        );
        // Fixed-tuple expansion already captures its source before running
        // `start`. A second aggregate capture would hide its projection-only
        // use from tuple scalar replacement, forcing a temporary allocation.
        let input = if fixed_tuple {
            input
        } else {
            self.capture_consumer_input(input, &mut body, &mut clear)
        };
        let initialize = Statement::Assign(result, start);
        let after = if convert || fixed_tuple {
            vec![initialize]
        } else {
            body.push(initialize);
            Vec::new()
        };
        body.extend(
            self.expand_items_after(input, call, after, |checker, value| {
                let value = checker.consumer_value(value, convert, call)?;
                checker.expect(Type::Int, value.ty, call)?;
                Ok(vec![Statement::Assign(
                    result,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Binary(
                            Binary::Add,
                            Box::new(checker.local(result)),
                            Box::new(value),
                        ),
                    },
                )])
            })?,
        );
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }

    /// The sealed converter has no user callback or escaping iterator object.
    /// Its input is evaluated now; each conversion stays inside the consumer.
    fn consumer_input(
        &mut self,
        expression: &ast::Expr,
        element: Option<Type>,
    ) -> Result<(Iteration, bool), String> {
        if let ast::Expr::Call(call) = expression
            && matches!(&*call.func, ast::Expr::Name(name) if name.id == "map")
        {
            if !call.arguments.keywords.is_empty()
                || !matches!(
                    call.arguments.args.as_ref(),
                    [ast::Expr::Name(name), _] if name.id == "int"
                )
            {
                return Err(error(
                    self.source,
                    call,
                    "map requires the builtin int converter and one concrete iterable",
                ));
            }
            return Ok((self.expansion_input(&call.arguments.args[1], None)?, true));
        }
        Ok((self.expansion_input(expression, element)?, false))
    }

    fn consumer_value(
        &self,
        value: Expr,
        convert: bool,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if !convert {
            return Ok(value);
        }
        if !matches!(value.ty, Type::Int | Type::Float | Type::Bool | Type::Str) {
            return Err(error(
                self.source,
                node,
                "map(int, iterable) requires int, float, bool, or str elements",
            ));
        }
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Int, vec![value]),
        })
    }

    /// Root source arguments before later consumer arguments can rebind them.
    /// Constructing a range rejects a zero step before `sum` evaluates `start`.
    fn capture_consumer_input(
        &mut self,
        input: Iteration,
        body: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Iteration {
        let mut capture = |value: Expr| {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            self.local(slot)
        };
        match input {
            Iteration::Range(args) => {
                let args = args.into_iter().map(&mut capture).collect::<Vec<_>>();
                if let [_, _, step] = args.as_slice() {
                    body.push(Statement::If(
                        Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Compare(
                                vec![
                                    step.clone(),
                                    Expr {
                                        ty: Type::Int,
                                        kind: ExprKind::Int("0".into()),
                                    },
                                ],
                                vec![Compare::Equal],
                            ),
                        },
                        vec![Statement::Raise(Some(Expr {
                            ty: Type::Exception,
                            kind: ExprKind::Exception {
                                kind: exception_kind("ValueError").unwrap(),
                                message: Some(Box::new(Expr {
                                    ty: Type::Str,
                                    kind: ExprKind::Str("range() arg 3 must not be zero".into()),
                                })),
                            },
                        }))],
                        Vec::new(),
                    ));
                }
                Iteration::Range(args)
            }
            Iteration::Container {
                iterable,
                items,
                start,
            } => Iteration::Container {
                iterable: capture(iterable),
                items,
                start: start.map(capture),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn mapped_string_digits_use_the_bulk_list_path() {
        let llvm = compile(
            "def digits(text: str) -> list[int]:\n    return list(map(int, text))\n",
            &CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(llvm.matches("call ptr @relar_str_int_list(").count(), 1);
        assert!(!llvm.contains("call ptr @relar_str_iter("));
        assert!(!llvm.contains("call i64 @relar_int_parse("));
    }

    #[test]
    fn mapped_strings_keep_requested_optional_element_storage() {
        let llvm = compile(
            "def digits(text: str) -> list[int | None]:\n    return list(map(int, text))\n",
            &CompileOptions::default(),
        )
        .unwrap();
        assert!(!llvm.contains("call ptr @relar_str_int_list("));
        assert!(llvm.contains("call ptr @relar_str_iter("));
    }
}
