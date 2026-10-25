//! Bind the iterable, function and seed in source order. A missing function uses ordinary addition.
use super::*;

impl Checker<'_> {
    pub(in crate::frontend) fn accumulate_cursor(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let params = [
            ("iterable", ParameterKind::PositionalOrKeyword, None),
            (
                "func",
                ParameterKind::PositionalOrKeyword,
                Some(none.clone()),
            ),
            ("initial", ParameterKind::KeywordOnly, Some(none.clone())),
        ]
        .map(|(name, kind, default)| Parameter {
            name: name.into(),
            ty: Type::None,
            kind,
            default,
            sign_only: false,
        });
        let mut deferred = None;
        let mut operator = "lambda total, item: total + item";
        let mut args = self.arguments_with(call, &params, None, |checker, i, node| {
            if i == 0 {
                return checker.cursor_source_value(node);
            }
            if i == 1
                && let ast::Expr::Name(name) = node
                && checker.resolve(node).is_none()
                && !checker.names.contains_key(name.id.as_str())
            {
                match name.id.as_str() {
                    "min" => operator = "lambda total, item: item if item < total else total",
                    "max" => operator = "lambda total, item: item if item > total else total",
                    _ => return checker.expr(node),
                }
                return Ok(none.clone());
            }
            if i == 1
                && (matches!(node, ast::Expr::Lambda(_))
                    || matches!(checker.resolve(node), Some(Binding::Definition(_))))
            {
                deferred = Some(node.clone());
                return Ok(none.clone());
            }
            checker.expr(node)
        })?;
        let [source, function, initial] = args.order.as_slice() else {
            unreachable!()
        };
        let source_ty = args.values[*source].ty;
        let item = self
            .iterable_element(source_ty)
            .or_else(|| {
                matches!(source_ty, Type::Bool | Type::Int | Type::Float | Type::None)
                    .then_some(Type::Int)
            })
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "accumulate requires a source with a concrete element type",
                )
            })?;
        let seed = &args.values[*initial];
        if seed.ty != Type::None && seed.ty != item {
            return Err(error(
                self.source,
                call,
                "accumulate needs a single concrete type for its seed, items and totals",
            ));
        }
        let signature = self.types.callable(vec![item, item], item);
        let mut static_function = None;
        if let Some(node) = deferred {
            args.values[*function] = self.expr_expected(&node, signature)?;
        } else if args.values[*function].ty == Type::None {
            // Compile ordinary source operations, including list concatenation and
            // integer promotion. Keep the actual None expression in evaluation
            // order. The built-in operation has no per-instance callable record.
            let parsed = ruff_python_parser::parse_expression(operator).unwrap();
            let ast::Expr::Lambda(operation) = parsed.expr() else {
                unreachable!()
            };
            let helper = self
                .lambda_value(operation, Some(vec![item, item]), Some(item))
                .map_err(|_| {
                    error(
                        self.source,
                        call,
                        "accumulate needs a supported operation with a stable total type",
                    )
                })?;
            let ExprKind::Callable {
                function,
                receiver: None,
            } = helper.kind
            else {
                unreachable!("the built-in binary operation has no bound receiver")
            };
            self.calls.insert(function);
            static_function = Some(function);
        }
        let func = args.values[*function].ty;
        if static_function.is_none()
            && func != signature
            && !matches!(
                func,
                Type::Bool | Type::Int | Type::Str | Type::Float | Type::Bytes
            )
        {
            return Err(error(
                self.source,
                call,
                "accumulate requires a binary callable with the input and result types of its total",
            ));
        }
        Ok(self.ordered_builtin(
            Builtin::Accumulate { static_function },
            self.types.cursor(item),
            args,
        ))
    }
}
