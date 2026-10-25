//! Native sampling retains the input sequence and evaluates bounds in source order.
use super::*;
use ir::RandomOperation as Op;

fn integer(n: usize) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(n.to_string()),
    }
}
fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}
fn random(op: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Random(op), args),
    }
}

impl Checker<'_> {
    pub(super) fn random_call(&mut self, op: Op, call: &ast::ExprCall) -> Result<Expr, String> {
        let specs = match op {
            Op::Float => vec![],
            Op::Seed => vec![("a", Some(none()))],
            Op::Bits => vec![("k", None)],
            Op::Randint => vec![("a", None), ("b", None)],
            Op::Randrange => vec![
                ("start", None),
                ("stop", Some(none())),
                ("step", Some(integer(1))),
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
            None,
            |checker, i, argument| {
                let node = argument.site();
                let value = Self::string_value(argument.value(checker)?);
                if op == Op::Seed && matches!(value.ty, Type::None | Type::Str | Type::Bytes)
                    || op == Op::Randrange && i == 1 && value.ty == Type::None
                {
                    return Ok(value);
                }
                checker.coerce(value, Type::Int, node)
            },
        )?;
        let mut values: Vec<_> = args.order.iter().map(|&i| args.values[i].clone()).collect();
        let value = match op {
            Op::Seed => random(op, Type::None, values),
            Op::Float => random(op, Type::Float, values),
            Op::Bits => random(op, Type::Int, values),
            Op::Randint => {
                values.push(integer(1));
                random(op, Type::Int, values)
            }
            Op::Randrange if values[1].ty == Type::None => random(
                Op::RandrangeOne,
                Type::Int,
                vec![integer(0), values[0].clone(), values[2].clone()],
            ),
            Op::Randrange => random(op, Type::Int, values),
            _ => unreachable!(),
        };
        Ok(scope.finish(value))
    }

    pub(super) fn random_choice(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
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
                    "random.choice requires one sequence",
                ));
            }
        };
        if let ast::Expr::Call(range) = arg
            && self.unshadowed_builtin(&range.func, "range")
        {
            self.reject_builtin_keywords(range)?;
            let values = range
                .arguments
                .args
                .iter()
                .map(|v| self.expr_expected(v, Type::Int))
                .collect::<Result<Vec<_>, _>>()?;
            let bounds = match values.as_slice() {
                [stop] => vec![integer(0), stop.clone(), integer(1)],
                [start, stop] => vec![start.clone(), stop.clone(), integer(1)],
                [start, stop, step] => vec![start.clone(), stop.clone(), step.clone()],
                _ => {
                    return Err(error(
                        self.source,
                        range,
                        "range requires one to three integer bounds",
                    ));
                }
            };
            return Ok(random(Op::ChoiceRange, Type::Int, bounds));
        }
        let value = Self::string_value(self.expr(arg)?);
        let (ty, get) = match value.ty {
            Type::Str => (Type::Str, Builtin::StringGet),
            Type::Bytes => (Type::Int, Builtin::BytesGet),
            Type::ListInt | Type::List(_) => {
                (self.types.element(value.ty).unwrap(), Builtin::ListGet)
            }
            _ => {
                return Err(error(
                    self.source,
                    arg,
                    "random.choice requires a native string, bytes, list or range",
                ));
            }
        };
        let mut scope = calls::ArgumentScope::default();
        let seq = scope.hold(self, value);
        let len = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![seq.clone()]),
        };
        let idx = random(Op::ChoiceIndex, Type::Int, vec![len]);
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Builtin(get, vec![seq, idx]),
        }))
    }
}
