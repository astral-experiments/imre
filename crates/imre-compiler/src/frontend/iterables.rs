//! Concrete eager consumers share ordinary iteration and own all private roots.

use super::*;

impl Checker<'_> {
    pub(super) fn list_call(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
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
        let call_input = source.as_call_expr();
        let binding = call_input.and_then(|call| self.resolve(&call.func));
        if let Some(view) = call_input
            && let ast::Expr::Attribute(method) = &*view.func
            && method.attr.as_str() == "keys"
            && binding.is_none()
        {
            // The same non-escaping view as a key union, exhausted immediately.
            let (value, _) = self.union_operand(source)?;
            let input = self.iteration_value(value, false, source)?;
            return self.collect_list(input, false, element, call);
        }
        if let Some(scan) = call_input
            && self.is_accumulate(scan, binding)
        {
            let args = self.accumulate_args(scan)?;
            if let Some(expected) = element {
                self.expect(expected, args.element, source)
                    .map_err(|_| self.scoped_builtin_error("itertools.accumulate", scan))?;
            }
            if args.source.ty == Type::ListInt && args.function.ty == Type::None {
                let mut operands = vec![args.source];
                operands.extend(args.initial);
                return Ok(args.scope.finish(Expr {
                    ty: Type::ListInt,
                    kind: ExprKind::Builtin(Builtin::ListAccumulate, operands),
                }));
            }
            let input = self.prepare_accumulate(scan, args)?;
            return self.collect_list(Iteration::Prepared(input), false, element, call);
        }
        if element.is_none_or(|ty| ty == Type::Int)
            && let ast::Expr::Name(name) = source
            && let Some(&slot) = self.names.get(name.id.as_str())
            && self.types.cursor_element(self.locals[slot]) == Some(Type::Int)
        {
            let cursor = self.expr(source)?;
            return Ok(Expr {
                ty: Type::ListInt,
                kind: ExprKind::Builtin(Builtin::CollectIntCursor, vec![cursor]),
            });
        }
        if let Some(call) = call_input
            && binding == Some(Binding::Intrinsic(Intrinsic::Ndiff))
        {
            let diff = self.materialize_ndiff(call)?;
            if let Some(element) = element {
                self.expect(element, Type::Str, source)?;
            }
            return Ok(diff);
        }
        if let ast::Expr::Generator(generator) = source
            && let Some(filtered) = self.string_list_filter(
                &imre_ast::comprehension::Plan::generator(generator),
                element,
            )?
        {
            return Ok(filtered);
        }
        let (input, convert) = self.consumer_input(source, element)?;
        match input {
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if !convert
                && self.types.element(iterable.ty) == Some(Type::Str)
                && element.is_none_or(|ty| ty == Type::Str) =>
            {
                Ok(Expr {
                    ty: iterable.ty,
                    kind: ExprKind::Builtin(Builtin::StringListCopy, vec![iterable]),
                })
            }
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if convert
                && iterable.ty == Type::Str
                && element.is_none_or(|ty| ty == Type::Int) =>
            {
                // Exhaust the checked immutable text in order, including its first conversion error.
                Ok(Expr {
                    ty: Type::ListInt,
                    kind: ExprKind::Builtin(
                        Builtin::StringIntList(self.call_origin(call)),
                        vec![iterable],
                    ),
                })
            }
            input => self.collect_list(input, convert, element, call),
        }
    }

    /// Collect an already-evaluated iterable into a fresh list.
    pub(super) fn collect_list(
        &mut self,
        input: Iteration,
        convert: bool,
        element: Option<Type>,
        call: &impl Ranged,
    ) -> Result<Expr, String> {
        self.collect_list_with(input, element, call, |checker, value| {
            checker.consumer_value(value, convert, call)
        })
    }

    /// Keep the partial list rooted while pulling and converting its elements.
    pub(super) fn collect_list_with(
        &mut self,
        input: Iteration,
        mut element: Option<Type>,
        call: &impl Ranged,
        mut convert: impl FnMut(&mut Self, Expr) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let result = self.hidden(Type::None);
        let mut body = self.expand_items(input, call, |checker, value| {
            let value = convert(checker, value)?;
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

    /// Materializes the default line differ where the caller immediately exhausts it.
    fn materialize_ndiff(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [before, after] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "ndiff requires two string sequences",
            ));
        };
        let ty = self.types.list(Type::Str);
        let before = self.expr_expected(before, ty)?;
        let after = self.expr_expected(after, ty)?;
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::Ndiff, vec![before, after]),
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
        if let ast::Expr::Call(view) = source
            && self.resolve(&view.func).is_none()
            && let ast::Expr::Attribute(method) = &*view.func
            && method.attr.as_str() == "values"
            && self.class_reference(&method.value).is_none()
        {
            let receiver = self.expr(&method.value)?;
            if receiver.ty == Type::PythonObject {
                let iterable = self.call_on(view, Some(receiver))?;
                return self.bind_registered_builtin_call(call, "sum", vec![iterable]);
            }
            if view.arguments.is_empty()
                && self
                    .types
                    .mapping(receiver.ty)
                    .is_some_and(|(_, value)| value == Type::Int)
            {
                let start = self
                    .integer_fold_start(call)?
                    .unwrap_or_else(|| Expr::integer("0"));
                return Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::DictSumValues, vec![receiver, start]),
                });
            }
            // A first-party values method still produces its usual iterable.
            // Use the already checked receiver, including any walrus bindings.
            method_input = Some(if view.arguments.is_empty() {
                let (Type::Class(class) | Type::StrEnum(class)) = receiver.ty else {
                    return Err(error(
                        self.source,
                        view,
                        "sum of values requires an integer-valued mapping or method returning an integer iterable",
                    ));
                };
                let iterable = self.iteration_method_call(view, method, class, receiver)?;
                self.iteration_value(iterable, false, view)?
            } else {
                self.mapping_values_iteration(view, method, receiver)?
            });
        }
        if let ast::Expr::ListComp(display) = source {
            // The exact integer reduction preserves the order of the eager comprehension.
            let plan = imre_ast::comprehension::Plan::new(
                &display.generators,
                imre_ast::comprehension::Output::List(&display.elt),
            );
            let subtotal =
                self.comprehension_display(source, &plan, comprehensions::Collector::IntegerSum)?;
            let start = self.integer_fold_start(call)?;
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
        let start = self.integer_fold_start(call)?;
        let iterable = match input {
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if !convert && iterable.ty == Type::ListInt => iterable,
            input => {
                let result = self.hidden(Type::Int);
                if let Some(sum) = zip_sum {
                    self.zip_sums.insert(result, sum);
                }
                let start = start.unwrap_or_else(|| Expr::integer("0"));
                return self.integer_fold(call, input, start, result, convert, Binary::Add);
            }
        };
        if let ExprKind::Builtin(Builtin::Slice, arguments) = iterable.kind {
            // Capture the slice's numeric snapshot before evaluating
            // `start`, which may mutate or replace the original list.
            // Exact integer addition has no observable intermediate work.
            let subtotal = Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::ListSliceSum, arguments),
            };
            return Ok(add_start(subtotal, start));
        }
        // Evaluate both arguments before borrowing the list: `start` may
        // mutate the source through an alias. Exact integer addition has
        // no callbacks or recoverable errors during the reduction.
        let mut operands = vec![iterable];
        operands.extend(start);
        let value = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::ListSum, operands),
        };
        if self
            .fixed_sites
            .as_ref()
            .is_some_and(|sites| sites.has_written_operands(call))
        {
            self.record_named_call(call, &value);
        }
        Ok(value)
    }

    /// Call the shared reducer when the operand storage and successful return type are known.
    pub(super) fn sumprod_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let ([left, right], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            return Err(error(
                self.source,
                call,
                "checked math.sumprod requires two positional arguments",
            ));
        };
        let mut operands = [self.expr(left)?, self.expr(right)?];
        for value in &mut operands {
            // A literal tuple's fields are already checked in evaluation order. The reducer
            // consumes these private values immediately, so no tuple identity is observable.
            if let ExprKind::Tuple(items) = &mut value.kind
                && let Some(element) = [Type::Int, Type::Float]
                    .into_iter()
                    .find(|ty| items.iter().all(|item| item.ty == *ty))
            {
                value.ty = self.types.list(element);
                value.kind = ExprKind::List(std::mem::take(items));
            }
        }
        let ty = if operands.iter().all(|value| value.ty == Type::ListInt) {
            Type::Int
        } else {
            let floats = self.types.list(Type::Float);
            for value in &mut operands {
                if matches!(&value.kind, ExprKind::List(items) if items.is_empty()) {
                    value.ty = floats;
                }
                if value.ty != floats {
                    return Err(error(
                        self.source,
                        call,
                        "checked math.sumprod requires matching integer or float elements in lists or tuple literals",
                    ));
                }
            }
            // A fresh nonempty literal proves a successful result is a float. Otherwise
            // the reducer can also return integer zero; preserve its concrete scalar kind.
            if operands.iter().any(Expr::is_nonempty_list_literal) {
                Type::Float
            } else {
                Type::ObjectValue
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::Sumprod, Vec::from(operands)),
        })
    }

    /// Lowers `math.prod` over exact integers without materializing the source.
    pub(super) fn integer_product(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let [source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "math.prod requires one positional iterable",
            ));
        };
        if call.arguments.keywords.len() > 1
            || call.arguments.keywords.first().is_some_and(|keyword| {
                keyword.arg.as_ref().map(|name| name.as_str()) != Some("start")
            })
        {
            return Err(error(
                self.source,
                call,
                "math.prod accepts only the keyword argument start",
            ));
        }
        let (input, convert) = self.consumer_input(source, None)?;
        let start = self
            .integer_fold_start(call)?
            .unwrap_or_else(|| Expr::integer("1"));
        let result = self.hidden(Type::Int);
        self.integer_fold(call, input, start, result, convert, Binary::Multiply)
    }

    /// Evaluates the iterable and start in order, then folds each yielded value.
    fn integer_fold(
        &mut self,
        call: &ast::ExprCall,
        input: Iteration,
        start: Expr,
        result: usize,
        convert: bool,
        operation: Binary,
    ) -> Result<Expr, String> {
        let accept_bool = matches!(operation, Binary::Multiply)
            || matches!(call.arguments.args[0], ast::Expr::Generator(_));
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
                let as_int = convert || (accept_bool && value.ty == Type::Bool);
                let value = checker.consumer_value(value, as_int, call)?;
                checker.expect(Type::Int, value.ty, call)?;
                Ok(vec![Statement::Assign(
                    result,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Binary(
                            operation,
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
    fn integer_fold_start(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        call.arguments
            .args
            .get(1)
            .or_else(|| {
                call.arguments
                    .keywords
                    .first()
                    .map(|keyword| &keyword.value)
            })
            .map(|start| self.expr_expected(start, Type::Int))
            .transpose()
    }

    /// The sealed converter has no user callback or escaping iterator object.
    /// Its input is evaluated now; each conversion stays inside the consumer.
    fn consumer_input(
        &mut self,
        expression: &ast::Expr,
        element: Option<Type>,
    ) -> Result<(Iteration, bool), String> {
        if let ast::Expr::Call(call) = expression
            && let Some((_, source)) =
                self.map_converter_input(call, |name| name == "int" && self.fixed_map(call))
        {
            if !call.arguments.keywords.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "map requires the builtin int converter and one concrete iterable",
                ));
            }
            // This consumer keeps only the iterable. A Python call needs the loaded
            // map and converter too. Method recovery can still use its field reads.
            if self.class.is_none() || self.settled_attributes.is_none() {
                self.fixed_sites = None;
            }
            return Ok((self.expansion_input(source, None)?, true));
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
        if value.ty == Type::PythonObject
            && self.missing_operation(&self.scoped_builtin_error("int", node))
        {
            return Ok(Expr {
                ty: Type::Int,
                ..Expr::unrepresented()
            });
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
            kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
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
                    body.push(pipelines::check_range_step(step.clone()));
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
fn add_start(subtotal: Expr, start: Option<Expr>) -> Expr {
    let Some(start) = start else { return subtotal };
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
