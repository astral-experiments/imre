//! Binds closed signatures without conflating argument evaluation and ABI order.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    KeywordOnly,
    KeywordRest,
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
pub(crate) fn scalar_literal(expression: &ast::Expr, source: &str) -> Result<Expr, String> {
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

pub(super) fn starred(call: &ast::ExprCall) -> bool {
    call.arguments
        .args
        .iter()
        .any(|arg| matches!(arg, ast::Expr::Starred(_)))
}

/// The value of a spread field is already checked when its fixed tuple is evaluated.
pub(super) enum Supplied<'a> {
    Source(&'a ast::Expr),
    Field(Expr, &'a ast::Expr),
}

impl<'a> Supplied<'a> {
    pub(super) fn site(&self) -> &'a ast::Expr {
        match self {
            Self::Source(node) | Self::Field(_, node) => node,
        }
    }

    pub(super) fn value(self, checker: &mut Checker<'_>) -> Result<Expr, String> {
        match self {
            Self::Source(node) => checker.expr(node),
            Self::Field(value, _) => Ok(value),
        }
    }

    pub(super) fn checked(
        self,
        checker: &mut Checker<'_>,
        ty: Type,
        sign_only: bool,
    ) -> Result<Expr, String> {
        match self {
            Self::Source(node) => checker.direct_argument(node, ty, sign_only),
            Self::Field(value, node) => checker.direct_value(value, node, ty, sign_only),
        }
    }
}

/// Roots source expressions in Python evaluation order while expanding fixed tuples.
#[derive(Default)]
pub(super) struct ArgumentScope {
    body: Vec<Statement>,
    clear: Vec<usize>,
}

impl ArgumentScope {
    pub(super) fn hold(&mut self, checker: &mut Checker<'_>, value: Expr) -> Expr {
        let slot = checker.hidden(value.ty);
        self.body.push(Statement::Assign(slot, value));
        self.clear.push(slot);
        checker.local(slot)
    }

    pub(super) fn spread(
        &mut self,
        checker: &mut Checker<'_>,
        argument: &ast::ExprStarred,
    ) -> Result<Vec<Expr>, String> {
        let value = checker.expr(&argument.value)?;
        let fields = checker.types.fields(value.ty).ok_or_else(|| {
            error(
                checker.source,
                argument,
                "starred argument requires a fixed tuple",
            )
        })?;
        let tuple = self.hold(checker, value);
        Ok(fields
            .into_iter()
            .enumerate()
            .map(|(index, ty)| Expr {
                ty,
                kind: ExprKind::Field(Box::new(tuple.clone()), index),
            })
            .collect())
    }

    pub(super) fn finish(self, value: Expr) -> Expr {
        if self.body.is_empty() {
            return value;
        }
        Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: self.body,
                clear: self.clear,
                value: Box::new(value),
            },
        }
    }
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

    /// A keyword-rest dictionary is packed after all arguments have evaluated.
    /// Retain the supplied values in source order even for constructors and local calls.
    pub(super) fn held_arguments(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        receiver: Option<Expr>,
        scope: &mut ArgumentScope,
    ) -> Result<ir::CallArgs, String> {
        if !params.iter().any(|p| p.kind == ParameterKind::KeywordRest) {
            return self.arguments(call, params, receiver);
        }
        self.bind_supplied(
            call,
            params,
            receiver,
            Some(scope),
            |checker, index, argument| {
                argument.checked(checker, params[index].ty, params[index].sign_only)
            },
        )
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
        self.bind_supplied(call, params, receiver, None, |checker, index, argument| {
            let Supplied::Source(node) = argument else {
                unreachable!("only source calls expand tuples")
            };
            check(checker, index, node)
        })
    }

    pub(super) fn bind_supplied(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        receiver: Option<Expr>,
        mut scope: Option<&mut ArgumentScope>,
        mut check: impl FnMut(&mut Self, usize, Supplied<'_>) -> Result<Expr, String>,
    ) -> Result<ir::CallArgs, String> {
        let offset = usize::from(receiver.is_some());
        let mut values = Vec::new();
        let mut order = vec![usize::MAX; params.len() + offset];
        if let Some(value) = receiver {
            values.push(if let Some(scope) = scope.as_deref_mut() {
                scope.hold(self, value)
            } else {
                value
            });
            order[0] = 0;
        }
        let rest = params
            .iter()
            .position(|p| p.kind == ParameterKind::KeywordRest);
        let mut extra = Vec::new();
        let mut positional = 0;
        for arg in &call.arguments.args {
            let mut bind = |checker: &mut Self,
                            input,
                            scope: Option<&mut ArgumentScope>|
             -> Result<(), String> {
                params
                    .get(positional)
                    .filter(|parameter| {
                        matches!(
                            parameter.kind,
                            ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
                        )
                    })
                    .ok_or_else(|| error(checker.source, arg, "too many positional arguments"))?;
                order[positional + offset] = values.len();
                let value = check(checker, positional, input)?;
                values.push(if let Some(scope) = scope {
                    scope.hold(checker, value)
                } else {
                    value
                });
                positional += 1;
                Ok(())
            };
            if let ast::Expr::Starred(star) = arg {
                let scope = scope
                    .as_deref_mut()
                    .ok_or_else(|| error(self.source, arg, "argument unpacking is unsupported"))?;
                for field in scope.spread(self, star)? {
                    bind(self, Supplied::Field(field, arg), Some(scope))?;
                }
            } else {
                bind(self, Supplied::Source(arg), scope.as_deref_mut())?;
            }
        }
        for keyword in &call.arguments.keywords {
            let name = keyword
                .arg
                .as_ref()
                .ok_or_else(|| {
                    error(
                        self.source,
                        keyword,
                        "keyword argument unpacking is unsupported",
                    )
                })?
                .as_str();
            let fixed = params
                .iter()
                .position(|p| p.name == name && p.kind != ParameterKind::KeywordRest);
            if let Some(index) = rest
                && fixed.is_none_or(|i| params[i].kind == ParameterKind::PositionalOnly)
            {
                let (_, element) = self
                    .types
                    .mapping(params[index].ty)
                    .expect("typed keyword rest");
                let value = self.expr_expected(&keyword.value, element)?;
                let value = if let Some(scope) = scope.as_deref_mut() {
                    scope.hold(self, value)
                } else {
                    value
                };
                extra.push((
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(name.into()),
                    },
                    value,
                ));
                continue;
            }
            let index = fixed.ok_or_else(|| {
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
            let value = check(self, index, Supplied::Source(&keyword.value))?;
            values.push(if let Some(scope) = scope.as_deref_mut() {
                scope.hold(self, value)
            } else {
                value
            });
        }
        if let Some(index) = rest {
            let (key, value) = self
                .types
                .mapping(params[index].ty)
                .expect("typed keyword rest");
            order[index + offset] = values.len();
            values.push(Expr {
                ty: params[index].ty,
                kind: ExprKind::Dict {
                    key,
                    value,
                    entries: extra,
                },
            });
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

    /// Keyword permutation happens only after supplied expressions have executed
    /// and acquired independent roots. Identity order needs no extra scope.
    pub(super) fn ordered_builtin(
        &mut self,
        operation: Builtin,
        returns: Type,
        arguments: ir::CallArgs,
    ) -> Expr {
        if arguments
            .order
            .iter()
            .copied()
            .eq(0..arguments.values.len())
        {
            return Expr {
                ty: returns,
                kind: ExprKind::Builtin(operation, arguments.values),
            };
        }
        let mut body = Vec::new();
        let mut clear = Vec::new();
        for value in arguments.values {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
        }
        let args = arguments
            .order
            .iter()
            .map(|&index| self.local(clear[index]))
            .collect();
        Expr {
            ty: returns,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: returns,
                    kind: ExprKind::Builtin(operation, args),
                }),
                clear,
            },
        }
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
