//! Native sampling retains the input sequence and evaluates bounds in source order.
use super::*;
use ir::RandomOperation as Op;

fn random(op: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Random(op), args),
    }
}

impl Checker<'_> {
    pub(super) fn random_call(&mut self, op: Op, call: &ast::ExprCall) -> Result<Expr, String> {
        if op == Op::Seed
            && (call.arguments.args.len() == 2
                || call
                    .arguments
                    .keywords
                    .iter()
                    .any(|kw| kw.arg.as_deref() == Some("version")))
        {
            return Err(self.scoped_builtin_error("random.seed", call));
        }
        let specs = match op {
            Op::Shuffle => vec![("x", None)],
            Op::Float => vec![],
            Op::Seed => vec![("a", Some(Expr::none()))],
            Op::Bits => vec![("k", None)],
            Op::Randint => vec![("a", None), ("b", None)],
            Op::Randrange => vec![
                ("start", None),
                ("stop", Some(Expr::none())),
                ("step", Some(Expr::integer(1))),
            ],
            _ => unreachable!(),
        };
        if op == Op::Bits {
            self.reject_builtin_keywords(call)?;
        }
        let params: Vec<_> = specs
            .into_iter()
            .map(|(name, default)| Parameter {
                name: name.into(),
                ty: Type::Int,
                kind: ParameterKind::PositionalOrKeyword,
                default,
                sign_only: false,
            })
            .collect();
        let mut scope = calls::ArgumentScope::default();
        let args = self.bind_supplied(
            call,
            &params,
            None,
            Some(&mut scope),
            &mut |checker, i, argument| {
                let node = argument.site();
                let value = Self::string_value(argument.value(checker)?);
                if op == Op::Shuffle {
                    return if matches!(value.ty, Type::ListInt | Type::List(_)) {
                        Ok(value)
                    } else {
                        Err(checker.object_builtin_error("random.shuffle", node))
                    };
                }
                if op == Op::Seed && value.ty == Type::Float {
                    return Err(checker.scoped_builtin_error("random.seed", node));
                }
                if op == Op::Seed && matches!(value.ty, Type::None | Type::Str | Type::Bytes)
                    || op == Op::Randrange && i == 1 && value.ty == Type::None
                {
                    return Ok(value);
                }
                checker.coerce(value, Type::Int, node)
            },
        )?;
        let mut values = args.into_formal_order();
        let value = match op {
            Op::Shuffle | Op::Seed => random(op, Type::None, values),
            Op::Float => random(op, Type::Float, values),
            Op::Bits => random(op, Type::Int, values),
            Op::Randint => {
                values.push(Expr::integer(1));
                random(op, Type::Int, values)
            }
            Op::Randrange if values[1].ty == Type::None => random(
                Op::RandrangeOne,
                Type::Int,
                vec![Expr::integer(0), values[0].clone(), values[2].clone()],
            ),
            Op::Randrange => random(op, Type::Int, values),
            _ => unreachable!(),
        };
        Ok(scope.finish(value))
    }

    pub(super) fn random_choice(
        &mut self,
        call: &ast::ExprCall,
        secure: bool,
    ) -> Result<Expr, String> {
        let name = if secure {
            "secrets.choice"
        } else {
            "random.choice"
        };
        calls::reject_unpacking(call, self.source)?;
        let arg = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some("seq") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    &format!("{name} requires one sequence"),
                ));
            }
        };
        if let ast::Expr::Call(range) = arg
            && self.unshadowed_builtin(&range.func, "range")
        {
            if let Some(failure) = self.range_argument_error(range, Type::Int)? {
                return Ok(failure);
            }
            self.reject_builtin_keywords(range)?;
            let values = range
                .arguments
                .args
                .iter()
                .map(|v| self.expr_expected(v, Type::Int))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let bounds = match values.as_slice() {
                [stop] => vec![Expr::integer(0), stop.clone(), Expr::integer(1)],
                [start, stop] => vec![start.clone(), stop.clone(), Expr::integer(1)],
                [start, stop, step] => vec![start.clone(), stop.clone(), step.clone()],
                _ => {
                    return Err(error(
                        self.source,
                        range,
                        "range requires one to three integer bounds",
                    ));
                }
            };
            return Ok(random(Op::ChoiceRange { secure }, Type::Int, bounds));
        }
        let value = Self::string_value(self.expr(arg)?);
        let (ty, get) = match value.ty {
            Type::Str => (Type::Str, Builtin::StringGet),
            Type::Bytes => (Type::Int, Builtin::BytesGet),
            ty if let Some(element) = self.types.homogeneous_tuple_element(ty) => {
                (element, Builtin::TupleGet)
            }
            Type::ListInt | Type::List(_) => (
                self.types.element(value.ty).unwrap(),
                Builtin::ListGet(None),
            ),
            _ => {
                return Err(error(
                    self.source,
                    arg,
                    &format!(
                        "{name} requires a native string, bytes, list, homogeneous tuple or range"
                    ),
                ));
            }
        };
        let mut scope = calls::ArgumentScope::default();
        let seq = scope.hold(self, value);
        let len = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![seq.clone()]),
        };
        let idx = random(Op::ChoiceIndex { secure }, Type::Int, vec![len]);
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Builtin(get, vec![seq, idx]),
        }))
    }
}
