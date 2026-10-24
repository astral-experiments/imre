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
        let source = &call.arguments.args[0];
        let mut method_input = None;
        if call.arguments.args.len() == 1
            && call.arguments.keywords.is_empty()
            && let ast::Expr::Call(view) = source
            && self.resolve(&view.func).is_none()
            && view.arguments.args.is_empty()
            && view.arguments.keywords.is_empty()
            && let ast::Expr::Attribute(method) = &*view.func
            && method.attr.as_str() == "values"
            && self.class_reference(&method.value).is_none()
        {
            let receiver = self.expr(&method.value)?;
            if self.types.is_counter(receiver.ty) {
                return Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::CounterTotal, vec![receiver]),
                });
            }
            // A first-party values method still produces its usual iterable.
            // Use the already checked receiver, including any walrus bindings.
            let (Type::Class(class) | Type::StrEnum(class)) = receiver.ty else {
                return Err(error(
                    self.source,
                    view,
                    "values() in sum requires a Counter",
                ));
            };
            let iterable = self.class_method_call(view, method, class, Some(receiver))?;
            method_input = Some(Iteration::Container {
                iterable,
                items: false,
                start: None,
            });
        }
        if matches!(source, ast::Expr::ListComp(_)) {
            let subtotal = self.sum_comprehension(source)?;
            let start = self.sum_start(call)?;
            return Ok(add_start(subtotal, start));
        }
        let generator = matches!(source, ast::Expr::Generator(_));
        let mut zip_sum = None;
        let (input, convert) = if let Some(input) = method_input {
            (input, false)
        } else if let ast::Expr::Generator(generator) = source
            && self.zip_sum_rhs == Some(call.range())
        {
            (
                Iteration::Prepared(self.generator_input(generator, None, Some(&mut zip_sum))?),
                false,
            )
        } else {
            self.consumer_input(source, (!generator).then_some(Type::Int))?
        };
        let start = self.sum_start(call)?;
        if !convert
            && let Iteration::Container {
                iterable,
                items: false,
                start: None,
            } = &input
            && iterable.ty == Type::ListInt
        {
            if let ExprKind::Builtin(Builtin::Slice, arguments) = &iterable.kind {
                // Capture the slice's numeric snapshot before evaluating
                // `start`, which may mutate or replace the original list.
                // Exact integer addition has no observable intermediate work.
                let subtotal = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::ListSliceSum, arguments.clone()),
                };
                return Ok(add_start(subtotal, start));
            }
            // Evaluate both arguments before borrowing the list: `start` may
            // mutate the source through an alias. Exact integer addition has
            // no callbacks or recoverable errors during the reduction.
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::ListSum, vec![iterable.clone(), start]),
            });
        }
        let result = self.hidden(Type::Int);
        if let Some(sum) = zip_sum {
            self.zip_sums.insert(result, sum);
        }
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
                let as_int = convert || (generator && value.ty == Type::Bool);
                let value = checker.consumer_value(value, as_int, call)?;
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

    /// Checks the optional starting value after checking the eager first argument.
    fn sum_start(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let start = call.arguments.args.get(1).or_else(|| {
            call.arguments
                .keywords
                .first()
                .map(|keyword| &keyword.value)
        });
        match start {
            Some(start) => self.expr_expected(start, Type::Int),
            None => Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Int("0".into()),
            }),
        }
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
            && matches!(call.arguments.args.as_ref(), [ast::Expr::Name(name), _] if name.id == "int")
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

    pub(super) fn consumer_value(
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
    pub(super) fn capture_consumer_input(
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
            Iteration::Prepared(mut input) => {
                body.append(&mut input.evaluate);
                clear.extend(&input.clear);
                Iteration::Prepared(input)
            }
        }
    }
}

/// Eager producers finish before `start` is evaluated. Exact integer addition
/// allows the producer's subtotal to replace a private materialized container.
fn add_start(subtotal: Expr, start: Expr) -> Expr {
    if matches!(&start.kind, ExprKind::Int(value) if value == "0") {
        subtotal
    } else {
        Expr {
            ty: Type::Int,
            kind: ExprKind::Binary(Binary::Add, Box::new(subtotal), Box::new(start)),
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
        assert_eq!(llvm.matches("call ptr @imre_str_int_list(").count(), 1);
        assert!(!llvm.contains("call ptr @imre_str_iter("));
        assert!(!llvm.contains("call i64 @imre_int_parse("));
    }

    #[test]
    fn mapped_strings_keep_requested_optional_element_storage() {
        let llvm = compile(
            "def digits(text: str) -> list[int | None]:\n    return list(map(int, text))\n",
            &CompileOptions::default(),
        )
        .unwrap();
        assert!(!llvm.contains("call ptr @imre_str_int_list("));
        assert!(llvm.contains("call ptr @imre_str_iter("));
    }
}
