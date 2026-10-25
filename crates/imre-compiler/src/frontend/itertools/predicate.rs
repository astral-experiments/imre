use super::*;
use crate::ir::{PredicateMode, PredicateTest};

impl Checker<'_> {
    /// Evaluate constructor arguments now; call and truth-test the function only when pulling.
    pub(in crate::frontend) fn predicate_cursor(
        &mut self,
        call: &ast::ExprCall,
        mode: PredicateMode,
    ) -> Result<Expr, String> {
        let args = &call.arguments.args;
        let keywords = &call.arguments.keywords;
        let builtin_bool = |checker: &Self, node: &ast::Expr| {
            matches!(node, ast::Expr::Name(name) if name.id == "bool")
                && checker.resolve(node).is_none()
                && !checker.names.contains_key("bool")
        };
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        if args.len() != 2 || !keywords.is_empty() {
            let name = match mode {
                PredicateMode::FilterFalse => "filterfalse",
                PredicateMode::DropWhile => "dropwhile",
                PredicateMode::TakeWhile => "takewhile",
            };
            let message = if keywords.is_empty() {
                format!("{name} expected 2 arguments, got {}", args.len())
            } else {
                format!("{name}() takes no keyword arguments")
            };
            if keywords.iter().any(|kw| kw.arg.is_none())
                || args.iter().any(|arg| matches!(arg, ast::Expr::Starred(_)))
            {
                return Err(error(
                    self.source,
                    call,
                    "predicate cursor argument unpacking needs a known call shape",
                ));
            }
            let mut values = Vec::new();
            for node in args.iter().chain(keywords.iter().map(|kw| &kw.value)) {
                values.push(if matches!(node, ast::Expr::Lambda(lambda) if lambda.parameters.as_deref().is_none_or(|params| params.iter_non_variadic_params().all(|p| p.default.is_none()))) || builtin_bool(self, node) {
                    none.clone()
                } else { self.cursor_source_value(node)? });
            }
            values.push(Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("TypeError").unwrap(),
                    message: Some(Box::new(unpacking::string(message))),
                },
            });
            return Ok(Expr {
                ty: self.types.cursor(Type::Int),
                kind: ExprKind::Builtin(Builtin::RaiseException, values),
            });
        }
        let func = &args[0];
        let source = self.cursor_source_value(&args[1])?;
        let item = self
            .iterable_element(source.ty)
            .or_else(|| {
                matches!(source.ty, Type::None | Type::Bool | Type::Int | Type::Float)
                    .then_some(Type::Int)
            })
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "predicate cursor requires one concrete source element type",
                )
            })?;
        let truth = builtin_bool(self, func);
        let range = matches!(func, ast::Expr::Call(c) if matches!(&*c.func, ast::Expr::Name(n) if n.id == "range") && self.resolve(&c.func).is_none() && !self.names.contains_key("range"));
        let function = if truth {
            none
        } else if range {
            self.cursor_source_value(func)?
        } else {
            self.item_callback(func, item, None, None)?
        };
        let direct = truth || (mode == PredicateMode::FilterFalse && function.ty == Type::None);
        let callable = matches!(function.ty, Type::Callable(_))
            || matches!(function.ty, Type::Class(class) if self.classes.methods[class].contains_key("__call__"));
        let test = if direct || callable {
            let (expression, params) = if direct {
                ("lambda item: True if item else False", vec![item])
            } else {
                (
                    "lambda pred, item: True if pred(item) else False",
                    vec![function.ty, item],
                )
            };
            let parsed = ruff_python_parser::parse_expression(expression).unwrap();
            let ast::Expr::Lambda(lambda) = parsed.expr() else {
                unreachable!()
            };
            let helper = self.lambda_value(lambda, Some(params), Some(Type::Bool))?;
            let ExprKind::Callable {
                function,
                receiver: None,
            } = helper.kind
            else {
                unreachable!()
            };
            self.calls.insert(function);
            PredicateTest::Function(function)
        } else {
            let name = match function.ty {
                _ if range => "range",
                Type::None => "NoneType",
                Type::Bool => "bool",
                Type::Int => "int",
                Type::Float => "float",
                Type::Str | Type::StrLiteral(_) => "str",
                Type::Bytes => "bytes",
                Type::ListInt | Type::List(_) => "list",
                Type::Tuple(_) | Type::VarTuple(_) => "tuple",
                Type::Dict(_) => "dict",
                Type::Set(_) => "set",
                Type::Class(class) => &self.classes.declarations.definitions[class].name,
                _ => {
                    return Err(error(
                        self.source,
                        func,
                        "predicate cursor needs a known callable or concrete argument type",
                    ));
                }
            };
            PredicateTest::Invalid(format!("'{name}' object is not callable"))
        };
        Ok(Expr {
            ty: self.types.cursor(item),
            kind: ExprKind::Builtin(Builtin::Predicate(mode, test), vec![function, source]),
        })
    }
}
