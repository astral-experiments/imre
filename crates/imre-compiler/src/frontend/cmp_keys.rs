//! functools comparator keys retain the callback and invoke it on comparison.
use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

pub(super) fn can_specialize(definition: &ast::StmtFunctionDef) -> bool {
    let params = &definition.parameters;
    !definition.is_async
        && definition.decorator_list.is_empty()
        && definition.type_params.is_none()
        && definition.returns.is_none()
        && params.len() == 2
        && params.posonlyargs.len() + params.args.len() == 2
        && params
            .iter_non_variadic_params()
            .all(|p| p.annotation().is_none() && p.default.is_none())
}

/// Find local functions passed to the resolved functools operation in this scope.
pub(super) fn local_comparators(
    body: &[ast::Stmt],
    is_factory: impl Fn(&ast::Expr) -> bool,
) -> HashSet<String> {
    struct Find<F> {
        factory: F,
        names: HashSet<String>,
    }
    impl<'a, F: Fn(&ast::Expr) -> bool> Visitor<'a> for Find<F> {
        fn visit_stmt(&mut self, statement: &'a ast::Stmt) {
            if !matches!(
                statement,
                ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_)
            ) {
                walk_stmt(self, statement);
            }
        }
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if let ast::Expr::Call(call) = expr
                && (self.factory)(&call.func)
            {
                let arg = match (
                    call.arguments.args.as_ref(),
                    call.arguments.keywords.as_ref(),
                ) {
                    ([one], []) => Some(one),
                    ([], [kw]) if kw.arg.as_deref() == Some("mycmp") => Some(&kw.value),
                    _ => None,
                };
                if let Some(ast::Expr::Name(name)) = arg {
                    self.names.insert(name.id.to_string());
                }
            }
            if !matches!(expr, ast::Expr::Lambda(_)) {
                walk_expr(self, expr);
            }
        }
    }
    let mut find = Find {
        factory: is_factory,
        names: HashSet::new(),
    };
    find.visit_body(body);
    find.names
}

fn view(value: Expr, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
    }
}

fn raised(ty: Type, name: &str, message: &str, mut evaluated: Vec<Expr>) -> Expr {
    evaluated.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind(name).unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    });
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RaiseException, evaluated),
    }
}

impl Checker<'_> {
    pub(super) fn cmp_key_hash(&self, key: Expr) -> Expr {
        raised(
            Type::Int,
            "TypeError",
            "unhashable type: 'functools.KeyWrapper'",
            vec![key],
        )
    }

    /// Bind the one public argument and evaluate supplied expressions even on a binding error.
    fn cmp_argument<'e>(
        &mut self,
        call: &'e ast::ExprCall,
        name: &str,
        param: &str,
        ty: Type,
    ) -> Result<Result<&'e ast::Expr, Expr>, String> {
        let args = &call.arguments;
        if args.args.is_empty() || !args.keywords.is_empty() {
            self.graph
                .checked_missing_argument(self.module, call.range());
        }
        self.graph
            .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
        for extra in args.args.iter().skip(1) {
            self.graph
                .checked_excess_argument(self.module, extra.range());
        }
        if let ([arg], []) = (args.args.as_ref(), args.keywords.as_ref()) {
            return Ok(Ok(arg));
        }
        if let ([], [kw]) = (args.args.as_ref(), args.keywords.as_ref())
            && kw.arg.as_deref() == Some(param)
        {
            return Ok(Ok(&kw.value));
        }
        if args.args.iter().any(ast::Expr::is_starred_expr)
            || args.keywords.iter().any(|kw| kw.arg.is_none())
        {
            return Err(error(
                self.source,
                call,
                "comparator keys need explicit arguments",
            ));
        }
        let count = args.args.len() + args.keywords.len();
        let message = if count > 1 {
            format!("{name}() takes at most 1 argument ({count} given)")
        } else {
            format!("{name}() missing required argument '{param}' (pos 1)")
        };
        let used = args
            .args
            .iter()
            .chain(args.keywords.iter().map(|kw| &kw.value))
            .map(|arg| self.cmp_callback_value(arg).map(|(value, _)| value))
            .collect::<Result<_, _>>()?;
        Ok(Err(raised(ty, "TypeError", &message, used)))
    }

    fn new_cmp_key(
        &self,
        comparator: Expr,
        value: Option<Expr>,
        source: Option<(usize, u32)>,
    ) -> Expr {
        let bound = value.is_some();
        let mut fields = vec![comparator];
        if let Some(value) = value {
            fields.push(Expr {
                ty: self.types.list(value.ty),
                kind: ExprKind::List(vec![value]),
            });
        }
        let frame = self
            .types
            .cmp_frame(fields.iter().map(|field| field.ty).collect(), source);
        let Type::Tuple(id) = frame else {
            unreachable!()
        };
        view(
            Expr {
                ty: frame,
                kind: ExprKind::Tuple(fields),
            },
            Type::CmpKey(id, bound),
        )
    }

    fn cmp_callback_value(
        &mut self,
        arg: &ast::Expr,
    ) -> Result<(Expr, Option<(usize, u32)>), String> {
        let source = self.deferred_comparator(arg);
        let comparator = if let Some((_, start)) = source {
            pipelines::integer(i64::from(start))
        } else {
            self.expr(arg)?
        };
        Ok((comparator, source))
    }

    pub(super) fn cmp_to_key(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let arg = match self.cmp_argument(call, "cmp_to_key", "mycmp", Type::None)? {
            Ok(arg) => arg,
            Err(error) => return Ok(error),
        };
        let (comparator, source) = self.cmp_callback_value(arg)?;
        Ok(self.new_cmp_key(comparator, None, source))
    }

    pub(super) fn cmp_field(&self, key: Expr, frame: usize, field: usize) -> Expr {
        let frame = Type::Tuple(frame);
        Expr {
            ty: self.types.fields(frame).unwrap()[field],
            kind: ExprKind::Field(Box::new(view(key, frame)), field),
        }
    }

    pub(super) fn bind_cmp_key(
        &mut self,
        call: &ast::ExprCall,
        key: Expr,
        frame: usize,
    ) -> Result<Expr, String> {
        if matches!(key.ty, Type::CmpKey(_, true)) {
            self.graph.checked_callable_key(self.module, call.range());
        }
        let mut scope = calls::ArgumentScope::default();
        let key = scope.hold(self, key);
        let arg = match self.cmp_argument(call, "K", "obj", Type::None)? {
            Ok(arg) => arg,
            Err(error) => return Ok(scope.finish(error)),
        };
        let value = self.expr(arg)?;
        Ok(scope.finish(self.new_cmp_key(
            self.cmp_field(key, frame, 0),
            Some(value),
            self.types.deferred_cmp_source(frame),
        )))
    }

    pub(super) fn cmp_key_obj(&self, key: Expr, frame: usize, bound: bool) -> Expr {
        if !bound {
            return Expr {
                ty: Type::None,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(key)],
                    value: Box::new(Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    }),
                    clear: vec![],
                },
            };
        }
        let cell = self.cmp_field(key, frame, 1);
        Expr {
            ty: self.types.element(cell.ty).unwrap(),
            kind: ExprKind::Builtin(Builtin::ListGet, vec![cell, pipelines::integer(0)]),
        }
    }

    /// Each intermediate key is evaluated once, after earlier comparisons succeed.
    pub(super) fn cmp_key_chain(
        &mut self,
        values: Vec<Expr>,
        ops: &[ast::CmpOp],
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        let mut values = values.into_iter();
        let left = values.next().unwrap();
        self.cmp_key_pairs(left, &mut values, ops, site)
    }

    fn cmp_key_pairs(
        &mut self,
        left: Expr,
        values: &mut impl Iterator<Item = Expr>,
        ops: &[ast::CmpOp],
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let left = scope.hold(self, left);
        let right = scope.hold(self, values.next().unwrap());
        if !matches!(left.ty, Type::CmpKey(..)) && !matches!(right.ty, Type::CmpKey(..)) {
            return Err(error(
                self.source,
                site,
                "mixed comparator-key chains need a key in each comparison",
            ));
        }
        let compared = self.compare_cmp_keys(left, right.clone(), ops[0], site)?;
        if ops.len() == 1 {
            return Ok(scope.finish(compared));
        }
        let then = self.cmp_key_pairs(right, values, &ops[1..], site)?;
        let no = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        };
        Ok(scope.finish(Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(Box::new(compared), Box::new(then), Box::new(no)),
        }))
    }

    /// Only the left key supplies the comparator, even when two factories are involved.
    fn compare_cmp_keys(
        &mut self,
        left: Expr,
        right: Expr,
        op: ast::CmpOp,
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        let operation = match op {
            ast::CmpOp::Lt => Compare::Less,
            ast::CmpOp::LtE => Compare::LessEqual,
            ast::CmpOp::Eq => Compare::Equal,
            ast::CmpOp::NotEq => Compare::NotEqual,
            ast::CmpOp::Gt => Compare::Greater,
            ast::CmpOp::GtE => Compare::GreaterEqual,
            _ => {
                return Err(error(
                    self.source,
                    site,
                    "unsupported comparator-key comparison",
                ));
            }
        };
        let (Type::CmpKey(l, lb), Type::CmpKey(r, rb)) = (left.ty, right.ty) else {
            return Ok(raised(
                Type::Bool,
                "TypeError",
                "other argument must be K instance",
                vec![left, right],
            ));
        };
        if !lb || !rb {
            return Ok(raised(
                Type::Bool,
                "AttributeError",
                "object",
                vec![left, right],
            ));
        }
        let a = self.cmp_key_obj(left.clone(), l, true);
        let b = self.cmp_key_obj(right, r, true);
        let comparator = if let Some(source) = self.types.deferred_cmp_source(l) {
            self.specialize_comparator(source, a.ty, b.ty, site)?
        } else {
            self.cmp_field(left, l, 0)
        };
        let Some((params, returns)) = self.types.signature(comparator.ty) else {
            let name = functools::noncallable_type(comparator.ty)
                .ok_or_else(|| error(self.source, site, "unsupported comparator callback type"))?;
            return Ok(raised(
                Type::Bool,
                "TypeError",
                &format!("'{name}' object is not callable"),
                vec![comparator],
            ));
        };
        let [expected_a, expected_b] = params.as_slice() else {
            return Err(error(
                self.source,
                site,
                "comparison requires a native callback accepting two positional arguments",
            ));
        };
        let args = vec![
            self.coerce(a, *expected_a, site)?,
            self.coerce(b, *expected_b, site)?,
        ];
        let comparison = Expr {
            ty: returns,
            kind: ExprKind::Invoke {
                callable: Box::new(comparator),
                args: ir::CallArgs::positional(args),
            },
        };
        let zero = match returns {
            Type::Int => pipelines::integer(0),
            Type::Float => Expr {
                ty: Type::Float,
                kind: ExprKind::Float(0.0),
            },
            _ => {
                return Err(error(
                    self.source,
                    site,
                    "comparator result requires native ordering with zero",
                ));
            }
        };
        Ok(pipelines::compare(operation, comparison, zero))
    }
}
