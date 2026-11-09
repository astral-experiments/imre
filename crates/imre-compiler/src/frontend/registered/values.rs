//! Lower values evaluated against a registered Python scope into the expression IR.
use super::*;

pub(in crate::frontend) struct Literals {
    replacements: HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
}

impl Literals {
    pub(in crate::frontend) fn new(module: &crate::modules::SourceModule) -> Self {
        let tokens = module.parsed.tokens();
        Self {
            replacements: literal_replacements(&module.source, tokens),
            has_templates: tokens
                .iter()
                .any(|token| token.kind() == ast::token::TokenKind::TStringStart),
        }
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

/// Fixed positional object calls use at most the registered entry block's scratch limit.
/// A method receiver stays with descriptor and direct-method lowering.
pub(crate) fn registered_positional_call(call: &ast::ExprCall) -> bool {
    !call.func.is_attribute_expr() && fixed_positional_arguments(call)
}

pub(in crate::frontend) fn fixed_positional_arguments(call: &ast::ExprCall) -> bool {
    call.arguments.keywords.is_empty()
        && call.arguments.args.len() <= 64
        && !call.arguments.args.iter().any(ast::Expr::is_starred_expr)
}

/// Bind a live callee to the positional values in their source evaluation order.
pub(in crate::frontend) fn positional_call<E>(
    call: &ast::ExprCall,
    callable: Expr,
    mut operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    let mut operands = Vec::with_capacity(1 + call.arguments.args.len());
    operands.push(callable);
    for argument in &call.arguments.args {
        operands.push(operand(argument)?);
    }
    Ok(positional_operands(call, operands))
}

/// Operands start with the live callee, followed by its arguments in source order.
pub(in crate::frontend) fn positional_operands(call: &ast::ExprCall, operands: Vec<Expr>) -> Expr {
    let name = call.func.is_name_expr().then(|| call.func.range());
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(
            Builtin::RegisteredCall(call.range(), call.func.end(), name),
            operands,
        ),
    }
}

/// Select one registered-frame operation before requesting its operands in source order.
fn operation<E>(
    node: &ast::Expr,
    replacements: &HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
    local: Option<Expr>,
    mut child: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Option<Expr>, E> {
    let kind = match node {
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
        ast::Expr::NumberLiteral(lit) => match &lit.value {
            ast::Number::Int(n) => ExprKind::Int(n.to_string()),
            ast::Number::Float(n) => ExprKind::Float(*n),
            ast::Number::Complex { real, imag } => {
                ExprKind::Builtin(Builtin::RegisteredComplex(*real, *imag), vec![])
            }
        },
        ast::Expr::StringLiteral(lit) => {
            return Ok(Some(registered_string(
                lit.value.to_str(),
                lit.value.iter(),
                replacements,
                has_templates,
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
        ast::Expr::Subscript(index) if !matches!(&*index.slice, ast::Expr::Slice(_)) => {
            ExprKind::Builtin(
                Builtin::RegisteredSubscript(node.range(), index.value.end()),
                vec![child(&index.value)?, child(&index.slice)?],
            )
        }
        ast::Expr::Call(call) if registered_positional_call(call) => {
            let callable = child(&call.func)?;
            return positional_call(call, callable, child).map(Some);
        }
        _ => return Ok(None),
    };
    Ok(Some(Expr {
        ty: Type::PythonObject,
        kind,
    }))
}

/// Build a complete expression region; an unsupported child keeps its original lowering.
pub(crate) fn registered_value(
    node: &ast::Expr,
    replacements: &HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
) -> Option<Expr> {
    operation(node, replacements, has_templates, None, |child| {
        registered_value(child, replacements, has_templates).ok_or(())
    })
    .ok()?
    .map(|value| located(node, value))
}

impl Checker<'_> {
    pub(in crate::frontend) fn registered_operand(
        &mut self,
        node: &ast::Expr,
        literals: &Literals,
    ) -> Result<Option<Expr>, String> {
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
        let Some(mut value) = operation(
            node,
            &literals.replacements,
            literals.has_templates,
            local,
            |child| self.expr(child),
        )?
        else {
            return Ok(None);
        };
        if let ExprKind::Builtin(Builtin::RegisteredCall(..), operands) = &value.kind
            && operands[0].ty != Type::PythonObject
        {
            return Err(error(
                self.source,
                node,
                "callee requires its checked calling convention",
            )
            .classify(Kind::ScopedObject));
        }
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
            return Ok(Some(located(node, value)));
        }
        // Checked locals and exact literals keep their known types in the registered frame.
        value.ty = match &value.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Str(_) | ExprKind::Builtin(Builtin::RegisteredString(_), _) => Type::Str,
            ExprKind::Bytes(_) => Type::Bytes,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::None => Type::None,
            _ => value.ty,
        };
        Ok(Some(located(node, value)))
    }
}
