//! Prepare interpolated string literals and checked template operands before code generation.
use super::*;
use imre_ast::strings::{TemplateFieldText, interpolation_conversion};

#[derive(Default)]
pub(crate) struct PreparedStrings {
    pub(crate) fields: HashMap<TextSize, TemplateFieldText>,
    pub(crate) templates: HashMap<TextRange, PreparedTemplate>,
}

pub(crate) enum PreparedTemplate {
    Checked(RegisteredOperand),
    Mixed {
        segments: Vec<Vec<u8>>,
        fields: Vec<TemplateField>,
    },
}

pub(crate) enum TemplateField {
    Checked(RegisteredOperand),
    External {
        value: Box<ast::Expr>,
        interpolation: Expr,
    },
    Unsupported(&'static str),
}

impl PreparedTemplate {
    /// Keep the checked fields in order when an input still needs its enclosing source entry.
    pub(crate) fn prepare(
        expr: &ast::Expr,
        source: RegisteredSource<'_>,
        owner: RegisteredScope,
    ) -> Option<Self> {
        if let Some(checked) = RegisteredOperand::prepare(expr, source, owner) {
            return Some(Self::Checked(checked));
        }
        let template = expr.as_t_string_expr()?;
        let segments = template_segments(template, source, source.template_fields?)?;
        let mut fields = Vec::new();
        for field in template
            .value
            .iter()
            .flat_map(|part| part.elements.interpolations())
        {
            // The incomplete-site entry rejected these specs before evaluating the value.
            if let Err(reason) = imre_ast::strings::template_format_spec(field) {
                fields.push(TemplateField::Unsupported(reason));
                break;
            }
            let checked = RegisteredOperand::prepare_with(source, owner, |scope| {
                scope.with_suspending_operands(|scope| {
                    source.interpolation(field, |child| scope.ordinary_operand(child))
                })
            });
            fields.push(match checked {
                Some(value) => TemplateField::Checked(value),
                None => TemplateField::External {
                    value: field.expression.clone(),
                    // The only input of this literal-spec interpolation is its value.
                    interpolation: source
                        .interpolation(field, |_| Some(Expr::local(0, Type::PythonObject)))?,
                },
            });
        }
        Some(Self::Mixed { segments, fields })
    }
}

impl RegisteredSource<'_> {
    pub(crate) fn formatted_literal(self, literal: &ast::InterpolatedStringLiteralElement) -> Expr {
        if let Some(replacements) = self.format_replacements(literal) {
            let mut bytes = Vec::new();
            values::push_codepoints(&mut bytes, &literal.value, Some(&replacements));
            object(Builtin::RegisteredString(bytes), vec![])
        } else {
            text(literal.value.to_string())
        }
    }
}

fn object(op: Builtin, operands: Vec<Expr>) -> Expr {
    operation(op, Type::PythonObject, operands)
}

fn text(s: impl Into<String>) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Str(s.into()),
    }
}

fn literal_points(
    source: RegisteredSource<'_>,
    literal: &ast::InterpolatedStringLiteralElement,
    buffer: &mut Vec<u8>,
) {
    let replacements = source.format_replacements(literal);
    values::push_codepoints(buffer, &literal.value, replacements.as_deref());
}

fn template_segments(
    template: &ast::ExprTString,
    source: RegisteredSource<'_>,
    fields: &HashMap<TextSize, TemplateFieldText>,
) -> Option<Vec<Vec<u8>>> {
    let mut strings = Vec::new();
    let mut segment = Vec::new();
    for element in template.value.elements() {
        match element {
            ast::InterpolatedStringElement::Literal(literal) => {
                literal_points(source, literal, &mut segment)
            }
            ast::InterpolatedStringElement::Interpolation(field) => {
                if let Some(prefix) = &fields.get(&field.start())?.debug_prefix {
                    values::push_codepoints(&mut segment, prefix, None);
                }
                strings.push(std::mem::take(&mut segment));
            }
        }
    }
    strings.push(segment);
    Some(strings)
}

impl RegisteredSource<'_> {
    /// Bind every field through the caller so names, calls and compiler-owned slots
    /// observe the same lexical scope as siblings in the enclosing expression.
    pub(super) fn template(
        self,
        template: &ast::ExprTString,
        in_function: bool,
        mut operand: impl FnMut(&ast::Expr) -> Option<Expr>,
    ) -> StdResult<Expr, Option<&'static str>> {
        let fields = self.template_fields.ok_or(None)?;
        let segments = template_segments(template, self, fields).ok_or(None)?;
        let mut captures = Vec::new();
        let mut residual_error = None;
        for field in template
            .value
            .iter()
            .flat_map(|part| part.elements.interpolations())
        {
            // A residual field checks its spelling and format before evaluating its value.
            residual_error = residual_error.or_else(|| {
                match imre_ast::strings::template_format_spec(field).err()? {
                    "debug t-string interpolation" => Some(
                        "unsupported debug t-string interpolation in a compiled suspended function",
                    ),
                    "nested t-string format spec" => Some(
                        "unsupported nested t-string format spec in a compiled suspended function",
                    ),
                    _ => None,
                }
            });
            captures.push(
                self.interpolation(field, &mut operand)
                    .ok_or(residual_error)?,
            );
        }
        let value = object(Builtin::RegisteredTemplate(segments), captures);
        if !in_function {
            // A context-free template plan cannot suspend in an unknown activation.
            let mut suspends = false;
            ir::walk_operand_expressions(&value, &mut |child| {
                suspends |= matches!(
                    child.kind,
                    ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
                );
            });
            if suspends {
                return Err(None);
            }
        }
        Ok(value)
    }

    fn interpolation(
        self,
        field: &ast::InterpolatedElement,
        mut operand: impl FnMut(&ast::Expr) -> Option<Expr>,
    ) -> Option<Expr> {
        let expression = self
            .template_fields?
            .get(&field.start())?
            .expression
            .clone();
        Some(object(
            Builtin::RegisteredInterpolation {
                expression,
                conversion: interpolation_conversion(field),
            },
            vec![
                operand(&field.expression)?,
                self.template_spec(field, &mut operand)?,
            ],
        ))
    }

    fn template_spec(
        self,
        field: &ast::InterpolatedElement,
        operand: &mut impl FnMut(&ast::Expr) -> Option<Expr>,
    ) -> Option<Expr> {
        let Some(spec) = &field.format_spec else {
            return Some(text(""));
        };
        let mut parts = Vec::new();
        for element in &spec.elements {
            let part = match element {
                ast::InterpolatedStringElement::Literal(literal) if literal.value.is_empty() => {
                    continue;
                }
                ast::InterpolatedStringElement::Literal(literal) => self.formatted_literal(literal),
                ast::InterpolatedStringElement::Interpolation(nested) => {
                    if let Some(prefix) = &self.template_fields?.get(&nested.start())?.debug_prefix
                    {
                        parts.push(text(prefix));
                    }
                    let mut value = operand(&nested.expression)?;
                    if let Some(c) = interpolation_conversion(nested) {
                        value = object(Builtin::FormatConversion(c), vec![value]);
                    }
                    let value = object(
                        Builtin::FormatField(Box::default()),
                        vec![value, self.template_spec(nested, operand)?],
                    );
                    object(
                        Builtin::RegisteredLocation(nested.range(), nested.start()),
                        vec![value],
                    )
                }
            };
            parts.push(part);
        }
        match parts.len() {
            0 => Some(text("")),
            1 => parts.pop(),
            _ => Some(object(Builtin::FormatJoin, parts)),
        }
    }
}

#[cfg(test)]
mod tests;
