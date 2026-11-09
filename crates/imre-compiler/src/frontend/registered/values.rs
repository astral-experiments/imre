//! Lower values evaluated against a registered Python scope into the expression IR.
use super::*;
use crate::ir::DisplayKind;
use std::collections::BTreeSet;

pub(in crate::frontend) struct ModuleSource {
    method_imports: BTreeSet<String>,
    replacements: HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
}

impl ModuleSource {
    pub(in crate::frontend) fn new(module: &crate::modules::SourceModule) -> Self {
        let tokens = module.parsed.tokens();
        Self {
            method_imports: imre_ast::module_imports(&module.parsed.syntax().body),
            replacements: literal_replacements(&module.source, tokens),
            has_templates: tokens
                .iter()
                .any(|token| token.kind() == ast::token::TokenKind::TStringStart),
        }
    }
    pub(super) fn context<'a>(&'a self, text: &'a str) -> RegisteredSource<'a> {
        RegisteredSource {
            templates: None,
            text,
            replacements: &self.replacements,
            has_templates: self.has_templates,
            method_imports: &self.method_imports,
            private_names: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RegisteredSource<'a> {
    pub(crate) text: &'a str,
    pub(crate) replacements: &'a HashMap<TextRange, Vec<u32>>,
    pub(crate) has_templates: bool,
    pub(crate) templates: Option<&'a HashMap<TextRange, Expr>>,
    pub(crate) method_imports: &'a BTreeSet<String>,
    pub(crate) private_names: Option<&'a imre_ast::Names>,
}

impl RegisteredSource<'_> {
    /// Escaped braces may join several middle tokens into one literal element.
    /// Recover replacement code points in their original source order.
    pub(crate) fn format_replacements(
        self,
        literal: &ast::InterpolatedStringLiteralElement,
    ) -> Option<Vec<u32>> {
        let range = literal.range();
        self.replacements
            .keys()
            .any(|&r| range.contains_range(r))
            .then(|| imre_ast::strings::replacements(&self.text[range]).collect())
    }

    pub(crate) fn direct_method(self, call: &ast::ExprCall) -> Option<&ast::ExprAttribute> {
        imre_ast::direct_method(call, self.method_imports, self.private_names)
    }

    pub(crate) fn call(self, call: &ast::ExprCall) -> (ir::RegisteredCall, &ast::Expr) {
        let (target, operand) = if let Some(field) = self.direct_method(call) {
            (
                ir::RegisteredCallTarget::Method {
                    name: field.attr.to_string(),
                    attribute: field.range(),
                },
                field.value.as_ref(),
            )
        } else {
            (callable_target(call), call.func.as_ref())
        };
        (call_site(call, target), operand)
    }

    /// Select the operation and the source operands that feed it. A residual child
    /// uses its own lowering without sending the enclosing operation to the syntax emitter.
    pub(crate) fn object_operation(self, node: &ast::Expr) -> Option<ObjectOperation<'_>> {
        let (opcode, target, arguments) = match node {
            ast::Expr::Attribute(field) => (attribute_load(field), field.value.as_ref(), None),
            ast::Expr::UnaryOp(unary) => (
                Builtin::RegisteredUnary(unary.op.into()),
                unary.operand.as_ref(),
                None,
            ),
            ast::Expr::Call(call) => {
                let (site, target) = self.call(call);
                (Builtin::RegisteredCall(site), target, Some(&call.arguments))
            }
            _ => return None,
        };
        Some(ObjectOperation {
            opcode,
            target,
            arguments,
        })
    }

    /// Preserve the fused get only for one single-line partition method call with a constant index.
    pub(crate) fn partition_index(self, subscript: &ast::ExprSubscript) -> Option<u32> {
        let call = subscript.value.as_call_expr()?;
        let field = self.direct_method(call)?;
        if !matches!(field.attr.as_str(), "partition" | "rpartition")
            || !call.arguments.keywords.is_empty()
            || !matches!(call.arguments.args.as_ref(), [arg] if !arg.is_starred_expr())
            || self.text[subscript.range()].contains('\n')
        {
            return None;
        }
        let ast::Number::Int(index) = &subscript.slice.as_number_literal_expr()?.value else {
            return None;
        };
        let index = u32::try_from(index.as_i64()?).ok()?;
        (index < 3).then_some(index)
    }

    pub(super) fn located(self, node: &ast::Expr, value: Expr) -> Expr {
        let position = imre_ast::expression_position(node, self.method_imports, self.private_names);
        located(node.range(), position, value)
    }
}

pub(crate) struct ObjectOperation<'a> {
    pub(crate) opcode: Builtin,
    pub(crate) target: &'a ast::Expr,
    arguments: Option<&'a ast::Arguments>,
}

impl<'a> ObjectOperation<'a> {
    pub(crate) fn arguments(&self) -> impl Iterator<Item = &'a ast::Expr> + use<'a> {
        self.arguments.into_iter().flat_map(registered_arguments)
    }
}

pub(crate) fn literal_replacements(
    source: &str,
    tokens: &ast::token::Tokens,
) -> HashMap<TextRange, Vec<u32>> {
    let mut replacements = HashMap::new();
    for token in tokens {
        let Some(range) = imre_ast::strings::cooked_content(token) else {
            continue;
        };
        let points: Vec<_> = imre_ast::strings::replacements(&source[range]).collect();
        if points.iter().any(|&point| point != 0xfffd) {
            replacements.insert(token.range(), points);
        }
    }
    replacements
}

pub(crate) fn push_codepoints(bytes: &mut Vec<u8>, text: &str, replacements: Option<&[u32]>) {
    let mut points = replacements.into_iter().flatten();
    for c in text.chars() {
        let point = if c == char::REPLACEMENT_CHARACTER && replacements.is_some() {
            *points
                .next()
                .expect("one original code point per replacement")
        } else {
            u32::from(c)
        };
        bytes.extend_from_slice(&point.to_le_bytes());
    }
    debug_assert!(points.next().is_none());
}

pub(crate) fn registered_string<'a>(
    text: &str,
    parts: impl Iterator<Item = &'a ast::StringLiteral> + Clone,
    replacements: &HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
) -> Expr {
    let kind = if !has_templates
        && parts
            .clone()
            .all(|part| !replacements.contains_key(&part.range()))
    {
        ExprKind::Str(text.to_owned())
    } else {
        let mut bytes = Vec::new();
        for part in parts {
            push_codepoints(
                &mut bytes,
                part,
                replacements.get(&part.range()).map(Vec::as_slice),
            );
        }
        ExprKind::Builtin(Builtin::RegisteredString(bytes), vec![])
    };
    Expr {
        ty: Type::PythonObject,
        kind,
    }
}

pub(in crate::frontend) fn name_binding(id: &str) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(Builtin::RegisteredName(id.to_owned()), vec![]),
    }
}

/// Fixed calls use bounded entry-block scratch. Expanded or larger calls retain
/// their positional and keyword layout for incremental packing.
pub(in crate::frontend) fn registered_argument_layout(
    args: &ast::Arguments,
) -> ir::RegisteredArguments {
    let positional = args.args.len();
    if positional + args.keywords.len() <= 64
        && !args.args.iter().any(ast::Expr::is_starred_expr)
        && args.keywords.iter().all(|key| key.arg.is_some())
    {
        if args.keywords.is_empty() {
            return ir::RegisteredArguments::Positional;
        }
        return ir::RegisteredArguments::Keywords {
            positional,
            names: args
                .keywords
                .iter()
                .map(|key| key.arg.as_ref().unwrap().to_string())
                .collect(),
        };
    }
    ir::RegisteredArguments::Packed(packed_arguments(args))
}

pub(crate) fn packed_arguments(args: &ast::Arguments) -> ir::PackedArguments {
    ir::PackedArguments {
        starred: args.args.iter().map(ast::Expr::is_starred_expr).collect(),
        keywords: args
            .keywords
            .iter()
            .map(|key| key.arg.as_ref().map(ToString::to_string))
            .collect(),
    }
}

fn positional_elements(args: &[ast::Expr]) -> impl Iterator<Item = (bool, &ast::Expr)> {
    args.iter().map(|arg| match arg {
        ast::Expr::Starred(star) => (true, star.value.as_ref()),
        value => (false, value),
    })
}

pub(crate) fn registered_arguments(args: &ast::Arguments) -> impl Iterator<Item = &ast::Expr> {
    positional_elements(&args.args)
        .map(|(_, value)| value)
        .chain(args.keywords.iter().map(|key| &key.value))
}

impl From<ast::UnaryOp> for ir::ObjectUnary {
    fn from(op: ast::UnaryOp) -> Self {
        match op {
            ast::UnaryOp::UAdd => Self::Positive,
            ast::UnaryOp::USub => Self::Negative,
            ast::UnaryOp::Invert => Self::Invert,
            ast::UnaryOp::Not => Self::Not,
        }
    }
}

fn located(range: TextRange, position: ruff_text_size::TextSize, value: Expr) -> Expr {
    super::operation(
        Builtin::RegisteredLocation(range, position),
        value.ty,
        vec![value],
    )
}

pub(in crate::frontend) fn loaded_attribute(field: &ast::ExprAttribute, receiver: Expr) -> Expr {
    let value = super::operation(attribute_load(field), Type::PythonObject, vec![receiver]);
    located(field.range(), field.attr.start(), value)
}

fn attribute_load(field: &ast::ExprAttribute) -> Builtin {
    Builtin::RegisteredAttribute {
        action: ir::AttributeAction::Load,
        name: field.attr.to_string(),
        range: field.range(),
    }
}

fn callable_target(call: &ast::ExprCall) -> ir::RegisteredCallTarget {
    ir::RegisteredCallTarget::Callable {
        end: call.func.end(),
        suspended_name: call.func.is_name_expr().then(|| call.func.range()),
    }
}

fn call_site(call: &ast::ExprCall, target: ir::RegisteredCallTarget) -> ir::RegisteredCall {
    ir::RegisteredCall {
        range: call.range(),
        target,
        arguments: registered_argument_layout(&call.arguments),
        explicit_scope: false,
    }
}

pub(in crate::frontend) fn fixed_positional_arguments(call: &ast::ExprCall) -> bool {
    matches!(
        registered_argument_layout(&call.arguments),
        ir::RegisteredArguments::Positional
    )
}

pub(in crate::frontend) fn object_call(call: &ast::ExprCall, operands: Vec<Expr>) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(
            Builtin::RegisteredCall(call_site(call, callable_target(call))),
            operands,
        ),
    }
}

pub(in crate::frontend) fn call_with_target<E>(
    call: &ast::ExprCall,
    target: Expr,
    mut operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    let mut operands =
        Vec::with_capacity(1 + call.arguments.args.len() + call.arguments.keywords.len());
    operands.push(target);
    for arg in registered_arguments(&call.arguments) {
        operands.push(operand(arg)?);
    }
    Ok(object_call(call, operands))
}

fn number_literal(number: &ast::Number, minus: bool) -> ExprKind {
    match number {
        ast::Number::Int(n) => {
            let mut text = n.to_string();
            if minus && text != "0" {
                text.insert(0, '-');
            }
            ExprKind::Int(text)
        }
        ast::Number::Float(n) => ExprKind::Float(if minus { -*n } else { *n }),
        ast::Number::Complex { real, imag } => {
            let (real, imag) = if minus {
                (-*real, -*imag)
            } else {
                (*real, *imag)
            };
            ExprKind::Builtin(Builtin::RegisteredComplex(real, imag), vec![])
        }
    }
}

/// Identify the sequence operation even when an individual operand still needs its source adapter.
pub(crate) fn display_elements(node: &ast::Expr) -> Option<(&[ast::Expr], DisplayKind)> {
    match node {
        ast::Expr::List(list) => Some((&list.elts, DisplayKind::List)),
        ast::Expr::Tuple(tuple) => Some((&tuple.elts, DisplayKind::Tuple)),
        ast::Expr::Set(set) => Some((&set.elts, DisplayKind::Set)),
        _ => None,
    }
}

/// An index constant is identified by typed literal content, including the sign of zero.
/// Fold only literal index operands. Dynamic bounds and starred elements still run in order.
pub(in crate::frontend) fn index_key<E>(
    node: &ast::Expr,
    operand: &mut impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    use ir::Constant as C;
    let (value, constant) = match node {
        ast::Expr::Slice(slice) => {
            let mut bounds = Vec::with_capacity(3);
            let mut literal = true;
            for bound in [&slice.lower, &slice.upper, &slice.step] {
                bounds.push(match bound.as_deref() {
                    Some(bound) => {
                        literal &= bound.is_literal_expr();
                        index_key(bound, operand)?
                    }
                    None => Expr::none(),
                });
            }
            let constant = literal
                .then(|| {
                    Some(C::Slice(Box::new([
                        ir::Constant::from_expr(&bounds[0])?,
                        ir::Constant::from_expr(&bounds[1])?,
                        ir::Constant::from_expr(&bounds[2])?,
                    ])))
                })
                .flatten();
            if slice.step.is_none() {
                bounds.pop();
            }
            (
                super::operation(Builtin::RegisteredSlice, Type::PythonObject, bounds),
                constant,
            )
        }
        ast::Expr::Tuple(tuple) => {
            let values = tuple
                .elts
                .iter()
                .map(|item| {
                    if let Some(starred) = item.as_starred_expr() {
                        let value = index_key(&starred.value, operand)?;
                        Ok(super::operation(
                            Builtin::RegisteredSpread,
                            Type::PythonObject,
                            vec![value],
                        ))
                    } else {
                        index_key(item, operand)
                    }
                })
                .collect::<StdResult<Vec<_>, E>>()?;
            let value = Expr {
                ty: Type::PythonObject,
                kind: ExprKind::Tuple(values),
            };
            let constant = ir::Constant::from_expr(&value);
            (value, constant)
        }
        _ => {
            let value = if let Some(op) = node.as_unary_op_expr()
                && matches!(op.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && let Some(n) = op.operand.as_number_literal_expr()
            {
                Expr {
                    ty: Type::PythonObject,
                    kind: number_literal(&n.value, op.op == ast::UnaryOp::USub),
                }
            } else {
                operand(node)?
            };
            let Some(key) = ir::Constant::from_expr(&value) else {
                return Ok(value);
            };
            if matches!(key, C::None | C::Ellipsis | C::Bool(_))
                || matches!(&key, C::Int(text) if text.parse::<i64>().is_ok_and(crate::dynamic::is_permanent_integer))
            {
                return Ok(value);
            }
            let value = match value.kind {
                ExprKind::Builtin(Builtin::RegisteredLocation(..), mut args) => args.remove(0),
                _ => value,
            };
            (value, Some(key))
        }
    };
    let value = if let Some(key) = constant {
        // Permanent singleton handles already have their Python identity.
        match key {
            C::None | C::Ellipsis | C::Bool(_) => value,
            _ => super::operation(
                Builtin::RegisteredIndexConstant(key),
                Type::PythonObject,
                vec![value],
            ),
        }
    } else {
        value
    };
    Ok(located(node.range(), node.start(), value))
}

/// Retain the same checked key when the subscript receiver or its containing statement is residual.
pub(crate) fn registered_constant_key(
    node: &ast::Expr,
    source: RegisteredSource<'_>,
) -> Option<(ir::Constant, Expr)> {
    let value = index_key(node, &mut |e| registered_value(e, source).ok_or(())).ok()?;
    match &value.kind {
        ExprKind::Builtin(Builtin::RegisteredLocation(..), args) => match &args[0].kind {
            ExprKind::Builtin(Builtin::RegisteredIndexConstant(key), _) => {
                Some((key.clone(), value))
            }
            _ => None,
        },
        _ => None,
    }
}

fn display_operands<E>(
    items: &[ast::Expr],
    source: RegisteredSource<'_>,
    child: &mut impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Vec<Expr>, E> {
    items
        .iter()
        .map(|item| {
            if let ast::Expr::Starred(star) = item {
                Ok(Expr {
                    ty: Type::PythonObject,
                    kind: ExprKind::Builtin(Builtin::RegisteredSpread, vec![child(&star.value)?]),
                })
            } else if let ast::Expr::UnaryOp(op) = item
                && matches!(op.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && let ast::Expr::NumberLiteral(n) = &*op.operand
            {
                // The parser keeps the sign separate. Only an exact literal has no user slot.
                Ok(source.located(
                    item,
                    Expr {
                        ty: match n.value {
                            ast::Number::Int(_) => Type::Int,
                            ast::Number::Float(_) => Type::Float,
                            ast::Number::Complex { .. } => Type::PythonObject,
                        },
                        kind: number_literal(&n.value, op.op == ast::UnaryOp::USub),
                    },
                ))
            } else {
                child(item)
            }
        })
        .collect()
}

/// Select one registered-frame operation before requesting its operands in source order.
pub(super) fn operation<E>(
    node: &ast::Expr,
    source: RegisteredSource<'_>,
    local: Option<Expr>,
    dynamic_receiver: impl FnOnce(&ast::Expr) -> bool,
    mut child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Option<Expr>, E> {
    if let Some((items, display)) = display_elements(node) {
        let values = display_operands(items, source, &mut child)?;
        return Ok(Some(Expr {
            ty: Type::PythonObject,
            kind: match display {
                DisplayKind::List => ExprKind::List(values),
                DisplayKind::Tuple => ExprKind::Tuple(values),
                DisplayKind::Set => ExprKind::Set {
                    element: Type::PythonObject,
                    values,
                },
            },
        }));
    }
    if let Some(selected) = source.object_operation(node) {
        if !matches!(
            selected.opcode,
            Builtin::RegisteredCall(ir::RegisteredCall {
                target: ir::RegisteredCallTarget::Callable { .. },
                ..
            })
        ) && !dynamic_receiver(selected.target)
        {
            return Ok(None);
        }
        let target = child(selected.target)?;
        if target.ty != Type::PythonObject {
            return Ok(None);
        }
        let mut operands = vec![target];
        for arg in selected.arguments() {
            operands.push(child(arg)?);
        }
        return Ok(Some(Expr {
            ty: Type::PythonObject,
            kind: ExprKind::Builtin(selected.opcode, operands),
        }));
    }
    let kind = match node {
        ast::Expr::If(branch) => ExprKind::Conditional(
            Box::new(child(&branch.test)?),
            Box::new(child(&branch.body)?),
            Box::new(child(&branch.orelse)?),
        ),
        ast::Expr::BinOp(binary) if binary.op == ast::Operator::Add => ExprKind::Binary(
            Binary::Add,
            Box::new(child(&binary.left)?),
            Box::new(child(&binary.right)?),
        ),
        ast::Expr::Name(name) => {
            if let Some(value) = local {
                return Ok(Some(value));
            }
            return Ok(Some(name_binding(&name.id)));
        }
        ast::Expr::NumberLiteral(lit) => number_literal(&lit.value, false),
        ast::Expr::TString(_) => {
            return Ok(source
                .templates
                .and_then(|sites| sites.get(&node.range()))
                .cloned());
        }
        ast::Expr::StringLiteral(lit) => {
            return Ok(Some(registered_string(
                lit.value.to_str(),
                lit.value.iter(),
                source.replacements,
                source.has_templates,
            )));
        }
        ast::Expr::BytesLiteral(lit) => ExprKind::Bytes(lit.value.bytes().collect()),
        ast::Expr::BooleanLiteral(lit) => ExprKind::Bool(lit.value),
        ast::Expr::NoneLiteral(_) => ExprKind::None,
        ast::Expr::EllipsisLiteral(_) => ExprKind::Builtin(Builtin::RegisteredEllipsis, vec![]),
        ast::Expr::Dict(dict) => {
            let mut entries = Vec::with_capacity(dict.items.len());
            let mut operands = Vec::new();
            for item in &dict.items {
                if let Some(key) = &item.key {
                    entries.push(ir::DictEntryKind::Pair);
                    operands.push(child(key)?);
                } else {
                    entries.push(ir::DictEntryKind::Unpack);
                }
                operands.push(child(&item.value)?);
            }
            ExprKind::Builtin(Builtin::RegisteredDict(entries), operands)
        }
        ast::Expr::Subscript(index) if source.partition_index(index).is_none() => {
            ExprKind::Builtin(
                Builtin::RegisteredSubscript(node.range(), index.value.end()),
                vec![child(&index.value)?, index_key(&index.slice, &mut child)?],
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(Expr {
        ty: Type::PythonObject,
        kind,
    }))
}

pub(crate) fn registered_index_key(key: &ast::Expr, source: RegisteredSource<'_>) -> Option<Expr> {
    index_key(key, &mut |e| registered_value(e, source).ok_or(())).ok()
}

pub(crate) fn lambda_defaults(lambda: &ast::ExprLambda) -> impl Iterator<Item = &ast::Expr> {
    lambda.parameters.iter().flat_map(|parameters| {
        parameters
            .iter_non_variadic_params()
            .filter_map(|p| p.default.as_deref())
    })
}

fn lambda_binding<E>(
    lambda: &ast::ExprLambda,
    operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    let defaults = lambda_defaults(lambda)
        .map(operand)
        .collect::<StdResult<_, _>>()?;
    Ok(super::operation(
        Builtin::RegisteredLambda(lambda.start(), false),
        Type::PythonObject,
        defaults,
    ))
}

/// A checked instruction can bind only a wholly prepared function. This also checks
/// each nested lambda before its binding can appear in a default or return.
pub(super) fn checked_binding(
    lambda: &ast::ExprLambda,
    source: RegisteredSource<'_>,
) -> Option<Expr> {
    Region::Lambda.operand(&lambda.body, source)?;
    lambda_binding(lambda, |default| {
        Region::Lambda.operand(default, source).ok_or(())
    })
    .ok()
}

#[derive(Clone, Copy)]
pub(super) enum Region {
    /// Residual expressions encounter a prepared lambda binding at the source site.
    Residual,
    Suite,
    /// Suspensions within a lambda's body or defaults use the enclosing native activation.
    Lambda,
}

impl Region {
    pub(super) fn operand(self, node: &ast::Expr, source: RegisteredSource<'_>) -> Option<Expr> {
        if node.is_slice_expr() {
            return index_key(node, &mut |child| self.operand(child, source).ok_or(())).ok();
        }
        let value = match node {
            ast::Expr::Lambda(lambda) if !matches!(self, Self::Residual) => {
                checked_binding(lambda, source)?
            }
            node if matches!(self, Self::Lambda)
                && let Some((kind, value)) = ir::SuspendValue::operand(node) =>
            {
                let value = match value {
                    Some(value) => self.operand(value, source)?,
                    None => Expr::none(),
                };
                super::operation(
                    Builtin::RegisteredSuspend(kind, node.range()),
                    Type::PythonObject,
                    vec![value],
                )
            }
            _ => operation(
                node,
                source,
                None,
                |_| true,
                |child| self.operand(child, source).ok_or(()),
            )
            .ok()??,
        };
        Some(source.located(node, value))
    }
}

/// An output statement encountered in a source suite. Only a residual operand needs syntax.
pub(crate) enum ValueStatement<'a> {
    Checked(Statement),
    ReturnOperand(&'a ast::Expr),
    DiscardOperand(&'a ast::Expr),
    Ignore,
}

pub(crate) fn value_statement<'a>(
    stmt: &'a ast::Stmt,
    source: RegisteredSource<'_>,
) -> Option<ValueStatement<'a>> {
    Some(match stmt {
        ast::Stmt::Return(ret) => match ret.value.as_deref() {
            None => ValueStatement::Checked(Statement::Return(Expr::none())),
            Some(value) => registered_value(value, source)
                .map_or(ValueStatement::ReturnOperand(value), |value| {
                    ValueStatement::Checked(Statement::Return(value))
                }),
        },
        ast::Stmt::Expr(expr) if expr.value.is_string_literal_expr() => ValueStatement::Ignore,
        ast::Stmt::Expr(expr) => registered_value(&expr.value, source)
            .map_or(ValueStatement::DiscardOperand(&expr.value), |value| {
                ValueStatement::Checked(Statement::Expr(value))
            }),
        _ => return None,
    })
}

/// Build a complete expression region; an unsupported child keeps its original lowering.
pub(crate) fn registered_value(node: &ast::Expr, source: RegisteredSource<'_>) -> Option<Expr> {
    Region::Residual.operand(node, source)
}

impl Checker<'_> {
    /// A Python handle does not erase an inferred container contract. Retain a fixed shape
    /// only when the checked operands establish its elements; otherwise keep object storage.
    fn registered_display_type(&self, kind: &ExprKind) -> Option<Type> {
        let items = match kind {
            ExprKind::List(items)
            | ExprKind::Tuple(items)
            | ExprKind::Set { values: items, .. } => items,
            _ => return None,
        };
        let tuple = matches!(kind, ExprKind::Tuple(_));
        let set = matches!(kind, ExprKind::Set { .. });
        let mut fields = Vec::with_capacity(items.len());
        for item in items {
            if let ExprKind::Builtin(Builtin::RegisteredSpread, values) = &item.kind {
                let ty = values[0].ty;
                if let Some(expanded) = self.types.fields(ty) {
                    fields.extend(expanded);
                } else if !tuple && let Some(element) = self.types.element(ty) {
                    fields.push(element);
                } else if set && let Some(element) = self.types.readable_set_element(ty) {
                    fields.push(element);
                } else {
                    return None;
                }
            } else {
                fields.push(item.ty);
            }
        }
        if fields.contains(&Type::PythonObject) {
            return None;
        }
        if tuple {
            return Some(self.types.tuple(fields));
        }
        if set {
            let &element = fields.first()?;
            return (self.types.hashable_key(element) && fields.iter().all(|&ty| ty == element))
                .then(|| self.types.set(element));
        }
        let element = fields.first().copied().unwrap_or(Type::Int);
        if fields.iter().all(|&ty| ty == element) {
            return Some(self.types.list(element));
        }
        if fields
            .iter()
            .all(|&ty| self.can_mix_list_items(element, ty))
        {
            return Some(self.types.list(self.types.string_or_int()));
        }
        None
    }

    pub(in crate::frontend) fn registered_operand(
        &mut self,
        node: &ast::Expr,
        module: &ModuleSource,
    ) -> Result<Option<Expr>, String> {
        let source = module.context(self.source);
        if let ast::Expr::Lambda(lambda) = node {
            if checked_binding(lambda, source).is_none() {
                return Ok(None);
            }
            let mut value = lambda_binding(lambda, |default| self.expr(default))?;
            if let Some(slot) = self.namespace
                && let ExprKind::Builtin(Builtin::RegisteredLambda(_, scoped), args) =
                    &mut value.kind
            {
                args.insert(0, self.local(slot));
                *scoped = true;
            }
            return Ok(Some(source.located(node, value)));
        }
        if matches!(
            node,
            ast::Expr::ListComp(_) | ast::Expr::DictComp(_) | ast::Expr::SetComp(_)
        ) && let Some(plan) = imre_ast::comprehension::Plan::from_expr(node)
            && !plan.suspends()
        {
            let value = self.lower_display(node, &plan, None)?;
            return Ok(Some(source.located(node, value)));
        }
        // A checked local with a fixed callable layout uses its original call checker.
        if let ast::Expr::Call(call) = node
            && let Some(name) = call.func.as_name_expr()
            && self
                .names
                .get(name.id.as_str())
                .is_some_and(|&slot| self.locals[slot] != Type::PythonObject)
        {
            return Ok(None);
        }
        let local = node
            .as_name_expr()
            .and_then(|n| self.names.get(n.id.as_str()))
            .map(|&slot| self.local(slot));
        let graph = self.graph;
        let owner = self.module;
        let Some(mut value) = operation(
            node,
            source,
            local,
            |receiver| graph.session.is_dynamic_receiver(owner, receiver),
            |child| self.expr(child),
        )?
        else {
            return Ok(None);
        };
        if let ExprKind::Binary(op, left, right) = value.kind {
            // Use the ordinary operation and diagnostic whenever both operand layouts
            // are known. An unresolved operand remains a live object operation.
            value = if left.ty == Type::PythonObject || right.ty == Type::PythonObject {
                Expr {
                    kind: ExprKind::Binary(op, left, right),
                    ty: Type::PythonObject,
                }
            } else {
                self.binary_operands(node.as_bin_op_expr().unwrap(), *left, *right)?
            };
            return Ok(Some(source.located(node, value)));
        }
        if let Some(slot) = self.namespace {
            match &mut value.kind {
                ExprKind::Builtin(Builtin::RegisteredCall(call), args) => {
                    args.insert(0, self.local(slot));
                    call.explicit_scope = true;
                }
                ExprKind::Builtin(Builtin::RegisteredName(_), args) => args.push(self.local(slot)),
                _ => {}
            }
        }
        // Checked locals and exact literals keep their known types in the registered frame.
        value.ty = match &value.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Str(_) | ExprKind::Builtin(Builtin::RegisteredString(_), _) => Type::Str,
            ExprKind::Bytes(_) => Type::Bytes,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::None => Type::None,
            ExprKind::Conditional(_, yes, no) if yes.ty == no.ty => yes.ty,
            _ => self
                .registered_display_type(&value.kind)
                .unwrap_or(value.ty),
        };
        if let ExprKind::Set { element, .. } = &mut value.kind
            && let Some(known) = self.types.set_element(value.ty)
        {
            *element = known;
        }
        Ok(Some(source.located(node, value)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn operation_and_position(value: &Expr) -> (&Builtin, &[Expr], TextSize) {
        let ExprKind::Builtin(Builtin::RegisteredLocation(_, position), values) = &value.kind
        else {
            panic!("expected a located expression");
        };
        let ExprKind::Builtin(op, operands) = &values[0].kind else {
            panic!("expected a registered operation");
        };
        (op, operands, *position)
    }

    #[test]
    fn attribute_and_direct_method_use_the_physical_token_and_resolve_imported_receivers_normally()
    {
        let text = "import sys as library\n(holder\n .method)(arg)\n(library\n .method)(arg)\n(holder\n .method)\n";
        let parsed = imre_ast::indentation::prepare(text).parse_module().unwrap();
        let imports = imre_ast::module_imports(&parsed.syntax().body);
        let source = RegisteredSource {
            templates: None,
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &imports,
            private_names: None,
        };
        let values: Vec<_> = parsed
            .syntax()
            .body
            .iter()
            .skip(1)
            .map(|stmt| {
                let ast::Stmt::Expr(stmt) = stmt else {
                    panic!("expected an expression")
                };
                registered_value(&stmt.value, source).unwrap()
            })
            .collect();
        let (
            Builtin::RegisteredCall(ir::RegisteredCall {
                target: ir::RegisteredCallTarget::Method { name, attribute },
                ..
            }),
            args,
            pos,
        ) = operation_and_position(&values[0])
        else {
            panic!("expected a direct method");
        };
        assert_eq!(name, "method");
        assert_eq!(args.len(), 2);
        assert_eq!(
            &text[attribute.start().to_usize()..pos.to_usize()],
            "holder\n ."
        );
        let (Builtin::RegisteredCall(..), args, call_position) = operation_and_position(&values[1])
        else {
            panic!("expected normal invocation of the imported module attribute");
        };
        let (Builtin::RegisteredAttribute { name, .. }, _, attr_position) =
            operation_and_position(&args[0])
        else {
            panic!("expected a separately evaluated attribute");
        };
        assert_eq!(name, "method");
        assert!(call_position < attr_position);
        let (Builtin::RegisteredAttribute { name, .. }, _, pos) =
            operation_and_position(&values[2])
        else {
            panic!("expected an ordinary attribute read");
        };
        assert_eq!(name, "method");
        assert_eq!(&text[pos.to_usize()..][..6], "method");
    }

    #[test]
    fn a_call_keeps_its_keyword_names_and_its_positional_before_keyword_order() {
        let text = "obj.emit(value, named=arg)\nf(first=a, *items, **mapping, last=b)\n";
        let parsed = imre_ast::indentation::prepare(text).parse_module().unwrap();
        let source = RegisteredSource {
            templates: None,
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &BTreeSet::new(),
            private_names: None,
        };
        let mut calls = parsed.syntax().body.iter().map(|stmt| {
            let expr = &stmt.as_expr_stmt().unwrap().value;
            registered_value(expr, source).unwrap()
        });
        let method = calls.next().unwrap();
        let (Builtin::RegisteredCall(method), operands, _) = operation_and_position(&method) else {
            panic!("expected a complete method call");
        };
        assert!(
            matches!(&method.target, ir::RegisteredCallTarget::Method { name, .. } if name == "emit")
        );
        assert_eq!(
            method.arguments,
            ir::RegisteredArguments::Keywords {
                positional: 1,
                names: vec!["named".into()],
            }
        );
        assert_eq!(operands.len(), 3);
        let call = calls.next().unwrap();
        let (Builtin::RegisteredCall(call), operands, _) = operation_and_position(&call) else {
            panic!("expected a complete packed call");
        };
        assert_eq!(
            call.arguments,
            ir::RegisteredArguments::Packed(ir::PackedArguments {
                starred: vec![true],
                keywords: vec![Some("first".into()), None, Some("last".into())],
            })
        );
        let names: Vec<_> = operands
            .iter()
            .map(|operand| match operation_and_position(operand).0 {
                Builtin::RegisteredName(name) => name.as_str(),
                _ => panic!("expected named operand"),
            })
            .collect();
        assert_eq!(names, ["f", "items", "a", "mapping", "b"]);
    }

    #[test]
    fn lowered_private_import_keeps_its_physical_module_spelling() {
        let text = "import sys as __library\n_C__library = __library\nclass C:\n    value = __library.getrecursionlimit()\n";
        let mut body = imre_ast::indentation::prepare(text)
            .parse_module()
            .unwrap()
            .into_syntax()
            .body;
        let imports = imre_ast::module_imports(&body);
        let names = imre_ast::lower(&mut body);
        let ast::Stmt::ClassDef(class) = &body[2] else {
            panic!("expected class")
        };
        let ast::Stmt::Assign(assign) = &class.body[0] else {
            panic!("expected assignment")
        };
        let call = assign.value.as_call_expr().unwrap();
        let source = RegisteredSource {
            templates: None,
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &imports,
            private_names: Some(&names),
        };
        let field = call.func.as_attribute_expr().unwrap();
        assert_eq!(
            field.value.as_name_expr().unwrap().id.as_str(),
            "_C__library"
        );
        assert!(source.direct_method(call).is_none());
        assert!(matches!(
            operation_and_position(&registered_value(&assign.value, source).unwrap()).0,
            Builtin::RegisteredCall(..)
        ));
    }

    #[test]
    fn argument_budget_and_partition_fusion_keep_their_existing_boundaries() {
        let imports = BTreeSet::new();
        let replacements = HashMap::new();
        for count in [29, 30, 64, 65] {
            let operands = std::iter::repeat_n("x", count)
                .collect::<Vec<_>>()
                .join(",");
            let text = format!("obj.send({operands})");
            let parsed = imre_ast::indentation::prepare(&text)
                .parse_module()
                .unwrap();
            let ast::Stmt::Expr(stmt) = &parsed.syntax().body[0] else {
                panic!("expected expression")
            };
            let source = RegisteredSource {
                templates: None,
                text: &text,
                replacements: &replacements,
                has_templates: false,
                method_imports: &imports,
                private_names: None,
            };
            let value = registered_value(&stmt.value, source).unwrap();
            let (Builtin::RegisteredCall(call), operands, _) = operation_and_position(&value)
            else {
                panic!("expected a Python call");
            };
            assert_eq!(operands.len(), count + 1);
            assert_eq!(
                matches!(call.target, ir::RegisteredCallTarget::Method { .. }),
                count < 30
            );
            assert_eq!(
                matches!(call.arguments, ir::RegisteredArguments::Packed(_)),
                count > 64
            );
        }
        for (text, fused) in [
            ("s.partition('-')[0]", Some(0)),
            ("s.rpartition('-')[2]", Some(2)),
            ("s.partition('-')[3]", None),
            ("s.partition(\n'-')[1]", None),
        ] {
            let parsed = imre_ast::indentation::prepare(text).parse_module().unwrap();
            let ast::Stmt::Expr(stmt) = &parsed.syntax().body[0] else {
                panic!("expected expression")
            };
            let source = RegisteredSource {
                templates: None,
                text,
                replacements: &replacements,
                has_templates: false,
                method_imports: &imports,
                private_names: None,
            };
            assert_eq!(
                source.partition_index(stmt.value.as_subscript_expr().unwrap()),
                fused
            );
            assert_eq!(
                registered_value(&stmt.value, source).is_none(),
                fused.is_some()
            );
        }
    }
}

#[cfg(test)]
mod index_constant_tests {
    use super::*;

    fn identity(source: &str) -> Option<ir::Constant> {
        let graph = SourceGraph::single(source).unwrap();
        let entry = &graph.modules[graph.entry];
        let module = ModuleSource::new(entry);
        let expr = &entry.parsed.syntax().body[0].as_expr_stmt().unwrap().value;
        let key = &expr.as_subscript_expr().unwrap().slice;
        registered_constant_key(key, module.context(source)).map(|(key, _)| key)
    }

    #[test]
    fn code_constants_distinguish_types_and_fold_only_literal_slices() {
        assert_eq!(identity("x[1, 2]"), identity("x[(+1, 2)]"));
        assert_ne!(identity("x[1,]"), identity("x[True,]"));
        assert_ne!(identity("x[0.0,]"), identity("x[-0.0,]"));
        assert_eq!(identity("x[:]"), identity("x[None:None:None]"));
        assert!(identity("x[1, 2:3, (False, 4)]").is_some());
        assert!(identity("x[1:-3:-1]").is_none());
        assert!(identity("x[a, :3]").is_none());
        assert!(identity("x[1, *values]").is_none());
    }
}
