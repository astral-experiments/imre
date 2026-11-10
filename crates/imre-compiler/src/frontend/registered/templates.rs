//! Prepare interpolated string literals and checked template operands before code generation.
use super::*;
use ast::visitor::{self, Visitor};
use imre_ast::strings::{TemplateFieldText, interpolation_conversion};

#[derive(Default)]
pub(crate) struct PreparedStrings {
    pub(crate) fields: HashMap<TextSize, TemplateFieldText>,
    pub(crate) templates: HashMap<TextRange, RegisteredOperand>,
    pub(crate) literals: HashMap<TextRange, Expr>,
    pub(crate) residual_templates: HashMap<TextRange, ResidualTemplate>,
}

pub(crate) struct ResidualTemplate {
    pub(crate) segments: Vec<Vec<u8>>,
    pub(crate) fields: Vec<StdResult<ResidualField, &'static str>>,
}

pub(crate) struct ResidualField {
    pub(crate) expression: String,
    pub(crate) conversion: Option<char>,
    pub(crate) spec: Expr,
}

/// Only a residual expression borrows the original values at its physical site.
pub(crate) fn template_operands(expr: &ast::Expr) -> Option<impl Iterator<Item = &ast::Expr>> {
    let template = expr.as_t_string_expr()?;
    Some(template.value.iter().flat_map(|part| {
        part.elements
            .interpolations()
            .map(|field| field.expression.as_ref())
    }))
}

struct ExpressionPreparation<'a> {
    source: RegisteredSource<'a>,
    prepared: PreparedStrings,
    generators: HashMap<TextRange, RegisteredGenerator>,
    // Template text is owned by its segments. Nested expressions can switch string kinds.
    in_f_string: bool,
}

/// Prepare nested generator bodies before a parent expression binds or captures them.
pub(crate) fn prepare_expression_sites(
    body: &[ast::Stmt],
    source: RegisteredSource<'_>,
) -> (PreparedStrings, HashMap<TextRange, RegisteredGenerator>) {
    let mut preparation = ExpressionPreparation {
        source,
        prepared: PreparedStrings::default(),
        generators: HashMap::new(),
        in_f_string: false,
    };
    preparation.visit_body(body);
    (preparation.prepared, preparation.generators)
}

impl<'a> Visitor<'a> for ExpressionPreparation<'_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        let enclosing =
            std::mem::replace(&mut self.in_f_string, matches!(expr, ast::Expr::FString(_)));
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            visitor::walk_expr(self, expr)
        });
        self.in_f_string = enclosing;
        let source = RegisteredSource {
            generators: Some(&self.generators),
            ..self.source
        };
        match expr {
            ast::Expr::Generator(generator) => {
                if let Some(child) = RegisteredGenerator::prepare(generator, source) {
                    self.generators.insert(generator.range(), child);
                }
            }
            ast::Expr::TString(template) => {
                if let Some(value) =
                    RegisteredOperand::prepare(expr, source, RegisteredScope::Module)
                {
                    self.prepared.templates.insert(expr.range(), value);
                } else if let Some(fields) = self.source.template_fields
                    && let Some(segments) = template_segments(template, self.source, fields)
                {
                    let fields = template
                        .value
                        .iter()
                        .flat_map(|part| part.elements.interpolations())
                        .map(|field| {
                            let spec = match imre_ast::strings::template_format_spec(field)? {
                                Some(literal) => self.source.formatted_literal(literal),
                                None => text(""),
                            };
                            Ok(ResidualField {
                                expression: fields[&field.start()].expression.clone(),
                                conversion: field.conversion.to_char(),
                                spec,
                            })
                        })
                        .collect();
                    self.prepared
                        .residual_templates
                        .insert(expr.range(), ResidualTemplate { segments, fields });
                }
            }
            ast::Expr::FString(formatted) => {
                for part in formatted.value.iter() {
                    if let ast::FStringPartRef::Literal(literal) = part
                        && !literal.value.is_empty()
                    {
                        let value = self
                            .source
                            .string(std::iter::once(FStringLiteral::Adjacent(literal)));
                        self.prepared.literals.insert(literal.range(), value);
                    }
                }
            }
            _ => {}
        }
    }

    fn visit_interpolated_string_element(&mut self, element: &'a ast::InterpolatedStringElement) {
        if self.in_f_string {
            match element {
                ast::InterpolatedStringElement::Literal(literal) if !literal.value.is_empty() => {
                    self.prepared
                        .literals
                        .insert(literal.range(), self.source.formatted_literal(literal));
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(debug) = &field.debug_text {
                        self.prepared
                            .literals
                            .insert(field.range(), text(debug.as_str()));
                    }
                }
                _ => {}
            }
        }
        visitor::walk_interpolated_string_element(self, element);
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
    ) -> Option<Expr> {
        let fields = self.template_fields?;
        let segments = template_segments(template, self, fields)?;
        let mut captures = Vec::new();
        for field in template
            .value
            .iter()
            .flat_map(|part| part.elements.interpolations())
        {
            let expression = fields[&field.start()].expression.clone();
            captures.push(object(
                Builtin::RegisteredInterpolation {
                    expression,
                    conversion: interpolation_conversion(field),
                },
                vec![
                    operand(&field.expression)?,
                    self.template_spec(field, &mut operand)?,
                ],
            ));
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
                return None;
            }
        }
        Some(value)
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
