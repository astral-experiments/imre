//! Binds closed signatures without conflating argument evaluation and ABI order.

use super::*;

/// Evaluate the supplied call operands in order, then raise the checked TypeError.
pub(super) fn type_error(ty: Type, mut values: Vec<Expr>, message: String) -> Expr {
    values.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    });
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RaiseException, values),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    KeywordOnly,
    Variadic,
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
    if expected == Type::ObjectValue && ty == Type::None {
        return Ok(Expr {
            ty: expected,
            kind: ExprKind::Builtin(Builtin::BoxValue, vec![value]),
        });
    }
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
    /// Evaluate an unused argument in source order, including iterator
    /// construction, without pulling any items before the call raises.
    pub(super) fn discard(
        &mut self,
        checker: &mut Checker<'_>,
        argument: &ast::Expr,
    ) -> Result<(), String> {
        let input = checker.expansion_input(argument, None)?;
        checker.capture_consumer_input(input, &mut self.body, &mut self.clear);
        Ok(())
    }

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
    /// Evaluate the supplied arguments before raising a positional arity or
    /// keyword error. Only the spans belonging to this exact call are proved.
    pub(super) fn one_positional_error(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        parameter: &str,
        returns: Type,
    ) -> Result<Expr, String> {
        let args = &call.arguments;
        let message = if args.keywords.is_empty() {
            format!(
                "{name}() takes exactly one argument ({} given)",
                args.args.len()
            )
        } else {
            format!("{name}() takes no keyword arguments")
        };
        self.positional_argument_error(call, &[parameter], returns, message)
    }

    pub(super) fn positional_argument_error(
        &mut self,
        call: &ast::ExprCall,
        parameters: &[&str],
        returns: Type,
        message: String,
    ) -> Result<Expr, String> {
        let args = &call.arguments;
        self.graph
            .checked_missing_argument(self.module, call.range());
        for kw in &args.keywords {
            if kw
                .arg
                .as_deref()
                .is_some_and(|name| parameters.contains(&name))
            {
                self.graph
                    .checked_positional_keyword(self.module, kw.range());
            }
        }
        self.graph
            .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
        for argument in args.args.iter().skip(parameters.len()) {
            self.graph
                .checked_excess_argument(self.module, argument.range());
        }
        let mut values = Vec::new();
        for argument in &args.args {
            values.push(self.expr(argument)?);
        }
        for kw in &args.keywords {
            values.push(self.expr(&kw.value)?);
        }
        Ok(type_error(returns, values, message))
    }

    /// Return a Python class name only when the concrete receiver type is known.
    pub(super) fn exact_argument_type_name(&self, ty: Type) -> Option<&str> {
        Some(match ty {
            Type::None => "NoneType",
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::PlainObject => "object",
            Type::List(_) | Type::ListInt | Type::ByteOutput => "list",
            Type::Set(_) => "set",
            Type::Dict(_) if !self.types.is_counter(ty) && !self.types.is_default_list_dict(ty) => {
                "dict"
            }
            Type::Tuple(_) if self.types.namedtuple_definition(ty).is_none() => "tuple",
            Type::Class(id)
                if self
                    .types
                    .concrete_subclasses(id)
                    .iter()
                    .all(|child| *child == Type::Class(id)) =>
            {
                &self.classes.declarations.definitions[id].name
            }
            Type::Enum(id) => &self.classes.declarations.definitions[id].name,
            _ => return None,
        })
    }

    /// Empty containers still have to be evaluated when a call rejects their type.
    pub(super) fn evaluated_argument(&mut self, arg: &ast::Expr) -> Result<Expr, String> {
        match arg {
            ast::Expr::List(list) if list.elts.is_empty() => self.expr_expected(arg, Type::ListInt),
            ast::Expr::Dict(dict) if dict.items.is_empty() => {
                self.expr_expected(arg, self.types.dict(Type::Int, Type::Int))
            }
            _ => self.expr(arg),
        }
    }

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
            None,
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
        self.bind_supplied(
            call,
            params,
            receiver,
            None,
            None,
            |checker, index, argument| {
                let Supplied::Source(node) = argument else {
                    unreachable!("only source calls expand tuples")
                };
                check(checker, index, node)
            },
        )
    }

    pub(super) fn bind_supplied(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        receiver: Option<Expr>,
        mut scope: Option<&mut ArgumentScope>,
        default_owner: Option<(usize, usize)>,
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
        let pack = params
            .iter()
            .position(|p| p.kind == ParameterKind::Variadic);
        let mut packed = Vec::new();
        let mut forwarded = None;
        let mut extra = Vec::new();
        let mut positional = 0;
        for (at, arg) in call.arguments.args.iter().enumerate() {
            // A callee's *args has the same private, read-only storage as the
            // caller's pack. Reuse it when all fixed arguments have been bound.
            if let ast::Expr::Starred(star) = arg
                && let Some(index) = pack
                && index == positional
                && packed.is_empty()
                && at + 1 == call.arguments.args.len()
                && star
                    .value
                    .as_name_expr()
                    .is_some_and(|name| self.positional_pack.as_deref() == Some(name.id.as_str()))
            {
                let value = self.expr_expected(&star.value, params[index].ty)?;
                forwarded = Some(if let Some(scope) = scope.as_deref_mut() {
                    scope.hold(self, value)
                } else {
                    value
                });
                continue;
            }
            let mut bind = |checker: &mut Self,
                            input: Supplied<'_>,
                            scope: Option<&mut ArgumentScope>|
             -> Result<(), String> {
                let ordinary = params.get(positional).is_some_and(|parameter| {
                    matches!(
                        parameter.kind,
                        ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
                    )
                });
                if !ordinary {
                    let index = pack.ok_or_else(|| {
                        error(checker.source, arg, "too many positional arguments")
                    })?;
                    let element = checker
                        .types
                        .element(params[index].ty)
                        .expect("typed positional rest");
                    let value = input.checked(checker, element, false)?;
                    packed.push(if let Some(scope) = scope {
                        scope.hold(checker, value)
                    } else {
                        value
                    });
                    return Ok(());
                }
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
        if let Some(index) = pack {
            order[index + offset] = values.len();
            values.push(forwarded.unwrap_or_else(|| Expr {
                ty: params[index].ty,
                kind: ExprKind::List(packed),
            }));
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
            let fixed = params.iter().position(|p| {
                p.name == name
                    && !matches!(p.kind, ParameterKind::Variadic | ParameterKind::KeywordRest)
            });
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
                // A starred tuple can supply several parameters and keyword-rest has its
                // own operand. Record the actual omission at the point that copies it.
                if let Some(reads) = &mut self.default_reads {
                    if let Some((id, skip)) = default_owner
                        && matches!(default.kind, ExprKind::Int(_) | ExprKind::Bool(_))
                    {
                        reads.push((id, skip + index));
                    } else if !matches!(
                        &default.kind,
                        ExprKind::None | ExprKind::Str(_) | ExprKind::Bytes(_) | ExprKind::Float(_)
                    ) && !matches!(&default.kind, ExprKind::Absent(value) if matches!(value.kind, ExprKind::None))
                    {
                        // A composite default can embed a changed int or bool.
                        self.default_reads = None;
                    }
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
