//! Binds closed signatures without conflating argument evaluation and ABI order.

use std::ops::ControlFlow;

use super::*;

/// Evaluate the supplied call operands in order, then raise the checked TypeError.
pub(super) fn type_error(ty: Type, values: Vec<Expr>, message: String) -> Expr {
    type_error_message(ty, values, unpacking::string(message))
}

pub(super) fn type_error_message(ty: Type, mut values: Vec<Expr>, message: Expr) -> Expr {
    values.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(message)),
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

impl ParameterKind {
    pub(super) fn fixed(parameters: &ast::Parameters, index: usize) -> Self {
        if index < parameters.posonlyargs.len() {
            Self::PositionalOnly
        } else if index < parameters.posonlyargs.len() + parameters.args.len() {
            Self::PositionalOrKeyword
        } else {
            Self::KeywordOnly
        }
    }

    pub(super) fn accepts_positional(self) -> bool {
        matches!(self, Self::PositionalOnly | Self::PositionalOrKeyword)
    }

    /// An individual parameter can be known even when another slot is missing
    /// or follows a spread. Inference and dormant lexical checks use Ruff's
    /// partial lookups without claiming that the entire call binds.
    pub(super) fn known_argument<'a>(
        self,
        arguments: &'a ast::Arguments,
        index: usize,
        name: &str,
    ) -> Option<&'a ast::Expr> {
        match self {
            Self::PositionalOnly => arguments.find_positional(index),
            Self::PositionalOrKeyword => arguments.find_argument_value(name, index),
            Self::KeywordOnly => arguments.find_keyword(name).map(|kw| &kw.value),
            Self::Variadic | Self::KeywordRest => None,
        }
    }

    /// Construct a parameter for a builtin call signature.
    pub(super) fn parameter(self, name: &str, ty: Type, default: Option<Expr>) -> Parameter {
        Parameter {
            name: name.into(),
            ty,
            kind: self,
            default,
            sign_only: false,
        }
    }
}

pub(crate) fn known_declared_argument<'a>(
    arguments: &'a ast::Arguments,
    parameters: &ast::Parameters,
    index: usize,
    name: &str,
) -> Option<&'a ast::Expr> {
    ParameterKind::fixed(parameters, index).known_argument(arguments, index, name)
}

/// Bind an explicit keyword before checking its value. A positional-only name
/// can still be collected by **kwargs without occupying its positional slot.
fn keyword_slot<'a>(
    parameters: impl Iterator<Item = (&'a str, ParameterKind)>,
    name: &str,
    rest: Option<usize>,
    occupied: impl FnOnce(usize) -> bool,
) -> std::result::Result<usize, String> {
    let fixed = parameters.enumerate().find(|(_, (parameter, kind))| {
        *parameter == name && !matches!(kind, ParameterKind::Variadic | ParameterKind::KeywordRest)
    });
    match (fixed, rest) {
        (Some((index, (_, kind))), _) if kind != ParameterKind::PositionalOnly => {
            if occupied(index) {
                Err(format!("multiple values for argument {name:?}"))
            } else {
                Ok(index)
            }
        }
        (_, Some(index)) => Ok(index),
        (Some(_), None) => Err(format!("parameter {name:?} is positional-only")),
        (None, None) => Err(format!("unexpected keyword argument {name:?}")),
    }
}

/// Bind supplied values in evaluation order, leaving omitted slots for the caller. Checked
/// lowering and the initializer proof use different value consumers for the same
/// slots. Checking one supplied value can fail before a later binding error.
fn bind_fixed<'a, P, E, B>(
    call: &'a ast::ExprCall,
    parameters: &[P],
    shape: impl Fn(&P) -> (&str, ParameterKind),
    offset: usize,
    mut bind: impl FnMut(usize, &'a ast::Expr) -> std::result::Result<ControlFlow<B>, E>,
    error: impl Fn(TextRange, String) -> E,
) -> std::result::Result<ControlFlow<B, Vec<usize>>, E> {
    let mut order = vec![usize::MAX; parameters.len() + offset];
    if offset > 0 {
        order[0] = 0;
    }
    for (position, supplied) in call.arguments.iter_source_order().enumerate() {
        let index = match supplied {
            ast::ArgOrKeyword::Arg(value) => {
                if !parameters
                    .get(position)
                    .is_some_and(|p| shape(p).1.accepts_positional())
                {
                    return Err(error(value.range(), "too many positional arguments".into()));
                }
                position
            }
            ast::ArgOrKeyword::Keyword(keyword) => {
                let name = keyword.arg.as_deref().expect("fixed keyword");
                keyword_slot(parameters.iter().map(&shape), name, None, |i| {
                    order[i + offset] != usize::MAX
                })
                .map_err(|reason| error(keyword.range(), reason))?
            }
        };
        if let ControlFlow::Break(stop) = bind(index, supplied.value())? {
            return Ok(ControlFlow::Break(stop));
        }
        order[index + offset] = position + offset;
    }
    Ok(ControlFlow::Continue(order))
}

/// Return the explicit fixed operands of a proved source declaration. Dynamic
/// expansion or collection by a variadic parameter stays with the actual call.
pub(crate) fn fixed_call_arguments<'a>(
    call: &'a ast::ExprCall,
    parameters: &'a ast::Parameters,
) -> Option<Vec<(&'a ast::ParameterWithDefault, &'a ast::Expr)>> {
    if has_unpacking(call) {
        return None;
    }
    let fixed: Vec<_> = parameters
        .iter_non_variadic_params()
        .enumerate()
        .map(|(i, p)| (p, ParameterKind::fixed(parameters, i)))
        .collect();
    let mut bound = Vec::with_capacity(call.arguments.len());
    let order = bind_fixed(
        call,
        &fixed,
        |(p, k)| (p.name().as_str(), *k),
        0,
        |index, value| {
            bound.push((fixed[index].0, value));
            Ok(ControlFlow::<()>::Continue(()))
        },
        |_, _| (),
    )
    .ok()?
    .continue_value()?;
    if fixed
        .iter()
        .zip(order)
        .any(|((p, _), slot)| slot == usize::MAX && p.default.is_none())
    {
        return None;
    }
    Some(bound)
}

/// A builtin parameter that accepts either a positional or a keyword argument.
pub(super) fn parameter(name: &str, ty: Type, default: Option<Expr>) -> Parameter {
    ParameterKind::PositionalOrKeyword.parameter(name, ty, default)
}

#[derive(Clone)]
pub(super) struct Parameter {
    pub name: String,
    pub ty: Type,
    pub kind: ParameterKind,
    pub default: Option<Expr>,
    pub sign_only: bool,
}

/// A source call binds the declaration's parameters after any bound receiver.
pub(super) enum CallParameters<'a> {
    Source {
        signature: &'a Signature,
        skip: usize,
    },
    Slice(&'a [Parameter]),
}

/// Defaults have no user-visible effects or mutable identity. Checking them at
/// definition time also rejects unsupported defaults in never-called functions.
/// Only one optional sign on a numeric literal is accepted as an expression.
pub(super) fn scalar_default(
    expression: &ast::Expr,
    expected: Type,
    types: &Types,
    source: &str,
) -> std::result::Result<Expr, String> {
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
    Err(diagnostic_message(
        source,
        expression,
        &format!("default requires exact type {expected}, got {ty}"),
    ))
}

/// A scalar default also supplies an exact type for an unannotated parameter.
pub(crate) fn scalar_literal(
    expression: &ast::Expr,
    source: &str,
) -> std::result::Result<Expr, String> {
    if let Some(literal) = literals::signed_number(expression, |ty| ty != Type::PythonObject) {
        return Ok(literal);
    }
    let (ty, kind) = match expression {
        ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
            value: ast::Number::Int(value),
            ..
        }) => (Type::Int, ExprKind::Int(value.to_string())),
        ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
            value: ast::Number::Float(value),
            ..
        }) => (Type::Float, ExprKind::Float(*value)),
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
            return Err(diagnostic_message(
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
        declaration: Option<TypeDeclaration>,
    ) -> Result<Expr, String> {
        self.check_with(
            checker,
            ty,
            sign_only,
            declaration,
            |checker, value, ty, site| checker.direct_value(value, site, ty, sign_only),
        )
    }

    /// Check an operand in context. The destination sees only its evaluated value.
    pub(super) fn check_with(
        self,
        checker: &mut Checker<'_>,
        ty: Type,
        sign_only: bool,
        declaration: Option<TypeDeclaration>,
        coerce: impl Fn(&Checker<'_>, Expr, Type, &ast::Expr) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        if let Self::Source(node) = self
            && !sign_only
        {
            return checker.expr_in_context(node, ty, |_, _| false, coerce, declaration);
        }
        let node = self.site();
        let value = self.value(checker)?;
        coerce(checker, value, ty, node)
            .map_err(|error| error.expected_from(declaration, node.range()))
    }
}

/// Roots source expressions in Python evaluation order while expanding fixed tuples.
#[derive(Default)]
pub(super) struct ArgumentScope {
    pub(super) body: Vec<Statement>,
    pub(super) clear: Vec<usize>,
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

    /// An eager operand raised before invocation. Root the evaluated prefix
    /// in call order and release it in stack order when the last operand unwinds.
    pub(super) fn stopped(
        checker: &mut Checker<'_>,
        prefix: Vec<Expr>,
        stop: Expr,
        returns: Type,
    ) -> Expr {
        let mut scope = Self::default();
        for value in prefix {
            scope.hold(checker, value);
        }
        scope.clear.reverse();
        scope.body.push(Statement::Expr(stop));
        // As for an unbound read, this slot supplies only the unreachable result layout.
        let unreachable = checker.hidden(returns);
        scope.clear.push(unreachable);
        scope.finish(checker.local(unreachable))
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
/// Split off a single mapping only if every keyword belongs to the typed rest parameter.
pub(super) fn rest_keywords<'a>(
    call: &'a ast::ExprCall,
    params: &[Parameter],
) -> (&'a [ast::Keyword], Option<&'a ast::Expr>) {
    let keywords = call.arguments.keywords.as_ref();
    if let [spread] = keywords
        && spread.arg.is_none()
        && !starred(call)
        && let Some((rest, positional)) = params.split_last()
        && rest.kind == ParameterKind::KeywordRest
        && positional
            .iter()
            .all(|p| p.kind == ParameterKind::PositionalOnly)
    {
        return (&[], Some(&spread.value));
    }
    (keywords, None)
}

/// A resolved native function can bind fixed named parameters from one trailing mapping.
pub(super) fn named_mapping<'a>(
    call: &'a ast::ExprCall,
    params: &[Parameter],
) -> Option<&'a ast::Expr> {
    let (last, explicit) = call.arguments.keywords.split_last()?;
    (last.arg.is_none()
        && !starred(call)
        && explicit.iter().all(|kw| kw.arg.is_some())
        && params.iter().any(|p| {
            matches!(
                p.kind,
                ParameterKind::PositionalOrKeyword | ParameterKind::KeywordOnly
            )
        }))
    .then_some(&last.value)
}

pub(super) fn unpacked_keywords(call: &ast::ExprCall) -> bool {
    call.arguments
        .keywords
        .iter()
        .any(|keyword| keyword.arg.is_none())
}

pub(super) fn has_unpacking(call: &ast::ExprCall) -> bool {
    starred(call) || unpacked_keywords(call)
}

pub(super) const UNSUPPORTED_UNPACKING: &str = "argument unpacking is unsupported";

/// Every dispatch route uses a closed signature, including methods reached
/// through iterable consumers before ordinary call-expression lowering.
pub(super) fn reject_unpacking(call: &ast::ExprCall, source: &str) -> Result<(), String> {
    if has_unpacking(call) {
        return Err(error(source, call, UNSUPPORTED_UNPACKING));
    }
    Ok(())
}

impl Checker<'_> {
    /// Format an unexpected keyword using the runtime's CPython spelling rules.
    pub(super) fn keyword_error_message(
        &self,
        prefix: String,
        key: impl Into<String>,
        candidates: Vec<Expr>,
    ) -> Expr {
        let tuple = Expr {
            ty: self.types.tuple(vec![Type::Str; candidates.len()]),
            kind: ExprKind::Tuple(candidates),
        };
        Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::ExceptionKeywordSuggestion,
                vec![unpacking::string(prefix), unpacking::string(key), tuple],
            ),
        }
    }

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
        self.positional_argument_error(call, &[parameter], returns, unpacking::string(message))
    }

    pub(super) fn positional_argument_error(
        &mut self,
        call: &ast::ExprCall,
        parameters: &[&str],
        returns: Type,
        message: Expr,
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
        Ok(type_error_message(returns, values, message))
    }

    /// Return a Python class name only when the concrete receiver type is known.
    pub(super) fn exact_argument_type_name(&self, ty: Type) -> Option<&str> {
        Some(match ty {
            Type::None => "NoneType",
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::ByteArray => "bytearray",
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
        if let Some(ty) = self.empty_argument_type(arg) {
            self.expr_expected(arg, ty)
        } else {
            self.expr(arg)
        }
    }

    pub(super) fn empty_argument_type(&self, arg: &ast::Expr) -> Option<Type> {
        match arg {
            ast::Expr::List(list) if list.elts.is_empty() => Some(Type::ListInt),
            ast::Expr::Dict(dict) if dict.items.is_empty() => {
                Some(self.types.dict(Type::Int, Type::Int))
            }
            _ => None,
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
            &mut |checker, index, argument| {
                argument.checked(checker, params[index].ty, params[index].sign_only, None)
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
            &mut |checker, index, argument| {
                let Supplied::Source(node) = argument else {
                    unreachable!("only source calls expand tuples")
                };
                check(checker, index, node)
            },
        )
    }

    /// Keep one native binder for the different operand-lowering callbacks.
    pub(super) fn bind_supplied(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        receiver: Option<Expr>,
        scope: Option<&mut ArgumentScope>,
        check: &mut dyn FnMut(&mut Self, usize, Supplied<'_>) -> Result<Expr, String>,
    ) -> Result<ir::CallArgs, String> {
        Ok(self
            .bind_eager_arguments(
                call,
                CallParameters::Slice(params),
                receiver,
                scope,
                None,
                check,
            )?
            .continue_value()
            .expect("only direct source calls can stop during binding"))
    }

    pub(super) fn bind_eager_arguments(
        &mut self,
        call: &ast::ExprCall,
        parameters: CallParameters<'_>,
        receiver: Option<Expr>,
        mut scope: Option<&mut ArgumentScope>,
        stop_return: Option<Type>,
        check: &mut dyn FnMut(&mut Self, usize, Supplied<'_>) -> Result<Expr, String>,
    ) -> Result<ControlFlow<Expr, ir::CallArgs>, String> {
        let (params, source_signature) = match parameters {
            CallParameters::Slice(params) => (params, None),
            CallParameters::Source { signature, skip } => {
                (&signature.params[skip..], Some((signature, skip)))
            }
        };
        let default_owner = source_signature.map(|(sig, skip)| (sig.id, skip));
        let declaration = |index| {
            source_signature.and_then(|(sig, skip)| sig.parameter_declaration(index + skip))
        };
        let mut check = |checker: &mut Self, index, input: Supplied<'_>| {
            let range = input.site().range();
            check(checker, index, input)
                .map_err(|error| error.expected_from(declaration(index), range))
        };
        let offset = usize::from(receiver.is_some());
        let mut values = Vec::new();
        if let Some(value) = receiver {
            values.push(if let Some(scope) = scope.as_deref_mut() {
                scope.hold(self, value)
            } else {
                value
            });
        }
        let rest = params
            .iter()
            .position(|p| p.kind == ParameterKind::KeywordRest);
        let pack = params
            .iter()
            .position(|p| p.kind == ParameterKind::Variadic);
        if pack.is_none() && rest.is_none() && !has_unpacking(call) {
            let source = self.source;
            let order = bind_fixed(
                call,
                params,
                |p| (&p.name, p.kind),
                offset,
                |index, supplied| {
                    let value = check(self, index, Supplied::Source(supplied))?;
                    if let Some(returns) = stop_return
                        && self.terminating_operand(&value)
                    {
                        let stop = if values.is_empty() {
                            value
                        } else {
                            ArgumentScope::stopped(
                                self,
                                std::mem::take(&mut values),
                                value,
                                returns,
                            )
                        };
                        return Ok(ControlFlow::Break(stop));
                    }
                    let value = if let Some(scope) = scope.as_deref_mut() {
                        scope.hold(self, value)
                    } else {
                        value
                    };
                    values.push(value);
                    Ok(ControlFlow::Continue(()))
                },
                |at, msg| error(source, &at, &msg),
            )?;
            let order = match order {
                ControlFlow::Continue(order) => order,
                ControlFlow::Break(stop) => return Ok(ControlFlow::Break(stop)),
            };
            return self.complete_call_arguments(call, params, values, order, default_owner);
        }
        let mut order = vec![usize::MAX; params.len() + offset];
        if offset > 0 {
            order[0] = 0;
        }
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
                && self.types.element(params[index].ty).is_some()
                && packed.is_empty()
                && at + 1 == call.arguments.args.len()
                && star
                    .value
                    .as_name_expr()
                    .is_some_and(|name| self.positional_pack.as_deref() == Some(name.id.as_str()))
            {
                let value =
                    self.expr_expected_from(&star.value, params[index].ty, declaration(index))?;
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
                let ordinary = params
                    .get(positional)
                    .is_some_and(|p| p.kind.accepts_positional());
                if !ordinary {
                    let index = pack.ok_or_else(|| {
                        error(checker.source, arg, "too many positional arguments")
                    })?;
                    let value = if let Some(element) = checker.types.element(params[index].ty) {
                        input.checked(checker, element, false, declaration(index))?
                    } else if default_owner.is_none() {
                        check(checker, index, input)?
                    } else {
                        input.value(checker)?
                    };
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
                    .ok_or_else(|| error(self.source, arg, UNSUPPORTED_UNPACKING))?;
                for field in scope.spread(self, star)? {
                    bind(self, Supplied::Field(field, arg), Some(scope))?;
                }
            } else {
                bind(self, Supplied::Source(arg), scope.as_deref_mut())?;
            }
        }
        if let Some(index) = pack {
            order[index + offset] = values.len();
            let value = if let Some(forwarded) = forwarded {
                forwarded
            } else if self.types.element(params[index].ty).is_some() {
                Expr {
                    ty: params[index].ty,
                    kind: ExprKind::List(packed),
                }
            } else {
                let ty = self.types.tuple(packed.iter().map(|arg| arg.ty).collect());
                // A source function shares its inferred pack across calls. A builtin
                // consumes this call's checked tuple directly.
                if let Some((function, skip)) = default_owner {
                    let expected = if let Some(inference) = self.inference {
                        inference.learn_parameter(function, index + skip, ty);
                        inference.parameter(function, index + skip)
                    } else {
                        Some(params[index].ty)
                    };
                    if expected != Some(ty) {
                        return Err(error(
                            self.source,
                            call,
                            "an unannotated variadic parameter requires the same tuple shape at every call",
                        ));
                    }
                }
                Expr {
                    ty,
                    kind: ExprKind::Tuple(packed),
                }
            };
            values.push(value);
        }
        let named_mapping = default_owner.and_then(|_| named_mapping(call, params));
        let (keywords, rest_mapping) = if named_mapping.is_some() {
            (
                &call.arguments.keywords[..call.arguments.keywords.len() - 1],
                None,
            )
        } else {
            rest_keywords(call, params)
        };
        for keyword in keywords {
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
            let index = keyword_slot(
                params.iter().map(|p| (p.name.as_str(), p.kind)),
                name,
                rest,
                |i| order[i + offset] != usize::MAX,
            )
            .map_err(|message| error(self.source, keyword, &message))?;
            if Some(index) == rest {
                let (_, element) = self
                    .types
                    .mapping(params[index].ty)
                    .expect("typed keyword rest");
                let value = self.expr_expected_from(&keyword.value, element, declaration(index))?;
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
            order[index + offset] = values.len();
            let value = check(self, index, Supplied::Source(&keyword.value))?;
            values.push(if let Some(scope) = scope.as_deref_mut() {
                scope.hold(self, value)
            } else {
                value
            });
        }
        let bound_mapping = if let Some(node) = named_mapping {
            let scope = scope.expect("resolved mapping calls root their operands");
            Some(self.bind_mapped_keywords(
                call,
                node,
                params,
                scope,
                &mut values,
                &mut order[offset..],
            )?)
        } else {
            None
        };
        if let Some(index) = rest {
            let (key, value) = self
                .types
                .mapping(params[index].ty)
                .expect("typed keyword rest");
            order[index + offset] = values.len();
            let dictionary = if let Some(mapping) = bound_mapping {
                if extra.is_empty() {
                    mapping
                } else {
                    let first = Expr {
                        ty: params[index].ty,
                        kind: ExprKind::dict_pairs(extra),
                    };
                    self.plain_dictionary(
                        ArgumentScope::default(),
                        (key, value),
                        vec![first, mapping],
                    )
                }
            } else if let Some(mapping) = rest_mapping {
                let input = self.expr(mapping)?;
                self.expect(params[index].ty, input.ty, mapping)?;
                let mut copy = ArgumentScope::default();
                let input = copy.hold(self, input);
                self.plain_dictionary(copy, (key, value), vec![input])
            } else {
                Expr {
                    ty: params[index].ty,
                    kind: ExprKind::dict_pairs(extra),
                }
            };
            values.push(dictionary);
        }
        self.complete_call_arguments(call, params, values, order, default_owner)
    }

    /// Supply omitted formals only on the native call, after all written operands have bound.
    fn complete_call_arguments(
        &mut self,
        call: &ast::ExprCall,
        params: &[Parameter],
        mut values: Vec<Expr>,
        mut order: Vec<usize>,
        default_owner: Option<(usize, usize)>,
    ) -> Result<ControlFlow<Expr, ir::CallArgs>, String> {
        let offset = order.len() - params.len();
        values.reserve_exact(order.len() - values.len());
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
                values.push(self.call_default(default, default_owner, index));
            }
        }
        Ok(ControlFlow::Continue(ir::CallArgs {
            values,
            order,
            site: ir::CallSite::None,
            exhaustive_enum_match: false,
        }))
    }

    fn call_default(
        &mut self,
        default: &Expr,
        default_owner: Option<(usize, usize)>,
        index: usize,
    ) -> Expr {
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
        default.clone()
    }

    /// Bind a live mapping after all earlier operands have acquired roots. The native
    /// binder writes to fresh formal and rest dictionaries in original key order.
    fn bind_mapped_keywords(
        &mut self,
        call: &ast::ExprCall,
        node: &ast::Expr,
        params: &[Parameter],
        scope: &mut ArgumentScope,
        values: &mut Vec<Expr>,
        order: &mut [usize],
    ) -> Result<Expr, String> {
        let rest = params
            .iter()
            .position(|p| p.kind == ParameterKind::KeywordRest);
        let Some(Binding::Definition(id)) = self.resolve(&call.func) else {
            return Err(error(
                self.source,
                call,
                "keyword mapping requires a resolved function",
            ));
        };
        let module = &self.graph.modules[id.module];
        let ast::Stmt::FunctionDef(function) = &module.parsed.syntax().body[id.statement] else {
            return Err(error(
                self.source,
                call,
                "keyword mapping requires a function definition",
            ));
        };
        let name = function.name.as_str().to_owned();
        let qualified = format!("{}.{name}", self.runtime_module_name(id.module));
        let input = self.expr(node)?;
        let (key, element) = self.types.mapping(input.ty).ok_or_else(|| {
            error(
                self.source,
                node,
                "keyword mapping requires a typed dictionary",
            )
        })?;
        self.expect(Type::Str, key, node)?;
        if let Some(index) = rest {
            let (_, target) = self
                .types
                .mapping(params[index].ty)
                .expect("typed keyword rest");
            self.expect(target, element, node)?;
        }
        let input = scope.hold(self, input);
        let empty = |key| Expr {
            ty: self.types.dict(key, element),
            kind: ExprKind::Dict(Vec::new()),
        };
        let fixed = empty(Type::Int);
        let rest = empty(Type::Str);
        let slots = scope.hold(self, fixed);
        let remainder = scope.hold(self, rest);
        let signature = self.keyword_signature(params);
        let explicit: Vec<_> = call
            .arguments
            .keywords
            .iter()
            .filter_map(|kw| kw.arg.as_deref())
            .map(unpacking::string)
            .collect();
        let explicit = Expr {
            ty: self.types.tuple(vec![Type::Str; explicit.len()]),
            kind: ExprKind::Tuple(explicit),
        };
        scope.body.push(Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(
                Builtin::BindSourceKeywords,
                vec![
                    input,
                    slots.clone(),
                    remainder.clone(),
                    unpacking::string(qualified),
                    signature,
                    explicit,
                    Expr::integer(call.arguments.args.len()),
                ],
            ),
        }));
        let fixed = params.iter().enumerate().filter(|(_, p)| {
            !matches!(p.kind, ParameterKind::Variadic | ParameterKind::KeywordRest)
        });
        for (slot, (index, parameter)) in fixed.enumerate() {
            if order[index] != usize::MAX
                || !matches!(
                    parameter.kind,
                    ParameterKind::PositionalOrKeyword | ParameterKind::KeywordOnly
                )
            {
                continue;
            }
            let key = Expr::integer(slot);
            let mapped = Expr {
                ty: element,
                kind: ExprKind::Builtin(Builtin::DictGet, vec![slots.clone(), key.clone()]),
            };
            let mapped = self.direct_value(mapped, node, parameter.ty, parameter.sign_only)?;
            let arg = if let Some(default) = &parameter.default {
                self.default_reads = None;
                if let ExprKind::Call(id, _) = &default.kind {
                    self.calls.insert(*id);
                }
                let has = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::DictContains, vec![key, slots.clone()]),
                };
                Expr {
                    ty: parameter.ty,
                    kind: ExprKind::Conditional(
                        Box::new(has),
                        Box::new(mapped),
                        Box::new(default.clone()),
                    ),
                }
            } else {
                mapped
            };
            order[index] = values.len();
            values.push(scope.hold(self, arg));
        }
        Ok(remainder)
    }

    /// A pooled, immutable declaration shared by all visits to this call. The
    /// fixed parameters precede the names with keyword-only defaults; positional
    /// defaults form a suffix. Actual arguments are separate IR operands.
    fn keyword_signature(&self, params: &[Parameter]) -> Expr {
        let pos = |p: &&Parameter| {
            matches!(
                p.kind,
                ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
            )
        };
        let fixed = |p: &&Parameter| {
            !matches!(p.kind, ParameterKind::KeywordRest | ParameterKind::Variadic)
        };
        let mut fields = vec![
            Expr::integer(
                params
                    .iter()
                    .take_while(|p| p.kind == ParameterKind::PositionalOnly)
                    .count(),
            ),
            Expr::integer(params.iter().filter(pos).count()),
            Expr::integer(params.iter().filter(fixed).count()),
            Expr::integer(
                params
                    .iter()
                    .filter(pos)
                    .filter(|p| p.default.is_some())
                    .count(),
            ),
        ];
        for kind in [ParameterKind::Variadic, ParameterKind::KeywordRest] {
            fields.push(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(params.iter().any(|p| p.kind == kind)),
            });
        }
        fields.extend(
            params
                .iter()
                .filter(fixed)
                .chain(
                    params
                        .iter()
                        .filter(|p| p.kind == ParameterKind::KeywordOnly && p.default.is_some()),
                )
                .map(|p| unpacking::string(&p.name)),
        );
        Expr {
            ty: self.types.tuple(fields.iter().map(|e| e.ty).collect()),
            kind: ExprKind::Tuple(fields),
        }
    }

    /// Keyword permutation happens only after supplied expressions have executed
    /// and acquired independent roots. Identity order needs no extra scope.
    pub(super) fn ordered_builtin(
        &mut self,
        operation: Builtin,
        returns: Type,
        mut arguments: ir::CallArgs,
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
        // Immediate values and initialized local reads in source order already have owners.
        let immediate = |value: &Expr| match &value.kind {
            ExprKind::Bool(_) | ExprKind::None => true,
            ExprKind::Absent(inner) => matches!(inner.kind, ExprKind::None),
            ExprKind::Int(_) => value.tagged_integer_literal().is_some(),
            _ => false,
        };
        let mut scope = ArgumentScope::default();
        let mut previous = None;
        let already_rooted = arguments.order.iter().all(|&index| {
            let value = &arguments.values[index];
            immediate(value)
                || matches!(value.kind, ExprKind::Local(slot) if self.assigned.contains(&slot))
                    && previous.replace(index).is_none_or(|last| last < index)
        });
        if !already_rooted {
            for value in &mut arguments.values {
                if !immediate(value) {
                    *value = scope.hold(self, std::mem::replace(value, Expr::none()));
                }
            }
        }
        scope.finish(Expr {
            ty: returns,
            kind: ExprKind::Builtin(operation, arguments.into_formal_order()),
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
