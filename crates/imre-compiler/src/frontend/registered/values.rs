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
            template_fields: None,
            generators: None,
            lambdas: None,
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
    pub(crate) template_fields: Option<&'a HashMap<TextSize, imre_ast::strings::TemplateFieldText>>,
    pub(crate) generators: Option<&'a HashMap<TextRange, RegisteredGenerator>>,
    pub(crate) lambdas: Option<&'a HashMap<TextSize, RegisteredLambda>>,
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

/// Recover the source code points and reject a retained docstring that Python cannot encode.
pub(crate) fn module_literal_replacements(
    source: &str,
    parsed: &ruff_python_parser::Parsed<ast::ModModule>,
    keep_docstrings: bool,
) -> StdResult<HashMap<TextRange, Vec<u32>>, String> {
    let replacements = literal_replacements(source, parsed.tokens());
    if keep_docstrings
        && !replacements.is_empty()
        && let Some(range) = imre_ast::strings::find_docstring(&parsed.syntax().body, true, |doc| {
            doc.value
                .iter()
                .map(Ranged::range)
                .find(|range| replacements.contains_key(range))
        })
    {
        return Err(diagnostic_message(
            source,
            &range,
            "surrogate string escapes are unsupported in docstrings",
        ));
    }
    Ok(replacements)
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

impl RegisteredSource<'_> {
    /// Decode adjacent source parts together so ordinary and formatted literals share
    /// their constant identity, including a surrogate after an ordinary UTF-8 prefix.
    pub(crate) fn string<'a>(self, pieces: impl Iterator<Item = FStringLiteral<'a>>) -> Expr {
        let mut text = String::new();
        let mut encoded = self.has_templates.then(Vec::new);
        for piece in pieces {
            let middle;
            let replacement = match piece {
                FStringLiteral::Adjacent(lit) => self.replacements.get(&lit.range()),
                FStringLiteral::Middle(lit, _) => {
                    middle = self.format_replacements(lit);
                    middle.as_ref()
                }
                FStringLiteral::Text(_) => None,
            };
            if replacement.is_some() || encoded.is_some() {
                let bytes = encoded.get_or_insert_with(|| {
                    let mut bytes = Vec::new();
                    push_codepoints(&mut bytes, &text, None);
                    bytes
                });
                push_codepoints(bytes, piece.text(), replacement.map(Vec::as_slice));
            } else {
                text.push_str(piece.text());
            }
        }
        Expr {
            ty: Type::PythonObject,
            kind: match encoded {
                Some(bytes) => ExprKind::Builtin(Builtin::RegisteredString(bytes), vec![]),
                None => ExprKind::Str(text),
            },
        }
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

pub(crate) fn registered_arguments(args: &ast::Arguments) -> impl Iterator<Item = &ast::Expr> {
    args.args
        .iter()
        .map(|arg| match arg {
            ast::Expr::Starred(star) => star.value.as_ref(),
            value => value,
        })
        .chain(args.keywords.iter().map(|keyword| &keyword.value))
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

pub(in crate::frontend) fn located(
    range: TextRange,
    position: ruff_text_size::TextSize,
    value: Expr,
) -> Expr {
    if matches!(&value.kind,
        ExprKind::Builtin(Builtin::RegisteredLocation(existing, start), _) if (*existing, *start) == (range, position)
    ) {
        return value;
    }
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

pub(in crate::frontend) fn subscript(
    index: &ast::ExprSubscript,
    mut value: Expr,
    key: Expr,
    source: &str,
) -> Expr {
    // Attach the projection to the call the normal operand checker already bound.
    // Keep its location and any explicit comprehension namespace.
    if index.value.is_call_expr()
        && let Some(number) = index.slice.as_number_literal_expr()
        && let ast::Number::Int(number) = &number.value
        && let Some(field @ 0..=2) = number.as_i64()
        && !source[index.range()].contains('\n')
    {
        let call = match &mut value.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) => &mut operands[0].kind,
            kind => kind,
        };
        if let ExprKind::Builtin(Builtin::RegisteredCall(call), operands) = call
            && call.range == index.value.range()
            && let ir::RegisteredCallTarget::Method { name, .. } = &call.target
            && matches!(name.as_str(), "partition" | "rpartition")
            && matches!(call.arguments, ir::RegisteredArguments::Positional)
            && operands.len() == 2 + usize::from(call.explicit_scope)
        {
            call.partition_field = Some((field as u32, index.range()));
            return value;
        }
    }
    super::operation(
        Builtin::RegisteredSubscript(index.range(), index.value.end()),
        Type::PythonObject,
        vec![value, key],
    )
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
        partition_field: None,
        explicit_scope: false,
    }
}

pub(in crate::frontend) fn fixed_positional_arguments(call: &ast::ExprCall) -> bool {
    matches!(
        registered_argument_layout(&call.arguments),
        ir::RegisteredArguments::Positional
    )
}

/// Invoke a checked callable value with its positional operands. Method resolution
/// and argument packing carry additional binding data in the registered call.
fn bound_call(call: ir::RegisteredCall, target: Expr, mut arguments: Vec<Expr>) -> Expr {
    if target.ty == Type::PythonObject
        && matches!(call.arguments, ir::RegisteredArguments::Positional)
        && let ir::RegisteredCallTarget::Callable {
            end,
            suspended_name,
        } = call.target
    {
        return positional_invoke(call.range, end, suspended_name, target, arguments);
    }
    arguments.insert(0, target);
    super::operation(Builtin::RegisteredCall(call), Type::PythonObject, arguments)
}

fn positional_invoke(
    range: TextRange,
    callee_end: TextSize,
    suspended_name: Option<TextRange>,
    target: Expr,
    arguments: Vec<Expr>,
) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Invoke {
            callable: Box::new(target),
            args: ir::CallArgs {
                python_site: Some(ir::InvokeSite {
                    range,
                    callee_end,
                    suspended_name,
                    scope: None,
                }),
                ..ir::CallArgs::positional(arguments)
            },
        },
    }
}

impl Checker<'_> {
    pub(in crate::frontend) fn located_callback_effect(
        &self,
        expr: &ast::Expr,
        call: &ast::ExprCall,
        callable: Expr,
        arguments: Vec<Expr>,
    ) -> Expr {
        let source = self
            .registered
            .expect("registered callback frame")
            .context(self.source);
        let target = source.located(&call.func, callable);
        let mut value = positional_invoke(
            call.range(),
            call.func.end(),
            Some(call.func.range()),
            target,
            arguments,
        );
        if let Some(slot) = self.namespace {
            in_namespace(&mut value, self.local(slot));
        }
        source.located(expr, value)
    }

    pub(in crate::frontend) fn call_with_target(
        &mut self,
        call: &ast::ExprCall,
        target: Expr,
    ) -> Result<Expr, String> {
        let arguments = registered_arguments(&call.arguments)
            .map(|arg| self.expr(arg))
            .collect::<Result<_, _>>()?;
        let mut value = bound_call(call_site(call, callable_target(call)), target, arguments);
        if let Some(slot) = self.namespace {
            in_namespace(&mut value, self.local(slot));
        }
        Ok(value)
    }
}

/// The fixed checker selected this builtin and recorded its registered frame requirement.
pub(in crate::frontend) fn name_call(
    call: &ast::ExprCall,
    name: &str,
    mut arguments: Vec<Expr>,
) -> Expr {
    arguments.insert(0, name_binding(name));
    super::operation(
        Builtin::RegisteredCall(call_site(call, callable_target(call))),
        Type::PythonObject,
        arguments,
    )
}

pub(super) fn number_literal(number: &ast::Number, minus: bool) -> ExprKind {
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

fn display_elements(node: &ast::Expr) -> Option<(&[ast::Expr], DisplayKind)> {
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
        ast::Expr::Slice(slice) => slice_key(slice, |bound| {
            let value = index_key(bound, operand)?;
            let constant = literal_bound(bound, Some(&value));
            Ok((value, constant))
        })?,
        ast::Expr::Tuple(tuple) => {
            let values = tuple
                .elts
                .iter()
                .map(|item| collection_item(item, |value| index_key(value, operand)))
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
    Ok(located_key(node, value, constant))
}

fn located_key(node: &ast::Expr, value: Expr, constant: Option<ir::Constant>) -> Expr {
    use ir::Constant as C;
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
    located(node.range(), node.start(), value)
}

fn literal_bound(bound: &ast::Expr, value: Option<&Expr>) -> Option<ir::Constant> {
    if bound.is_literal_expr() {
        ir::Constant::from_expr(value?)
    } else {
        None
    }
}

fn slice_bounds(slice: &ast::ExprSlice) -> [Option<&ast::Expr>; 3] {
    [
        slice.lower.as_deref(),
        slice.upper.as_deref(),
        slice.step.as_deref(),
    ]
}

/// A bare annotated target evaluates its receiver and key expressions without accessing
/// the final attribute or item. Each slice bound remains an ordinary expression.
pub(super) fn bare_target_operands(target: &ast::Expr) -> Option<Vec<&ast::Expr>> {
    fn key_operands<'a>(key: &'a ast::Expr, inputs: &mut Vec<&'a ast::Expr>) {
        match key {
            ast::Expr::Tuple(tuple) => {
                for item in &tuple.elts {
                    key_operands(item, inputs);
                }
            }
            ast::Expr::Slice(slice) => inputs.extend(slice_bounds(slice).into_iter().flatten()),
            value => inputs.push(value),
        }
    }
    match target {
        ast::Expr::Attribute(field) => Some(vec![&field.value]),
        ast::Expr::Subscript(item) => {
            let mut inputs = vec![item.value.as_ref()];
            key_operands(&item.slice, &mut inputs);
            Some(inputs)
        }
        _ => None,
    }
}

fn slice_key<'a, E>(
    slice: &'a ast::ExprSlice,
    mut operand: impl FnMut(&'a ast::Expr) -> StdResult<(Expr, Option<ir::Constant>), E>,
) -> StdResult<(Expr, Option<ir::Constant>), E> {
    use ir::Constant as C;
    let mut bounds = Vec::with_capacity(3);
    let mut constants = Some([C::None, C::None, C::None]);
    for (i, bound) in slice_bounds(slice).into_iter().enumerate() {
        if let Some(bound) = bound {
            let (value, constant) = operand(bound)?;
            bounds.push(value);
            constants = constants.and_then(|mut keys| {
                keys[i] = constant?;
                Some(keys)
            });
        } else if i < 2 {
            bounds.push(Expr::none());
        }
    }
    Ok((
        super::operation(Builtin::RegisteredSlice, Type::PythonObject, bounds),
        constants.map(|keys| C::Slice(Box::new(keys))),
    ))
}

/// Only the input producers may need the enclosing source adapter. The operation
/// itself uses ordinary checked IR with positional inputs, independent of Python locals.
pub(super) struct IndexOperands<'a> {
    pub(super) value: Expr,
    pub(super) inputs: Vec<(&'a ast::Expr, Option<Expr>)>,
}

pub(super) fn index_operands<'a>(
    node: &'a ast::Expr,
    source: RegisteredSource<'_>,
) -> Option<IndexOperands<'a>> {
    let mut inputs = Vec::new();
    let mut input = |expr: &'a ast::Expr, value: Option<Expr>| {
        let slot = inputs.len();
        inputs.push((expr, value));
        Expr::local(slot, Type::PythonObject)
    };
    let (read, key) = match node {
        ast::Expr::Subscript(index) => {
            let receiver = input(&index.value, registered_value(&index.value, source));
            (Some((index, receiver)), index.slice.as_ref())
        }
        ast::Expr::Slice(_) => (None, node),
        _ => return None,
    };
    // A complete object key keeps its ordinary cache and construction boundary.
    // A two-bound read uses the checked slice shape directly in read_item.
    let slice = key.as_slice_expr();
    let fused = read.is_some() && slice.is_some_and(|slice| slice.step.is_none());
    let checked = if fused {
        None
    } else {
        registered_index_key(key, source)
    };
    let value = if let Some(checked) = checked {
        input(key, Some(checked))
    } else if let Some(slice) = slice {
        let Ok((value, constant)) = slice_key(slice, |bound| {
            let value = registered_index_key(bound, source);
            let constant = literal_bound(bound, value.as_ref());
            Ok::<_, std::convert::Infallible>((input(bound, value), constant))
        });
        located_key(key, value, constant)
    } else {
        input(key, None)
    };
    let value = match read {
        Some((index, receiver)) => subscript(index, receiver, value, source.text),
        None => value,
    };
    Some(IndexOperands { value, inputs })
}

pub(super) fn collection_item<E>(
    item: &ast::Expr,
    child: impl FnOnce(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    match item {
        ast::Expr::Starred(star) => Ok(super::operation(
            Builtin::RegisteredSpread,
            Type::PythonObject,
            vec![child(&star.value)?],
        )),
        item => child(item),
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
            if let ast::Expr::UnaryOp(op) = item
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
            } else if item.is_slice_expr() {
                // An extended subscript key can mix a slice with other tuple elements.
                index_key(item, child)
            } else {
                collection_item(item, &mut *child)
            }
        })
        .collect()
}

/// Test membership without allocating the small constant tuple at runtime.
fn literal_membership(cmp: &ast::ExprCompare) -> Option<(&ast::Expr, &[ast::Expr], bool)> {
    let [op @ (ast::CmpOp::In | ast::CmpOp::NotIn)] = cmp.ops.as_ref() else {
        return None;
    };
    let [needle, ast::Expr::Tuple(items)] = cmp.operands.as_ref() else {
        return None;
    };
    (items.elts.len() <= 8 && items.elts.iter().all(imre_ast::literal_tuple_element)).then_some((
        needle,
        &items.elts,
        *op == ast::CmpOp::NotIn,
    ))
}

/// The Python operand order is also used when a child still needs its source emitter.
pub(super) fn binary_operands(node: &ast::Expr) -> Option<(Binary, [&ast::Expr; 2])> {
    let binary = node.as_bin_op_expr()?;
    Some((binary.op.into(), [&binary.left, &binary.right]))
}

pub(super) fn binary(op: Binary, left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
    }
}

/// Select one registered-frame operation before requesting its operands in source order.
pub(super) fn operation<E>(
    node: &ast::Expr,
    source: RegisteredSource<'_>,
    local: Option<Expr>,
    class_namespace: bool,
    dynamic_receiver: impl FnOnce(&ast::Expr) -> bool,
    mut child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Option<Expr>, E> {
    if let Some((op, [left, right])) = binary_operands(node) {
        return Ok(Some(binary(op, child(left)?, child(right)?)));
    }
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
        if target.ty != Type::PythonObject && !class_namespace {
            return Ok(None);
        }
        let arguments = selected
            .arguments()
            .map(&mut child)
            .collect::<StdResult<_, _>>()?;
        if let Builtin::RegisteredCall(call) = selected.opcode {
            return Ok(Some(bound_call(call, target, arguments)));
        }
        return Ok(Some(super::operation(
            selected.opcode,
            Type::PythonObject,
            vec![target],
        )));
    }
    let kind = match node {
        ast::Expr::Generator(generator)
            if source
                .generators
                .is_some_and(|sites| sites.contains_key(&generator.range())) =>
        {
            let outer = imre_ast::comprehension::outer_clause(&generator.generators);
            let acquire = if outer.is_async {
                ir::IteratorOp::Async(outer.iter.range())
            } else {
                ir::IteratorOp::Get(outer.iter.range())
            };
            let iterator = super::operation(
                Builtin::RegisteredIter(acquire),
                Type::PythonObject,
                vec![child(&outer.iter)?],
            );
            ExprKind::Builtin(
                Builtin::RegisteredGenerator(generator.range(), false),
                vec![iterator],
            )
        }
        ast::Expr::Compare(cmp) => {
            if let Some((needle, items, negate)) = literal_membership(cmp) {
                let values = std::iter::once(needle)
                    .chain(items)
                    .map(&mut child)
                    .collect::<StdResult<Vec<_>, _>>()?;
                ExprKind::Builtin(Builtin::RegisteredContainsLiteral(negate), values)
            } else {
                let values = cmp
                    .operands
                    .iter()
                    .map(&mut child)
                    .collect::<StdResult<Vec<_>, _>>()?;
                ExprKind::Builtin(Builtin::RegisteredCompare(cmp.ops.to_vec()), values)
            }
        }
        ast::Expr::If(branch) => {
            return Ok(Some(select(
                child(&branch.test)?,
                child(&branch.body)?,
                child(&branch.orelse)?,
            )));
        }
        ast::Expr::BoolOp(op) => {
            let values = op.values.iter().map(child).collect::<StdResult<_, _>>()?;
            return Ok(Some(selected_operands(op.op == ast::BoolOp::And, values)));
        }
        ast::Expr::Name(name) => {
            if let Some(value) = local {
                return Ok(Some(value));
            }
            return Ok(Some(name_binding(&name.id)));
        }
        ast::Expr::NumberLiteral(lit) => number_literal(&lit.value, false),
        ast::Expr::StringLiteral(lit) => {
            return Ok(Some(
                source.string(lit.value.iter().map(FStringLiteral::Adjacent)),
            ));
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
        ast::Expr::Subscript(index) => {
            return Ok(Some(subscript(
                index,
                child(&index.value)?,
                index_key(&index.slice, &mut child)?,
                source.text,
            )));
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
    lambda
        .parameters
        .as_deref()
        .into_iter()
        .flat_map(imre_ast::parameter_defaults)
}

pub(super) fn lambda_binding(lambda: &ast::ExprLambda, defaults: Vec<Expr>) -> Expr {
    super::operation(
        Builtin::RegisteredLambda(lambda.start(), false),
        Type::PythonObject,
        defaults,
    )
}

/// The caller lowers each default in its enclosing lexical scope.
pub(super) fn has_checked_lambda_body(
    lambda: &ast::ExprLambda,
    source: RegisteredSource<'_>,
) -> bool {
    match source.lambdas {
        Some(sites) => sites
            .get(&lambda.start())
            .is_some_and(|site| site.body.is_some()),
        None => RegisteredLambda::body(lambda, source).is_some(),
    }
}

pub(super) fn checked_binding(
    lambda: &ast::ExprLambda,
    source: RegisteredSource<'_>,
) -> Option<Expr> {
    if let Some(sites) = source.lambdas {
        let site = sites.get(&lambda.start())?;
        site.body.as_ref()?;
        return match &site.binding {
            ExpressionSelection::Checked(binding) => Some(binding.clone()),
            _ => None,
        };
    }
    RegisteredLambda::body(lambda, source)?;
    let defaults = lambda_defaults(lambda)
        .map(|default| Region::Lambda.operand(default, source))
        .collect::<Option<_>>()?;
    Some(lambda_binding(lambda, defaults))
}

#[derive(Clone, Copy)]
pub(in crate::frontend) enum Region {
    /// Residual expressions encounter a prepared lambda binding at the source site.
    Residual,
    SuspendedOperand,
    Suite,
    /// Suspensions within a lambda's body or defaults use the enclosing native activation.
    Lambda,
}

impl Region {
    pub(in crate::frontend) fn operand(
        self,
        node: &ast::Expr,
        source: RegisteredSource<'_>,
    ) -> Option<Expr> {
        if node.is_slice_expr() {
            return index_key(node, &mut |child| self.operand(child, source).ok_or(())).ok();
        }
        let value = match node {
            ast::Expr::TString(template) => {
                source.template(template, false, |child| self.operand(child, source))?
            }
            ast::Expr::FString(value) if matches!(self, Self::Suite | Self::Lambda) => {
                fstrings::registered_fstring(value, source, |child| {
                    self.operand(child, source).ok_or(())
                })
                .ok()?
            }
            ast::Expr::Lambda(lambda)
                if !matches!(self, Self::Residual | Self::SuspendedOperand) =>
            {
                checked_binding(lambda, source)?
            }
            node if matches!(self, Self::Lambda | Self::SuspendedOperand)
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
                false,
                |_| true,
                |child| self.operand(child, source).ok_or(()),
            )
            .ok()??,
        };
        Some(source.located(node, value))
    }
}

pub(crate) fn suspended_operand(node: &ast::Expr, source: RegisteredSource<'_>) -> Option<Expr> {
    Region::SuspendedOperand.operand(node, source)
}

/// A display's calls and name lookups observe its lexical namespace.
pub(super) fn in_namespace(value: &mut Expr, namespace: Expr) {
    match &mut value.kind {
        ExprKind::Invoke {
            args:
                ir::CallArgs {
                    python_site: Some(site),
                    ..
                },
            ..
        } => site.scope = Some(Box::new(namespace)),
        ExprKind::Builtin(Builtin::RegisteredCall(call), args) => {
            args.insert(0, namespace);
            call.explicit_scope = true;
        }
        ExprKind::Builtin(
            Builtin::RegisteredLambda(_, scoped) | Builtin::RegisteredGenerator(_, scoped),
            args,
        ) => {
            args.insert(0, namespace);
            *scoped = true;
        }
        ExprKind::Builtin(Builtin::RegisteredName(_), args) => args.push(namespace),
        _ => {}
    }
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
            let defaults = lambda_defaults(lambda)
                .map(|default| {
                    self.expr(default)
                        .map(|value| source.located(default, value))
                })
                .collect::<Result<_, _>>()?;
            let mut value = lambda_binding(lambda, defaults);
            if let Some(slot) = self.namespace {
                in_namespace(&mut value, self.local(slot));
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
        if let ast::Expr::Call(call) = node
            && let Some(name) = call.func.as_name_expr()
            && let Some(&slot) = self.names.get(name.id.as_str())
            && self.locals[slot] != Type::PythonObject
        {
            // The fixed call checker binds the positional signature and result type.
            return Ok(None);
        }
        if let ast::Expr::BinOp(binary) = node {
            let value = self.binary_operation(binary)?;
            return Ok(Some(source.located(node, value)));
        }
        let local = node
            .as_name_expr()
            .and_then(|n| self.names.get(n.id.as_str()))
            .map(|&slot| self.local(slot));
        let graph = self.graph;
        let owner = self.module;
        if let ast::Expr::Compare(cmp) = node {
            // Preserve the existing checker for every link between concrete operands.
            let opaque = cmp.operands.iter().map(|operand| {
                let receiver = match operand {
                    ast::Expr::Call(call) => source.call(call).1,
                    ast::Expr::Attribute(field) => field.value.as_ref(),
                    _ => operand,
                };
                graph.session.is_dynamic_receiver(owner, receiver)
            });
            let mut previous = None;
            for dynamic in opaque {
                if previous == Some(false) && !dynamic {
                    return Ok(None);
                }
                previous = Some(dynamic);
            }
        }
        let Some(mut value) = operation(
            node,
            source,
            local,
            false,
            |receiver| graph.session.is_dynamic_receiver(owner, receiver),
            |child| self.expr(child),
        )?
        else {
            return Ok(None);
        };
        if let Some(slot) = self.namespace {
            in_namespace(&mut value, self.local(slot));
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

    fn value_and_position(value: &Expr) -> (&ExprKind, TextSize) {
        let ExprKind::Builtin(Builtin::RegisteredLocation(_, position), values) = &value.kind
        else {
            panic!("expected a located expression");
        };
        (&values[0].kind, *position)
    }

    fn operation_and_position(value: &Expr) -> (&Builtin, &[Expr], TextSize) {
        let (ExprKind::Builtin(op, operands), position) = value_and_position(value) else {
            panic!("expected a registered operation");
        };
        (op, operands, position)
    }

    #[test]
    fn attribute_and_direct_method_use_the_physical_token_and_resolve_imported_receivers_normally()
    {
        let text = "import sys as library\n(holder\n .method)(arg)\n(library\n .method)(arg)\n(holder\n .method)\n";
        let parsed = imre_ast::indentation::prepare(text).parse_module().unwrap();
        let imports = imre_ast::module_imports(&parsed.syntax().body);
        let source = RegisteredSource {
            template_fields: None,
            generators: None,
            lambdas: None,
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
        let (ExprKind::Invoke { callable, .. }, call_position) = value_and_position(&values[1])
        else {
            panic!("expected normal invocation of the imported module attribute");
        };
        let (Builtin::RegisteredAttribute { name, .. }, _, attr_position) =
            operation_and_position(callable)
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
            template_fields: None,
            generators: None,
            lambdas: None,
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
    fn live_name_call_invokes_the_evaluated_lookup() {
        let text = "callback(argument)";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = ModuleSource::new(module);
        let call = &module.parsed.syntax().body[0].as_expr_stmt().unwrap().value;
        let value = registered_value(call, source.context(text)).unwrap();
        let (ExprKind::Invoke { callable, args }, _) = value_and_position(&value) else {
            panic!("expected a positional invocation of the looked-up value");
        };
        assert!(
            matches!(operation_and_position(callable).0, Builtin::RegisteredName(name) if name == "callback")
        );
        let [argument] = args.values.as_slice() else {
            panic!("expected the checked positional argument");
        };
        assert!(
            matches!(operation_and_position(argument).0, Builtin::RegisteredName(name) if name == "argument")
        );
        assert_eq!(args.order, [0]);
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
            template_fields: None,
            generators: None,
            lambdas: None,
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
            value_and_position(&registered_value(&assign.value, source).unwrap()).0,
            ExprKind::Invoke { .. }
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
                template_fields: None,
                generators: None,
                lambdas: None,
                text: &text,
                replacements: &replacements,
                has_templates: false,
                method_imports: &imports,
                private_names: None,
            };
            let value = registered_value(&stmt.value, source).unwrap();
            match value_and_position(&value).0 {
                ExprKind::Invoke { args, .. } => {
                    assert!((30..=64).contains(&count));
                    assert_eq!(args.values.len(), count);
                }
                ExprKind::Builtin(Builtin::RegisteredCall(call), operands) => {
                    assert!(!(30..=64).contains(&count));
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
                _ => panic!("expected a Python call"),
            }
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
                template_fields: None,
                generators: None,
                lambdas: None,
                text,
                replacements: &replacements,
                has_templates: false,
                method_imports: &imports,
                private_names: None,
            };
            let value = registered_value(&stmt.value, source).unwrap();
            let mut calls = Vec::new();
            ir::walk_operand_expressions(&value, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredCall(call), operands) = &expr.kind {
                    calls.push((call.range, call.partition_field, operands.len()));
                }
            });
            let subscript = stmt.value.as_subscript_expr().unwrap();
            assert_eq!(
                calls,
                [(
                    subscript.value.range(),
                    fused.map(|i| (i, subscript.range())),
                    2
                )]
            );
            let (operation, _, _) = operation_and_position(&value);
            assert_eq!(
                matches!(operation, Builtin::RegisteredSubscript(..)),
                fused.is_none()
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
        ir::Constant::from_expr(&registered_index_key(key, module.context(source))?)
    }

    #[test]
    fn residual_bound_is_an_input_to_a_checked_slice_read() {
        let source = "owner()[1000:next(i for i in items):1001]";
        let graph = SourceGraph::single(source).unwrap();
        let entry = &graph.modules[graph.entry];
        let module = ModuleSource::new(entry);
        let expr = &entry.parsed.syntax().body[0].as_expr_stmt().unwrap().value;
        let plan = index_operands(expr, module.context(source)).unwrap();
        let mut residual = Vec::new();
        for (child, value) in &plan.inputs {
            if value.is_none() {
                residual.push(&source[child.range()]);
            }
        }
        assert_eq!(residual, ["next(i for i in items)"]);
        assert_eq!(plan.inputs.len(), 4);
        let mut slots = Vec::new();
        ir::walk_operand_expressions(&plan.value, &mut |operand| {
            if let ExprKind::Local(slot) = operand.kind {
                slots.push(slot);
            }
        });
        assert_eq!(slots, [0, 1, 2, 3]);
        assert!(matches!(
            plan.value.kind,
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), _)
        ));
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
