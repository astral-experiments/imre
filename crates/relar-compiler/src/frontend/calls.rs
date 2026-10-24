//! Binds closed signatures without conflating argument evaluation and ABI order.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    KeywordOnly,
}

#[derive(Clone)]
pub(super) struct Parameter {
    pub name: String,
    pub ty: Type,
    pub kind: ParameterKind,
    pub default: Option<Expr>,
    pub sign_only: bool,
}

/// Defaults have no user-visible effects or mutable identity. Checking them at
/// definition time also rejects unsupported defaults in never-called functions.
/// Only one optional sign on a numeric literal is accepted as an expression.
pub(super) fn scalar_default(
    expression: &ast::Expr,
    expected: Type,
    types: &Types,
    source: &str,
) -> Result<Expr, String> {
    let mut value = scalar_literal(expression, source)?;
    let ty = value.ty;
    let concrete = types.payload(expected).unwrap_or(expected);
    if literals::accepts(types, concrete, &value) {
        value = literals::view(value, concrete);
    }
    if types
        .members(concrete)
        .is_some_and(|members| members.contains(&ty))
    {
        value = Expr {
            ty: concrete,
            kind: ExprKind::Builtin(Builtin::UnionInject, vec![value]),
        };
    }
    if value.ty == expected {
        return Ok(value);
    }
    if let Some(payload) = types.payload(expected) {
        let kind = if ty == Type::None {
            Some(ExprKind::Absent(Box::new(value)))
        } else if value.ty == payload {
            Some(ExprKind::Present(Box::new(value)))
        } else {
            None
        };
        if let Some(kind) = kind {
            return Ok(Expr { ty: expected, kind });
        }
    }
    Err(error(
        source,
        expression,
        &format!("default requires exact type {expected}, got {ty}"),
    ))
}

/// A scalar default also supplies an exact type for an unannotated parameter.
pub(super) fn scalar_literal(expression: &ast::Expr, source: &str) -> Result<Expr, String> {
    let (ty, kind) = match expression {
        ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
            value: ast::Number::Int(value),
            ..
        }) => (Type::Int, ExprKind::Int(value.to_string())),
        ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
            value: ast::Number::Float(value),
            ..
        }) => (Type::Float, ExprKind::Float(*value)),
        ast::Expr::UnaryOp(unary)
            if matches!(unary.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && matches!(&*unary.operand, ast::Expr::NumberLiteral(value) if matches!(&value.value, ast::Number::Int(_) | ast::Number::Float(_))) =>
        {
            let ast::Expr::NumberLiteral(value) = &*unary.operand else {
                unreachable!()
            };
            let (ty, kind) = match &value.value {
                ast::Number::Int(value) => (Type::Int, ExprKind::Int(value.to_string())),
                ast::Number::Float(value) => (Type::Float, ExprKind::Float(*value)),
                _ => unreachable!(),
            };
            let kind = if unary.op == ast::UnaryOp::USub {
                ExprKind::Negate(Box::new(Expr { ty, kind }))
            } else {
                kind
            };
            (ty, kind)
        }
        ast::Expr::BooleanLiteral(value) => (Type::Bool, ExprKind::Bool(value.value)),
        ast::Expr::StringLiteral(value) => {
            check_string(value, source)?;
            (Type::Str, ExprKind::Str(value.value.to_string()))
        }
        ast::Expr::BytesLiteral(value) => {
            (Type::Bytes, ExprKind::Bytes(value.value.bytes().collect()))
        }
        ast::Expr::NoneLiteral(_) => (Type::None, ExprKind::None),
        _ => {
            return Err(error(
                source,
                expression,
                "defaults must be int, float, bool, str, bytes, or None literals; numbers may have one sign",
            ));
        }
    };
    Ok(Expr { ty, kind })
}

/// Every dispatch route uses a closed signature, including methods reached
/// through iterable consumers before ordinary call-expression lowering.
pub(super) fn reject_unpacking(call: &ast::ExprCall, source: &str) -> Result<(), String> {
    if call
        .arguments
        .args
        .iter()
        .any(|argument| matches!(argument, ast::Expr::Starred(_)))
        || call
            .arguments
            .keywords
            .iter()
            .any(|keyword| keyword.arg.is_none())
    {
        return Err(error(source, call, "argument unpacking is unsupported"));
    }
    Ok(())
}

impl Checker<'_> {
    /// The receiver and supplied expressions stay in source order. The separate
    /// permutation binds each parameter only after every supplied value exists.
    /// Literal defaults follow supplied expressions and cannot run user code.
    ///
    /// ```python
    /// f(second=read(), first=write())  # read() still runs before write()
    /// ```
    pub(super) fn arguments(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        receiver: Option<Expr>,
    ) -> Result<ir::CallArgs, String> {
        self.arguments_with(call, params, receiver, |checker, index, argument| {
            checker.direct_argument(argument, params[index].ty, params[index].sign_only)
        })
    }

    /// Discovery and final checking share binding, defaults, and evaluation order.
    /// Only the source of the parameter's concrete type differs between passes.
    pub(super) fn arguments_with(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        receiver: Option<Expr>,
        mut check: impl FnMut(&mut Self, usize, &ast::Expr) -> Result<Expr, String>,
    ) -> Result<ir::CallArgs, String> {
        let offset = usize::from(receiver.is_some());
        let mut values = receiver.into_iter().collect::<Vec<_>>();
        let mut order = vec![usize::MAX; params.len() + offset];
        if offset != 0 {
            order[0] = 0;
        }
        // Without starred arguments Python requires all positional expressions
        // before keyword expressions, so these two lists retain source order.
        for (index, argument) in call.arguments.args.iter().enumerate() {
            params
                .get(index)
                .filter(|parameter| parameter.kind != ParameterKind::KeywordOnly)
                .ok_or_else(|| error(self.source, argument, "too many positional arguments"))?;
            order[index + offset] = values.len();
            values.push(check(self, index, argument)?);
        }
        for keyword in &call.arguments.keywords {
            let name = keyword
                .arg
                .as_ref()
                .expect("call lowering rejects argument unpacking")
                .as_str();
            let index = params
                .iter()
                .position(|parameter| parameter.name == name)
                .ok_or_else(|| {
                    error(
                        self.source,
                        keyword,
                        &format!("unexpected keyword argument {name:?}"),
                    )
                })?;
            let parameter = &params[index];
            if parameter.kind == ParameterKind::PositionalOnly {
                return Err(error(
                    self.source,
                    keyword,
                    &format!("parameter {name:?} is positional-only"),
                ));
            }
            if order[index + offset] != usize::MAX {
                return Err(error(
                    self.source,
                    keyword,
                    &format!("multiple values for argument {name:?}"),
                ));
            }
            order[index + offset] = values.len();
            values.push(check(self, index, &keyword.value)?);
        }
        for (index, parameter) in params.iter().enumerate() {
            if order[index + offset] == usize::MAX {
                let default = parameter.default.as_ref().ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        &format!("missing required argument {:?}", parameter.name),
                    )
                })?;
                order[index + offset] = values.len();
                let value = match &default.kind {
                    ExprKind::Present(value) => &**value,
                    _ => default,
                };
                if let ExprKind::Call(id, _) = &value.kind {
                    self.calls.insert(*id);
                }
                values.push(default.clone());
            }
        }
        Ok(ir::CallArgs {
            values,
            order,
            exhaustive_enum_match: false,
        })
    }

    pub(super) fn reject_builtin_keywords(&self, call: &ast::ExprCall) -> Result<(), String> {
        if call.arguments.keywords.is_empty() {
            Ok(())
        } else {
            Err(error(
                self.source,
                call,
                "keyword arguments are unsupported for this builtin",
            ))
        }
    }
}
