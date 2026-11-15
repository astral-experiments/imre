//! Lower values evaluated against a registered Python scope into the expression IR.
use super::*;
use crate::ir::DisplayKind;
use std::collections::BTreeSet;

pub(crate) struct ModuleSource {
    pub(crate) method_imports: BTreeSet<String>,
    pub(crate) replacements: HashMap<TextRange, Vec<u32>>,
    pub(crate) has_templates: bool,
}

impl ModuleSource {
    pub(crate) fn new(module: &crate::modules::SourceModule) -> Self {
        Self::from_tokens(
            &module.source,
            module.parsed.tokens(),
            imre_ast::module_imports(&module.parsed.syntax().body),
        )
    }

    pub(crate) fn from_tokens(
        text: &str,
        tokens: &ast::token::Tokens,
        method_imports: BTreeSet<String>,
    ) -> Self {
        let (replacements, has_templates) = scan_literals(text, tokens);
        Self {
            method_imports,
            replacements,
            has_templates,
        }
    }
    #[cfg(test)]
    pub(in crate::frontend) fn context<'a>(&'a self, text: &'a str) -> RegisteredSource<'a> {
        self.operands(text, 0)
    }

    pub(crate) fn operands<'a>(
        &'a self,
        text: &'a str,
        python_optimization: i32,
    ) -> RegisteredSource<'a> {
        RegisteredSource {
            python_optimization,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            replacements: &self.replacements,
            method_imports: &self.method_imports,
            private_names: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RegisteredSource<'a> {
    pub(crate) python_optimization: i32,
    pub(crate) operand_types: Option<crate::analysis::OperandTypes<'a>>,
    pub(crate) text: &'a str,
    pub(crate) replacements: &'a HashMap<TextRange, Vec<u32>>,
    pub(crate) template_fields: Option<&'a HashMap<TextSize, imre_ast::strings::TemplateFieldText>>,
    pub(crate) generators: Option<&'a HashMap<TextRange, RegisteredGenerator>>,
    pub(crate) lambdas: Option<&'a HashMap<TextSize, RegisteredLambda>>,
    pub(crate) method_imports: &'a BTreeSet<String>,
    pub(crate) private_names: Option<&'a imre_ast::Names>,
}

impl<'a> RegisteredSource<'a> {
    pub(crate) fn with_children(
        self,
        lambdas: &'a HashMap<TextSize, RegisteredLambda>,
        generators: &'a HashMap<TextRange, RegisteredGenerator>,
    ) -> Self {
        Self {
            lambdas: Some(lambdas),
            generators: Some(generators),
            ..self
        }
    }

    pub(crate) fn direct_method(self, call: &ast::ExprCall) -> Option<&ast::ExprAttribute> {
        imre_ast::direct_method(call, self.method_imports, self.private_names)
    }

    /// Load the original imported module member before evaluating the written arguments.
    pub(in crate::frontend) fn imported_callable(
        self,
        module: &ast::ExprName,
        call: &ast::ExprCall,
        types: crate::analysis::OperandTypes<'_>,
    ) -> Option<Expr> {
        if !self.method_imports.contains(module.id.as_str())
            || !types.is_original_operand(&call.func)
        {
            return None;
        }
        registered_value(&call.func, self)
    }

    fn call_target(self, call: &ast::ExprCall) -> (ir::RegisteredCallTarget, &ast::Expr) {
        if let Some(field) = self.direct_method(call) {
            (method_target(field), field.value.as_ref())
        } else {
            (callable_target(call), call.func.as_ref())
        }
    }

    /// Bind the receiver before the arguments, including physical inputs in a mixed expression.
    fn object_operation<E>(
        self,
        node: &ast::Expr,
        placed: Option<FixedAttribute>,
        allow_typed_receiver: impl FnOnce(Option<&ast::Expr>, &ast::Expr, &Expr) -> bool,
        dynamic_receiver: impl FnOnce(&ast::Expr) -> bool,
        mut child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
    ) -> StdResult<Option<Expr>, E> {
        let receiver = |node: &ast::Expr, needs_protocol: bool, method| {
            if needs_protocol && !dynamic_receiver(node) {
                return Ok(None);
            }
            let value = child(node)?;
            Ok(
                (allow_typed_receiver(method, node, &value) || value.ty == Type::PythonObject)
                    .then_some(value),
            )
        };
        match node {
            ast::Expr::Call(call) => {
                let field = self.direct_method(call);
                let input = field.map_or(call.func.as_ref(), |field| field.value.as_ref());
                let method = field.map(|_| call.func.as_ref());
                let Some(target) = receiver(input, field.is_some(), method)? else {
                    return Ok(None);
                };
                if let (Some(field), Some(FixedAttribute::Call(None, site))) = (field, placed)
                    && let ir::RegisteredCallTarget::Method {
                        name,
                        attribute,
                        name_start,
                    } = &site.target
                    && *name == field.attr.id
                    && *attribute == field.range()
                    && *name_start == field.attr.start()
                {
                    let arguments = call_arguments(call, child)?;
                    return Ok(Some(invoke_value(site, Box::new(target), arguments)));
                }
                let selection = field.map_or_else(|| callable_target(call), method_target);
                Ok(Some(selection.prepare(call, target, child)?))
            }
            ast::Expr::Attribute(field) => {
                Ok(receiver(&field.value, true, None)?.map(|value| loaded_attribute(field, value)))
            }
            _ => Ok(None),
        }
    }

    /// Ordinary object calls select their own source target before evaluating the operands.
    pub(super) fn located_operation(self, node: &ast::Expr, value: Expr) -> Expr {
        let range = node.range();
        let position = self.position(node, range, &value, Some(range.start()));
        located(range, position, value)
    }

    pub(in crate::frontend) fn located(self, node: &ast::Expr, value: Expr) -> Expr {
        let range = node.range();
        let position = self.position(node, range, &value, None);
        located(range, position, value)
    }

    fn position(
        self,
        node: &ast::Expr,
        range: TextRange,
        value: &Expr,
        callable_start: Option<TextSize>,
    ) -> TextSize {
        value
            .python_call()
            .filter(|call| call.position.is_none() && call.range == range)
            .and_then(|call| call.method_name_start().or(callable_start))
            .unwrap_or_else(|| {
                imre_ast::expression_position(node, self.method_imports, self.private_names)
            })
    }
}

/// Reject a retained docstring that Python cannot encode.
pub(crate) fn validate_docstring_literals(
    source: &str,
    parsed: &ruff_python_parser::Parsed<ast::ModModule>,
    keep_docstrings: bool,
    replacements: Option<&HashMap<TextRange, Vec<u32>>>,
) -> StdResult<(), String> {
    if keep_docstrings
        && replacements.is_none_or(|map| !map.is_empty())
        && let Some(range) = imre_ast::strings::find_docstring(&parsed.syntax().body, true, |doc| {
            doc.value
                .iter()
                .find(|part| match replacements {
                    Some(map) => map.contains_key(&part.range()),
                    None => has_surrogate(part, source),
                })
                .map(Ranged::range)
        })
    {
        return Err(diagnostic_message(
            source,
            &range,
            "surrogate string escapes are unsupported in docstrings",
        ));
    }
    Ok(())
}

/// Find cooked Unicode replacements and whether to collect template fields in the same token pass.
pub(crate) fn scan_literals(
    source: &str,
    tokens: &ast::token::Tokens,
) -> (HashMap<TextRange, Vec<u32>>, bool) {
    let mut has_templates = false;
    let mut replacements = HashMap::new();
    for token in tokens {
        if token.kind() == ast::token::TokenKind::TStringStart {
            has_templates = true;
            continue;
        }
        let Some(range) = imre_ast::strings::cooked_content(token) else {
            continue;
        };
        let points: Vec<_> = imre_ast::strings::replacements(&source[range]).collect();
        if points.iter().any(|&point| point != 0xfffd) {
            replacements.insert(token.range(), points);
        }
    }
    (replacements, has_templates)
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
        let mut encoded = None;
        for piece in pieces {
            let replacement = match piece {
                FStringLiteral::Adjacent(lit) => self.replacements.get(&lit.range()),
                FStringLiteral::Middle(lit, _) => self.replacements.get(&lit.range()),
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

pub(in crate::frontend) fn located_name(name: &ast::ExprName) -> Expr {
    located(name.range(), name.start(), name_binding(name.id.clone()))
}

pub(in crate::frontend) fn name_binding(id: impl Into<ast::name::Name>) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(Builtin::RegisteredName(id.into()), vec![]),
    }
}

/// Fixed calls use bounded entry-block scratch. Expanded or larger calls retain
/// their positional and keyword layout for incremental packing.
fn fixed_arguments(args: &ast::Arguments, expanded: bool) -> bool {
    args.len() <= 64 && !expanded
}

fn argument_layout(args: &ast::Arguments, expanded: bool) -> ir::RegisteredArguments {
    if fixed_arguments(args, expanded) {
        fixed_argument_layout(args)
    } else {
        ir::RegisteredArguments::Packed(packed_arguments(args))
    }
}

pub(in crate::frontend) fn fixed_argument_layout(args: &ast::Arguments) -> ir::RegisteredArguments {
    if args.keywords.is_empty() {
        ir::RegisteredArguments::Positional
    } else {
        ir::RegisteredArguments::Keywords {
            positional: args.args.len(),
            names: args
                .keywords
                .iter()
                .map(|key| key.arg.as_ref().unwrap().id.clone())
                .collect(),
        }
    }
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

pub(in crate::frontend) fn located(
    range: TextRange,
    position: ruff_text_size::TextSize,
    mut value: Expr,
) -> Expr {
    if value
        .source_position()
        .is_some_and(|(existing, anchor, _)| (existing, anchor) == (range, position))
    {
        return value;
    }
    if let ExprKind::Invoke { args, .. } = &mut value.kind
        && let ir::CallSite::Python(site) = &mut args.site
        && site.call.range == range
        && site.call.position.is_none()
    {
        site.call.position = Some((position, None));
        return value;
    }
    let hint = (value.ty == Type::PythonObject && matches!(value.kind, ExprKind::Int(_)))
        .then_some(Type::Int);
    super::operation(
        Builtin::RegisteredLocation(range, position, hint),
        value.ty,
        vec![value],
    )
}

/// A physical bound method reads its first argument from the Python frame.
pub(in crate::frontend) fn method_receiver(
    first_local: Option<usize>,
    class: Option<usize>,
    source: &ast::Expr,
    operand: &Expr,
) -> bool {
    first_local.is_some_and(|first| first != 0)
        && source.is_name_expr()
        && matches!(operand.without_location().kind, ExprKind::Local(0))
        && class.map(Type::Class) == Some(operand.ty)
}

pub(in crate::frontend) fn loaded_attribute(field: &ast::ExprAttribute, receiver: Expr) -> Expr {
    let value = super::operation(
        super::attribute_opcode(ir::AttributeAction::Load, field),
        Type::PythonObject,
        vec![receiver],
    );
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
        if let ExprKind::Invoke { args, .. } = call
            && let ir::CallSite::Python(site) = &mut args.site
            && site.call.range == index.value.range()
            && let ir::RegisteredCallTarget::Method { name, .. } = &site.call.target
            && matches!(name.as_str(), "partition" | "rpartition")
            && matches!(site.call.arguments, ir::RegisteredArguments::Positional)
            && args.values.len() == 1
        {
            site.call.partition_field = Some((field as u32, index.range()));
            return value;
        }
    }
    super::operation(
        Builtin::RegisteredSubscript(index.range(), index.value.end()),
        Type::PythonObject,
        vec![value, key],
    )
}

pub(in crate::frontend) fn method_target(field: &ast::ExprAttribute) -> ir::RegisteredCallTarget {
    ir::RegisteredCallTarget::Method {
        name: field.attr.id.clone(),
        attribute: field.range(),
        name_start: field.attr.start(),
    }
}

pub(in crate::frontend) fn callable_target(call: &ast::ExprCall) -> ir::RegisteredCallTarget {
    ir::RegisteredCallTarget::Callable {
        end: call.func.end(),
        suspended_name: call.func.is_name_expr().then(|| call.func.range()),
    }
}

/// A source operand whose literal value can be checked at this exec call.
pub(crate) fn exec_source_operand(call: &ast::ExprCall) -> Option<&ast::Expr> {
    if call.func.as_name_expr()?.id.as_str() != "exec" {
        return None;
    }
    let args = &call.arguments.args;
    let source = args.first()?;
    if !matches!(source, ast::Expr::StringLiteral(_) | ast::Expr::Name(_)) {
        return None;
    }
    if args.len() > 3 || args.iter().any(ast::Expr::is_starred_expr) {
        return None;
    }
    let mut globals = args.len() >= 2;
    let mut locals = args.len() >= 3;
    for keyword in &call.arguments.keywords {
        let present = match keyword.arg.as_ref()?.as_str() {
            "globals" => &mut globals,
            "locals" => &mut locals,
            _ => return None,
        };
        if std::mem::replace(present, true) {
            return None;
        }
    }
    Some(source)
}

fn has_surrogate(part: &ast::StringLiteral, source: &str) -> bool {
    !part.flags.prefix().is_raw() && has_surrogate_escape(part, part.content_range(), source)
}

/// Ruff replaces lone surrogates in its UTF-8 view. The compiled source must
/// represent the actual Python string, including concatenated raw/cooked parts.
pub(crate) fn exec_literal_text<'a>(
    literal: &'a ast::ExprStringLiteral,
    outer: &str,
) -> StdResult<&'a str, &'static str> {
    let text = literal.value.to_str();
    if literal.value.iter().any(|part| has_surrogate(part, outer)) {
        return Err("exec source contains a surrogate that cannot be encoded as UTF-8");
    }
    Ok(text)
}

pub(in crate::frontend) fn fixed_positional_arguments(call: &ast::ExprCall) -> bool {
    call.arguments.keywords.is_empty()
        && fixed_arguments(&call.arguments, calls::has_unpacking(call))
}

pub(in crate::frontend) fn call_arguments<E>(
    call: &ast::ExprCall,
    check: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Vec<Expr>, E> {
    imre_ast::argument_values(&call.arguments)
        .map(check)
        .collect()
}

/// Check the written values and select their calling convention in the same visit.
fn prepared_arguments<E>(
    call: &ast::ExprCall,
    mut child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<(ir::RegisteredArguments, Vec<Expr>), E> {
    let mut expanded = false;
    let mut values = Vec::with_capacity(call.arguments.len());
    for (arg, unpack) in imre_ast::call_operands(&call.arguments) {
        expanded |= unpack;
        values.push(child(arg)?);
    }
    Ok((argument_layout(&call.arguments, expanded), values))
}

/// Keep the evaluated callable and its argument layout together on the invocation.
pub(in crate::frontend) fn bound_call(
    call: &ast::ExprCall,
    selection: ir::RegisteredCallTarget,
    target: Expr,
    arguments: Vec<Expr>,
) -> Expr {
    let (site, target) = selection.bind(call, target, None);
    invoke_value(site, target, arguments)
}

impl ir::RegisteredCallTarget {
    fn prepare<E>(
        self,
        call: &ast::ExprCall,
        target: Expr,
        child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
    ) -> StdResult<Expr, E> {
        let (layout, arguments) = prepared_arguments(call, child)?;
        let (site, target) = self.bind(call, target, Some(layout));
        Ok(invoke_value(site, target, arguments))
    }

    /// Bind the evaluated operand to its source call, reusing a layout established by the checker.
    pub(in crate::frontend) fn bind(
        self,
        call: &ast::ExprCall,
        target: Expr,
        layout: Option<ir::RegisteredArguments>,
    ) -> (Box<ir::RegisteredCall>, Box<Expr>) {
        let arguments = layout.unwrap_or_else(|| match self {
            Self::Method { .. } => fixed_argument_layout(&call.arguments),
            Self::Callable { .. } | Self::FixedExec { .. } => {
                argument_layout(&call.arguments, calls::has_unpacking(call))
            }
        });
        (
            ir::RegisteredCall::new(call.range(), self, arguments),
            Box::new(target),
        )
    }
}

fn invoke_value(site: Box<ir::RegisteredCall>, target: Box<Expr>, arguments: Vec<Expr>) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Invoke {
            callable: target,
            args: ir::CallArgs::positional_at(arguments, ir::CallSite::Python(site.invoke_site())),
        },
    }
}

/// A written Python call whose original checked parameter remains available to either layout.
#[derive(Clone)]
pub(crate) struct CheckedCall {
    pub(in crate::frontend) parameter: ast::name::Name,
    pub(in crate::frontend) slot: usize,
    pub(in crate::frontend) ty: Type,
    pub(in crate::frontend) call: Expr,
}

impl CheckedCall {
    /// Preserve the ordinary call after its supplied operands have been checked in written order.
    pub(in crate::frontend) fn method(
        call: &ast::ExprCall,
        receiver: &mut Expr,
        written: Vec<Expr>,
    ) -> Option<Self> {
        let field = call.func.as_attribute_expr()?;
        let name = field.value.as_name_expr()?;
        Self::with_parameter(name, receiver, |value| {
            bound_call(
                call,
                method_target(field),
                located(name.range(), name.start(), value),
                written,
            )
        })
    }

    /// Transfer the original operand. Its native alternative reads the same checked formal.
    pub(in crate::frontend) fn with_parameter(
        name: &ast::ExprName,
        value: &mut Expr,
        bind: impl FnOnce(Expr) -> Expr,
    ) -> Option<Self> {
        let ExprKind::Local(slot) = value.kind else {
            return None;
        };
        let ty = value.ty;
        let original = std::mem::replace(value, Expr::local(slot, ty));
        Some(Self {
            parameter: name.id.clone(),
            slot,
            ty,
            call: bind(original),
        })
    }
}

impl Checker<'_> {
    pub(in crate::frontend) fn located_callback(
        &self,
        expr: &ast::Expr,
        call: &ast::ExprCall,
        callable: Expr,
        arguments: Vec<Expr>,
    ) -> Expr {
        let source = self
            .registered
            .expect("registered callback frame")
            .operands(self.source, self.graph.python_optimization());
        let target = source.located(&call.func, callable);
        let site = ir::RegisteredCall::new(
            call.range(),
            callable_target(call),
            ir::RegisteredArguments::Positional,
        );
        let mut value = invoke_value(site, Box::new(target), arguments);
        if let Some(slot) = self.namespace {
            in_namespace(&mut value, self.local(slot));
        }
        source.located(expr, value)
    }

    /// A refused fixed region still checks the call's type and operands.
    /// Its registered frame owns the executable call.
    pub(in crate::frontend) fn discarded_call(&self, result: Type) -> Option<Expr> {
        self.unrepresented
            .borrow()
            .as_ref()
            .is_some_and(|failure| {
                if self.module_scope {
                    self.inference.is_none()
                } else {
                    failure.is_object() && self.registered.is_none() && self.fixed_sites.is_none()
                }
            })
            .then(|| super::operation(Builtin::UnrepresentedCall, result, vec![]))
    }

    pub(in crate::frontend) fn call_object_method(
        &mut self,
        call: &ast::ExprCall,
        field: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<Expr, String> {
        if self.placing_module_call(call)
            && let Some(method) = self.graph.modules[self.module]
                .registered_values()
                .operands(self.source, self.graph.python_optimization())
                .direct_method(call)
        {
            let value = method_target(method).prepare(call, receiver, |arg| self.expr(arg))?;
            return Ok(self.expression_at(call.range(), value));
        }
        if let Some(result) = self.discarded_call(Type::PythonObject) {
            self.check_discarded_arguments(call, 0)?;
            return Ok(result);
        }
        self.call_with_target(call, loaded_attribute(field, receiver))
    }

    pub(in crate::frontend) fn check_discarded_arguments(
        &mut self,
        call: &ast::ExprCall,
        checked: usize,
    ) -> Result<(), String> {
        imre_ast::argument_values(&call.arguments)
            .skip(checked)
            .try_for_each(|arg| self.check_discarded_operand(arg))
    }

    fn check_discarded_operand(&mut self, operand: &ast::Expr) -> Result<(), String> {
        match (self.registered, operand) {
            (None, ast::Expr::StringLiteral(literal)) => check_string(literal, self.source)?,
            // A plain tuple has no check of its own once its elements have been checked.
            (None, ast::Expr::Tuple(tuple)) if !tuple.iter().any(ast::Expr::is_starred_expr) => {
                for item in tuple {
                    self.check_discarded_operand(item)?;
                }
            }
            _ => {
                self.expr(operand)?;
            }
        }
        Ok(())
    }

    pub(in crate::frontend) fn call_live_target(
        &mut self,
        call: &ast::ExprCall,
        target: Expr,
    ) -> Result<Expr, String> {
        let ty = target.ty;
        let value = self.call_with_target(call, target)?;
        self.graph.checked_live_call(self.module, call, ty);
        Ok(value)
    }

    pub(in crate::frontend) fn call_with_target(
        &mut self,
        call: &ast::ExprCall,
        target: Expr,
    ) -> Result<Expr, String> {
        let value = callable_target(call).prepare(call, target, |arg| self.expr(arg))?;
        Ok(self.scoped_call(value))
    }

    pub(in crate::frontend) fn call_with_values(
        &self,
        call: &ast::ExprCall,
        target: Expr,
        arguments: Vec<Expr>,
    ) -> Expr {
        self.scoped_call(bound_call(call, callable_target(call), target, arguments))
    }

    fn scoped_call(&self, mut value: Expr) -> Expr {
        if let Some(slot) = self.namespace {
            in_namespace(&mut value, self.local(slot));
        }
        value
    }

    /// Resolve the live imported target before evaluating its already checked operands.
    pub(in crate::frontend) fn call_import_with_operands(
        &mut self,
        call: &ast::ExprCall,
        operands: Vec<Expr>,
    ) -> Result<Expr, String> {
        let source = self
            .registered
            .unwrap_or_else(|| self.graph.modules[self.module].registered_values())
            .operands(self.source, self.graph.python_optimization());
        let (selection, receiver) = source.call_target(call);
        let target = registered_value(receiver, source).map_or_else(|| self.expr(receiver), Ok)?;
        Ok(self.placed_call(call, bound_call(call, selection, target, operands)))
    }

    /// Both the linked literal and the checked str call load exec before the source operand.
    pub(in crate::frontend) fn exec_with_operand(
        &mut self,
        call: &ast::ExprCall,
        operand: Expr,
        fixed_exec: Option<TextRange>,
    ) -> Expr {
        let callee = located(call.func.range(), call.func.start(), name_binding("exec"));
        let selection = match fixed_exec {
            Some(operand) => ir::RegisteredCallTarget::FixedExec {
                name: call.func.range(),
                module: self.module,
                operand,
            },
            None => callable_target(call),
        };
        let (site, target) = selection.bind(call, callee, None);
        let mut value = invoke_value(site, target, vec![operand]);
        if fixed_exec.is_some() {
            value.ty = Type::None;
        }
        self.placed_call(call, value)
    }

    fn placed_call(&mut self, call: &ast::ExprCall, value: Expr) -> Expr {
        let value = self.scoped_call(value);
        if self.namespace.is_none()
            && let Some(sites) = &mut self.fixed_sites
            && let ExprKind::Invoke { args, .. } = &value.kind
            && sites.has_written_arguments(call, &args.values)
        {
            sites.invoked(call, &value);
        }
        value
    }

    /// Bind the Python method and check its operands in source order. A fixed
    /// caller can check the result contract before selecting its registered frame.
    pub(in crate::frontend) fn registered_method_call(
        &mut self,
        call: &ast::ExprCall,
        method: &ast::ExprAttribute,
        receiver: Expr,
        fixed_result: Type,
    ) -> Result<Expr, String> {
        self.registered_method_call_with_values(call, method, receiver, fixed_result, None)
    }

    pub(in crate::frontend) fn registered_method_call_with_values(
        &mut self,
        call: &ast::ExprCall,
        method: &ast::ExprAttribute,
        receiver: Expr,
        fixed_result: Type,
        checked: Option<Vec<Expr>>,
    ) -> Result<Expr, String> {
        let check_only = self.module_scope && self.inference.is_none() && self.registered.is_none();
        let arguments = match checked {
            Some(arguments) => arguments,
            None if check_only => {
                self.check_discarded_arguments(call, 0)?;
                Vec::new()
            }
            None => call_arguments(call, |arg| self.expr(arg))?,
        };
        let result = if self.registered.is_none() {
            let failure =
                self.scoped_builtin_error(&format!("{}.{}", receiver.ty, method.attr), call);
            if !self.missing_operation(&failure) {
                return Err(failure);
            }
            // The module's Python frame will bind and invoke the method. Only the
            // checked result type and possible call effects survive in this region.
            if check_only {
                return Ok(super::operation(
                    Builtin::UnrepresentedCall,
                    fixed_result,
                    vec![],
                ));
            }
            fixed_result
        } else {
            Type::PythonObject
        };
        let mut value = self.call_with_values(call, loaded_attribute(method, receiver), arguments);
        value.ty = result;
        Ok(value)
    }
}

/// The fixed checker selected this builtin and recorded its registered frame requirement.
pub(in crate::frontend) fn name_call(
    call: &ast::ExprCall,
    name: &str,
    arguments: Vec<Expr>,
) -> Expr {
    bound_call(call, callable_target(call), name_binding(name), arguments)
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
            let value = if let Some(literal) = literals::signed_number(node, |_| true) {
                Expr {
                    ty: Type::PythonObject,
                    ..literal
                }
            } else {
                operand(node)?
            };
            // A source string already owns its Python literal identity.
            if matches!(
                value.without_location().kind,
                ExprKind::Str(_) | ExprKind::Builtin(Builtin::RegisteredString(_), _)
            ) {
                return Ok(located(node.range(), node.start(), value));
            }
            if crate::llvm::registered::permanent_handle(&value).is_some() {
                return Ok(value);
            }
            let Some(key) = ir::Constant::from_expr(&value) else {
                return Ok(value);
            };
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
    let value = if let Some(key) = constant {
        super::operation(
            Builtin::RegisteredIndexConstant(key),
            Type::PythonObject,
            vec![value],
        )
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

pub(in crate::frontend) fn slice_bounds(slice: &ast::ExprSlice) -> [Option<&ast::Expr>; 3] {
    [
        slice.lower.as_deref(),
        slice.upper.as_deref(),
        slice.step.as_deref(),
    ]
}

/// A bare annotated target evaluates its receiver and key expressions without accessing
/// the final attribute or item. Each slice bound remains an ordinary expression.
pub(in crate::frontend) fn bare_target_operands(target: &ast::Expr) -> Option<Vec<&ast::Expr>> {
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
        constants.map(C::slice),
    ))
}

/// Only the input producers may need the enclosing source adapter. The operation
/// itself uses ordinary checked IR with positional inputs, independent of Python locals.
pub(super) fn index_operation<'a>(
    node: &'a ast::Expr,
    source: RegisteredSource<'_>,
    mut input: impl FnMut(&'a ast::Expr, Option<Expr>) -> Expr,
) -> Option<Expr> {
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
    Some(match read {
        Some((index, receiver)) => subscript(index, receiver, value, source.text),
        None => value,
    })
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
            if let Some(literal) = literals::signed_number(item, |_| true) {
                Ok(source.located(item, literal))
            } else if item.is_slice_expr() {
                // An extended subscript key can mix a slice with other tuple elements.
                index_key(item, child)
            } else {
                collection_item(item, &mut *child)
            }
        })
        .collect()
}

fn refine_operand(types: crate::analysis::OperandTypes<'_>, node: &ast::Expr, value: &mut Expr) {
    if value.ty == Type::PythonObject
        && let Some((range, hint)) = value.source_hint_mut()
        && range == node.range()
        && hint.is_none()
    {
        *hint = types.scalar_type(node);
    }
}

/// Retain the source layout on the Python operand that the arithmetic guard will inspect.
pub(in crate::frontend) fn binary(
    source: &ast::ExprBinOp,
    ty: Type,
    mut left: Expr,
    mut right: Expr,
    types: Option<crate::analysis::OperandTypes<'_>>,
) -> Expr {
    let op = source.op.into();
    if let Some(types) = types.filter(|_| {
        matches!(
            op,
            Binary::Add
                | Binary::Subtract
                | Binary::FloorDivide
                | Binary::Modulo
                | Binary::RightShift
        )
    }) {
        refine_operand(types, &source.left, &mut left);
        if left.checked_operand_type() == Type::Int {
            refine_operand(types, &source.right, &mut right);
        }
    }
    Expr {
        ty,
        kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
    }
}

/// Compare the original Python operands, keeping any scalar type checked at that use.
pub(in crate::frontend) fn comparison(
    cmp: &ast::ExprCompare,
    mut values: Vec<Expr>,
    types: Option<crate::analysis::OperandTypes<'_>>,
) -> Expr {
    if let [op] = cmp.ops.as_ref()
        && ir::Compare::from_operator(*op).is_some()
        && let Some(types) = types
        && let [left, right] = values.as_mut_slice()
    {
        if right.checked_operand_type() == Type::Int {
            refine_operand(types, &cmp.operands[0], left);
        } else if left.checked_operand_type() == Type::Int {
            refine_operand(types, &cmp.operands[1], right);
        }
    }
    // A chain can return any comparison that ends it. Identity and
    // membership always return a boolean, including on user-defined objects.
    let ty = if cmp
        .ops
        .iter()
        .all(|op| ir::Compare::from_operator(*op).is_none())
    {
        Type::Bool
    } else {
        Type::PythonObject
    };
    Expr {
        ty,
        kind: ExprKind::Compare(values, cmp.ops.to_vec()),
    }
}

/// Select one registered-frame operation before requesting its operands in source order.
pub(super) fn operation<E>(
    node: &ast::Expr,
    source: RegisteredSource<'_>,
    placed: Option<FixedAttribute>,
    allow_typed_receiver: impl FnOnce(Option<&ast::Expr>, &ast::Expr, &Expr) -> bool,
    dynamic_receiver: impl FnOnce(&ast::Expr) -> bool,
    mut child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Option<Expr>, E> {
    if let ast::Expr::BinOp(operation) = node {
        return Ok(Some(binary(
            operation,
            Type::PythonObject,
            child(&operation.left)?,
            child(&operation.right)?,
            source.operand_types,
        )));
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
    if let ast::Expr::UnaryOp(unary) = node {
        if let Some(literal) = literals::signed_number(node, |_| true) {
            return Ok(Some(literal));
        }
        if unary.op == ast::UnaryOp::Not {
            let value = child(&unary.operand)?;
            return Ok(Some(Expr::unary(unary.op, value, Type::Bool)));
        }
        if !dynamic_receiver(&unary.operand) {
            return Ok(None);
        }
        let value = child(&unary.operand)?;
        return Ok((allow_typed_receiver(None, &unary.operand, &value)
            || value.ty == Type::PythonObject)
            .then(|| Expr::unary(unary.op, value, Type::PythonObject)));
    }
    let kind = match node {
        ast::Expr::Generator(generator)
            if source
                .generators
                .is_some_and(|sites| sites.contains_key(&generator.range())) =>
        {
            return generator_binding(generator, child).map(Some);
        }
        ast::Expr::Compare(cmp) => {
            let values = cmp
                .operands
                .iter()
                .map(&mut child)
                .collect::<StdResult<Vec<_>, _>>()?;
            return Ok(Some(comparison(cmp, values, source.operand_types)));
        }
        ast::Expr::If(branch) => {
            return Ok(Some(select(
                child(&branch.test)?,
                child(&branch.body)?,
                child(&branch.orelse)?,
            )));
        }
        ast::Expr::BoolOp(op) => {
            let and = op.op == ast::BoolOp::And;
            let mut values = Vec::with_capacity(op.values.len());
            for operand in &op.values {
                let value = child(operand)?;
                let stops = ir::control::boolean_literal(&value) == Some(!and);
                values.push(value);
                if stops {
                    break;
                }
            }
            return Ok(Some(selected_operands(and, values)));
        }
        ast::Expr::Name(name)
            if let Some(value) = debug_operand(name, source.python_optimization) =>
        {
            value
        }
        ast::Expr::Name(name) => return Ok(Some(name_binding(name.id.clone()))),
        literal if let Some(literal) = literal.as_literal_expr() => {
            literals::scalar(literal, Some(source))
        }
        ast::Expr::Dict(dict) => {
            let mut entries = Vec::with_capacity(dict.items.len());
            for item in &dict.items {
                let key = item.key.as_ref().map(&mut child).transpose()?;
                let value = child(&item.value)?;
                entries.push(ir::DictEntry { key, value });
            }
            ExprKind::Dict(entries)
        }
        ast::Expr::Subscript(index) => {
            return Ok(Some(subscript(
                index,
                child(&index.value)?,
                index_key(&index.slice, &mut child)?,
                source.text,
            )));
        }
        _ => {
            return source.object_operation(
                node,
                placed,
                allow_typed_receiver,
                dynamic_receiver,
                child,
            );
        }
    };
    Ok(Some(Expr {
        ty: Type::PythonObject,
        kind,
    }))
}

/// The defining frame evaluates the outer iterable and acquires the cursor handed to the child.
fn generator_binding<E>(
    generator: &ast::ExprGenerator,
    mut operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    let outer = imre_ast::comprehension::outer_clause(&generator.generators);
    let acquire = if outer.is_async {
        ir::IteratorOp::Async(outer.iter.range())
    } else {
        ir::IteratorOp::Get(outer.iter.range())
    };
    Ok(super::operation(
        Builtin::RegisteredGenerator(generator.range(), false, acquire),
        Type::PythonObject,
        vec![operand(&outer.iter)?],
    ))
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

pub(super) fn checked_binding(
    lambda: &ast::ExprLambda,
    source: RegisteredSource<'_>,
) -> Option<Expr> {
    if let Some(sites) = source.lambdas {
        let site = sites.get(&lambda.start())?;
        site.body.as_ref()?;
        return match site.binding.as_ref() {
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
            ast::Expr::Lambda(lambda) => match self {
                Self::Residual => return None,
                // A suspended operand can reuse a binding prepared in its enclosing scope.
                Self::SuspendedOperand if source.lambdas.is_none() => return None,
                _ => checked_binding(lambda, source)?,
            },
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
                |_, _, _| false,
                |_| true,
                |child| self.operand(child, source).ok_or(()),
            )
            .ok()??,
        };
        Some(source.located_operation(node, value))
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
                    site: ir::CallSite::Python(site),
                    ..
                },
            ..
        } => site.scope = Some(Box::new(namespace)),
        ExprKind::Builtin(
            Builtin::RegisteredLambda(_, scoped) | Builtin::RegisteredGenerator(_, scoped, _),
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
        let source = module.operands(self.source, self.graph.python_optimization());
        if let ast::Expr::Await(awaited) = node
            && self.asynchronous
            && self.registered_local_start.is_some()
        {
            let value = self.expr(&awaited.value)?;
            let value = self.task_await(value, false, awaited)?;
            self.frame_arguments = FrameArguments::Python;
            return Ok(Some(source.located(node, value)));
        }
        if let ast::Expr::FString(value) = node
            && self.registered_local_start.is_some()
        {
            let value = fstrings::registered_fstring(value, source, |child| self.expr(child))?;
            self.frame_arguments = FrameArguments::Python;
            return Ok(Some(source.located(node, value)));
        }
        if let ast::Expr::Generator(generator) = node
            && self.registered_local_start.is_some()
        {
            let mut value = generator_binding(generator, |outer| self.expr(outer))?;
            self.frame_arguments = FrameArguments::Python;
            if let Some(slot) = self.namespace {
                in_namespace(&mut value, self.local(slot));
            }
            return Ok(Some(source.located(node, value)));
        }
        if let ast::Expr::Lambda(lambda) = node {
            // The enclosing frame evaluates defaults. Its lexical owner prepares the body.
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
        let declaration = match node.as_call_expr() {
            Some(call)
                if let Some(name) = call.func.as_name_expr()
                    && let Some(&slot) = self.names.get(name.id.as_str()) =>
            {
                if self.locals[slot] != Type::PythonObject {
                    // The fixed call checker binds the positional signature and result type.
                    return Ok(None);
                }
                self.narrowed
                    .get(&slot)
                    .filter(|_| self.checked_call_contracts && self.inference.is_none())
                    .and_then(|&ty| self.saved_method(ty))
                    .filter(|(method, _)| !method.asynchronous)
                    .map(|(method, receiver)| (Some(method), usize::from(receiver.is_some()), None))
            }
            Some(call)
                if self.checked_call_contracts
                    && let Some(Binding::Definition(id)) = self.resolve(&call.func)
                    && !self.classes.declarations.ids.contains_key(&id) =>
            {
                Some((self.functions.get(&id), 0, Some(id)))
            }
            _ => None,
        };
        if let ast::Expr::Named(named) = node {
            let value = self.named_expression(named, None)?;
            return Ok(Some(source.located(node, value)));
        }
        if let ast::Expr::BinOp(binary) = node {
            let value = self.binary_operation(binary)?;
            return Ok(Some(source.located(node, value)));
        }
        if let ast::Expr::Call(call) = node
            && let Some(method) = stdlib::registered_string_method(call)
            && self.graph.session.scalar_type(self.module, &method.value) == Some(Type::Str)
        {
            // The receiver has a scalar layout. Place this method call in the same
            // checked frame as its operands and let normal attribute lookup bind it.
            let value = self.call(call)?;
            return Ok(Some(source.located(node, value)));
        }
        if let Some(name) = node.as_name_expr()
            && let Some(&slot) = self.names.get(name.id.as_str())
        {
            return Ok(Some(source.located(node, self.local(slot))));
        }
        if self.checked_call_contracts
            && let Some(call) = node.as_call_expr()
            && let Some(method) = call.func.as_attribute_expr()
            && let Some(receiver) = method.value.as_name_expr()
            && let Some(&slot) = self.names.get(receiver.id.as_str())
            && self.types.element(self.locals[slot]).is_some()
            && source.direct_method(call).is_some()
        {
            // Let the list method check its written operands before binding the live method.
            let receiver = self.expr(&method.value)?;
            let mut value = self.call_on(call, Some(receiver))?;
            if value.python_call().is_none() {
                return Err(self.scoped_builtin_error("declared list method", call));
            }
            if let Some(slot) = self.namespace {
                in_namespace(&mut value, self.local(slot));
            }
            return Ok(Some(source.located_operation(node, value)));
        }
        if let Some((signature, skip, definition)) = declaration
            && let ast::Expr::Call(call) = node
        {
            let refused = || self.scoped_builtin_error("declared Python call", call);
            let signature = signature.ok_or_else(refused)?;
            if !call.func.is_name_expr() {
                return Err(refused());
            }
            let indices = calls::checked_parameter_indices(call, &signature.params[skip..])
                .ok_or_else(refused)?;
            let target = self.expr(&call.func)?;
            // A live Python argument can satisfy an inferred layout only at dispatch.
            // Forward it unchanged and leave the result untyped on that edge.
            let mut fixed_result = true;
            // Positional calls use their written order. Keyword calls already have a slot per operand.
            let mut indices = indices.into_iter().chain(0..call.arguments.len());
            let value = callable_target(call).prepare(call, target, |arg| {
                let index = skip + indices.next().expect("checked parameter for each argument");
                let parameter = &signature.params[index];
                let value = self.expr(arg)?;
                let actual = value.checked_operand_type();
                let expected = self.inference.map_or(parameter.ty, |inference| {
                    inference.learn_parameter(signature.id, index, actual)
                });
                if let Err(error) = self.expect(expected, actual, arg) {
                    let declaration = signature.parameter_declaration(index);
                    let contract = definition
                        .zip(declaration)
                        .filter(|(id, decl)| decl.module == id.module && expected == parameter.ty)
                        .and_then(|(id, decl)| {
                            self.graph
                                .session
                                .operand_types(self.module)
                                .argument_contract(arg, id, index, decl.annotation)
                        });
                    match contract {
                        Some(crate::analysis::ArgumentContract::Checked) => {}
                        _ if actual == Type::PythonObject
                            && self.inference.is_none()
                            && (declaration.is_none()
                                || matches!(
                                    contract,
                                    Some(crate::analysis::ArgumentContract::Dynamic)
                                )) =>
                        {
                            fixed_result = false;
                        }
                        _ => return Err(error.expected_from(declaration, arg.range())),
                    }
                }
                Ok(value)
            })?;
            let mut located = source.located_operation(node, self.scoped_call(value));
            let fixed_result = fixed_result
                && !signature.asynchronous
                && !self.classes.generators.contains(&signature.id);
            if definition.is_none() {
                if fixed_result {
                    located.ty = signature.returns;
                }
            } else if fixed_result && let Some((_, hint)) = located.source_hint_mut() {
                *hint = Some(self.enter_source_call(call, signature)?);
            } else {
                self.calls.insert(signature.id);
            }
            return Ok(Some(located));
        }
        let graph = self.graph;
        let owner = self.module;
        let mut frame_identity = false;
        if let ast::Expr::Compare(cmp) = node {
            let frame_op = self
                .registered_local_start
                .and(cmp.as_single())
                .map(|(_, op, _)| *op);
            frame_identity = matches!(frame_op, Some(ast::CmpOp::Is | ast::CmpOp::IsNot));
            let opaque = |operand: &ast::Expr| {
                let receiver = match operand {
                    ast::Expr::Call(call) => match call.func.as_ref() {
                        ast::Expr::Attribute(field)
                            if field.value.is_name_expr()
                                && source.direct_method(call).is_some() =>
                        {
                            field.value.as_ref()
                        }
                        callee => callee,
                    },
                    ast::Expr::Attribute(field) => field.value.as_ref(),
                    _ => operand,
                };
                let selected = operand
                    .as_subscript_expr()
                    .map_or(receiver, |subscript| subscript.value.as_ref());
                selected
                    .as_name_expr()
                    .and_then(|name| self.names.get(name.id.as_str()))
                    .is_some_and(|&slot| self.locals[slot] == Type::PythonObject)
                    || graph.session.is_dynamic_receiver(owner, receiver)
            };
            let mut previous_fixed = false;
            if matches!(frame_op, Some(ast::CmpOp::Eq | ast::CmpOp::NotEq))
                || !frame_identity
                    && cmp.operands.iter().map(opaque).any(|dynamic| {
                        let fixed_pair = previous_fixed && !dynamic;
                        previous_fixed = !dynamic;
                        fixed_pair
                    })
            {
                if frame_op.and_then(ir::Compare::from_operator).is_none() {
                    return Ok(None);
                }
                let checked = self.fixed_expr(node)?;
                return Ok((checked.ty == Type::PythonObject
                    && matches!(checked.kind, ExprKind::Compare(..)))
                .then(|| source.located(node, checked)));
            }
        }
        let first_local = self.registered_local_start;
        let python_frame = self.frame_arguments == FrameArguments::Python;
        let operands = graph.session.operand_types(owner);
        let is_bound_method = |callee: &ast::Expr| operands.is_bound_method(callee);
        let class = self.class;
        // Rewritten syntax can share a physical range with another source operation.
        let placed = self.settled_attributes.as_deref_mut().and_then(|sites| {
            if node.is_attribute_expr() {
                return sites.remove(&node.range());
            }
            let call = node.as_call_expr()?;
            let std::collections::hash_map::Entry::Occupied(site) = sites.entry(call.range()) else {
                return None;
            };
            match site.get() {
                FixedAttribute::CallValue(saved)
                    if self.namespace.is_none()
                        && first_local.is_some_and(|first| saved.slot < first)
                        && self.names.get(saved.parameter.as_str()) == Some(&saved.slot)
                        && self.locals[saved.slot] == saved.ty
                        && source.direct_method(call).is_some() == saved.call.python_call().is_some_and(|call| matches!(call.target, ir::RegisteredCallTarget::Method { .. }))
                        && operands.is_original_operand(node) => {}
                _ if self.namespace.is_none()
                    && call.func.as_name_expr().or_else(|| call.func.as_attribute_expr()?.value.as_name_expr())
                        .is_some_and(|name| !self.names.contains_key(name.id.as_str())) =>
                {
                    let attr = site.remove();
                    return matches!(&attr, FixedAttribute::Call(Some(_), saved)
                        if saved.target == callable_target(call) && operands.is_original_operand(&call.func))
                        .then_some(attr);
                }
                FixedAttribute::Call(None, _) if operands.is_original_operand(node) => {}
                _ => return None,
            }
            Some(site.remove())
        });
        let placed = match (node.as_call_expr(), placed) {
            (Some(_), Some(FixedAttribute::CallValue(saved))) => {
                return Ok(Some(source.located_operation(node, saved.call)));
            }
            (Some(call), Some(FixedAttribute::Call(Some(target), mut site))) => {
                // Forwarding can preserve the imported target and replace its arguments.
                let (layout, args) = prepared_arguments(call, |arg| self.expr(arg))?;
                site.arguments = layout;
                return Ok(Some(
                    source.located_operation(node, invoke_value(site, target, args)),
                ));
            }
            (_, attribute) => attribute,
        };
        let mut live_receiver = false;
        let Some(mut value) = operation(
            node,
            source,
            placed,
            |method, receiver, operand| {
                let Some(first) = first_local else {
                    return false;
                };
                // This operand and its frame already use the Python protocol.
                if python_frame && operand.ty == Type::PythonObject {
                    return true;
                }
                let local = match operand.without_location().kind {
                    ExprKind::Local(slot) if receiver.is_name_expr() => Some(slot),
                    _ => None,
                };
                let parameter = local.is_some_and(|slot| slot < first);
                if let Some(callee) = method {
                    let list_parameter =
                        parameter && matches!(operand.ty, Type::ListInt | Type::List(_));
                    live_receiver = !list_parameter
                        && (local.is_some() || operand.ty == Type::Str)
                        && is_bound_method(callee);
                    list_parameter || live_receiver
                } else {
                    node.as_attribute_expr().is_some_and(|attribute| {
                        // A bound method reads this attribute on its live Python receiver.
                        live_receiver =
                            method_receiver(first_local, class, &attribute.value, operand);
                        local.is_some()
                    })
                }
            },
            // Even a skipped operand participates in static argument and result checking.
            |receiver| {
                first_local.is_some()
                    || graph.session.is_dynamic_receiver(owner, receiver)
                    || node
                        .as_call_expr()
                        .is_some_and(|call| is_bound_method(&call.func))
            },
            |child| self.expr(child),
        )?
        else {
            return Ok(None);
        };
        if frame_identity
            && let ExprKind::Compare(values, _) = &value.kind
            && !values
                .iter()
                .any(|operand| operand.ty == Type::PythonObject)
        {
            return Ok(None);
        }
        if let Some(slot) = self.namespace {
            in_namespace(&mut value, self.local(slot));
        }
        if live_receiver {
            self.frame_arguments = FrameArguments::Python;
        }
        // Checked locals and exact literals keep their known types in the registered frame.
        value.ty = value
            .kind
            .scalar_literal_type()
            .unwrap_or_else(|| match &value.kind {
                ExprKind::Conditional(_, yes, no) if yes.ty == no.ty => yes.ty,
                _ => self
                    .registered_display_type(&value.kind)
                    .unwrap_or(value.ty),
            });
        if let ExprKind::Set { element, .. } = &mut value.kind
            && let Some(known) = self.types.set_element(value.ty)
        {
            *element = known;
        }
        Ok(Some(source.located_operation(node, value)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn value_and_position(value: &Expr) -> (&ExprKind, TextSize) {
        let (_, position, _) = value
            .source_position()
            .expect("expected a located expression");
        (&value.without_location().kind, position)
    }

    fn operation_and_position(value: &Expr) -> (&Builtin, &[Expr], TextSize) {
        let (ExprKind::Builtin(op, operands), position) = value_and_position(value) else {
            panic!("expected a registered operation");
        };
        (op, operands, position)
    }

    #[test]
    fn selected_builtin_keeps_a_live_positional_call() {
        let parsed = ruff_python_parser::parse_module("globals()").unwrap();
        let call = parsed.syntax().body[0]
            .as_expr_stmt()
            .unwrap()
            .value
            .as_call_expr()
            .unwrap();
        let result = name_call(call, "globals", Vec::new());
        assert!(result.is_registered_invoke());
        let ExprKind::Invoke { callable, args } = result.kind else {
            unreachable!()
        };
        assert!(
            matches!(callable.kind, ExprKind::Builtin(Builtin::RegisteredName(ref name), _) if name == "globals")
        );
        assert_eq!(args.site.python().unwrap().call.range, call.range());
    }

    #[test]
    fn attribute_and_direct_method_use_the_physical_token_and_resolve_imported_receivers_normally()
    {
        let text = "import sys as library\n(holder\n .method)(arg)\n(library\n .method)(arg)\n(holder\n .method)\n";
        let parsed = imre_ast::indentation::prepare(text).parse_module().unwrap();
        let imports = imre_ast::module_imports(&parsed.syntax().body);
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            replacements: &HashMap::new(),
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
        let (ExprKind::Invoke { args, .. }, pos) = value_and_position(&values[0]) else {
            panic!("expected a direct method invocation");
        };
        let ir::RegisteredCallTarget::Method {
            name,
            attribute,
            name_start,
        } = &args.site.python().unwrap().call.target
        else {
            panic!("expected the physical method target");
        };
        assert_eq!(name, "method");
        assert_eq!(*name_start, pos);
        assert_eq!(args.values.len(), 1);
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
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            replacements: &HashMap::new(),
            method_imports: &BTreeSet::new(),
            private_names: None,
        };
        let mut calls = parsed.syntax().body.iter().map(|stmt| {
            let expr = &stmt.as_expr_stmt().unwrap().value;
            registered_value(expr, source).unwrap()
        });
        let method = calls.next().unwrap();
        let (ExprKind::Invoke { callable, args }, _) = value_and_position(&method) else {
            panic!("expected a checked method invocation");
        };
        let method = &args.site.python().unwrap().call;
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
        let names: Vec<_> = std::iter::once(callable.as_ref())
            .chain(&args.values)
            .map(|value| match operation_and_position(value).0 {
                Builtin::RegisteredName(name) => name.as_str(),
                _ => panic!("expected named operand"),
            })
            .collect();
        assert_eq!(names, ["obj", "value", "arg"]);
        let call = calls.next().unwrap();
        let (ExprKind::Invoke { callable, args }, _) = value_and_position(&call) else {
            panic!("expected a checked packed invocation");
        };
        assert_eq!(
            args.site.python().unwrap().call.arguments,
            ir::RegisteredArguments::Packed(ir::PackedArguments {
                starred: vec![true],
                keywords: vec![Some("first".into()), None, Some("last".into())],
            })
        );
        let names: Vec<_> = std::iter::once(callable.as_ref())
            .chain(&args.values)
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
        assert!(args.order.is_empty());
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
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            replacements: &HashMap::new(),
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
                python_optimization: 0,
                operand_types: None,
                template_fields: None,
                generators: None,
                lambdas: None,
                text: &text,
                replacements: &replacements,
                method_imports: &imports,
                private_names: None,
            };
            let value = registered_value(&stmt.value, source).unwrap();
            let (ExprKind::Invoke { args, .. }, _) = value_and_position(&value) else {
                panic!("expected a Python invocation");
            };
            let call = &args.site.python().unwrap().call;
            assert_eq!(args.values.len(), count);
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
                python_optimization: 0,
                operand_types: None,
                template_fields: None,
                generators: None,
                lambdas: None,
                text,
                replacements: &replacements,
                method_imports: &imports,
                private_names: None,
            };
            let value = registered_value(&stmt.value, source).unwrap();
            let mut calls = Vec::new();
            ir::walk_operand_expressions(&value, &mut |expr| {
                if let ExprKind::Invoke { args, .. } = &expr.kind {
                    let call = &args.site.python().unwrap().call;
                    calls.push((call.range, call.partition_field, args.values.len()));
                }
            });
            let subscript = stmt.value.as_subscript_expr().unwrap();
            assert_eq!(
                calls,
                [(
                    subscript.value.range(),
                    fused.map(|i| (i, subscript.range())),
                    1
                )]
            );
            assert_eq!(
                matches!(
                    value_and_position(&value).0,
                    ExprKind::Builtin(Builtin::RegisteredSubscript(..), _)
                ),
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
        let mut inputs = Vec::new();
        let value = index_operation(expr, module.context(source), |child, checked| {
            let slot = inputs.len();
            inputs.push((&source[child.range()], checked.is_some()));
            Expr::registered_input(slot, Type::PythonObject)
        })
        .unwrap();
        assert_eq!(
            inputs,
            [
                ("owner()", true),
                ("1000", true),
                ("next(i for i in items)", false),
                ("1001", true)
            ]
        );
        let mut slots = Vec::new();
        ir::walk_operand_expressions(&value, &mut |operand| {
            if let ExprKind::Builtin(Builtin::RegisteredInput(slot), _) = operand.kind {
                slots.push(slot);
            }
        });
        assert_eq!(slots, [0, 1, 2, 3]);
        assert!(matches!(
            value.kind,
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), _)
        ));
    }

    #[test]
    fn subscript_text_retains_the_source_literal_and_its_position() {
        for source in [r#"x["id"]"#, r#"x["a b"]"#, r#"x["\ud800"]"#] {
            let graph = SourceGraph::single(source).unwrap();
            let entry = &graph.modules[graph.entry];
            let module = ModuleSource::new(entry);
            let statement = entry.parsed.syntax().body[0].as_expr_stmt().unwrap();
            let key = &statement.value.as_subscript_expr().unwrap().slice;
            let value = registered_index_key(key, module.context(source)).unwrap();
            assert!(matches!(&value.kind,
                ExprKind::Builtin(Builtin::RegisteredLocation(range, _, _), _)
                    if *range == key.range()));
            assert!(matches!(
                value.without_location().kind,
                ExprKind::Str(_) | ExprKind::Builtin(Builtin::RegisteredString(_), _)
            ));
        }
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
