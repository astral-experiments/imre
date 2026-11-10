//! Prepare interpolated string literals and checked template operands before code generation.
use super::*;
use ast::visitor::{self, Visitor};
use imre_ast::strings::{TemplateFieldText, interpolation_conversion};

#[derive(Default)]
pub(crate) struct PreparedStrings {
    pub(crate) templates: HashMap<TextRange, Expr>,
    pub(crate) literals: HashMap<TextRange, Expr>,
    pub(crate) residual_templates: HashMap<TextRange, ResidualTemplate>,
}

pub(crate) struct ResidualTemplate {
    pub(crate) segments: Vec<Vec<u8>>,
    pub(crate) specs: Vec<StdResult<Expr, &'static str>>,
}

struct StringPreparation<'a> {
    source: RegisteredSource<'a>,
    fields: &'a HashMap<TextSize, TemplateFieldText>,
    prepared: PreparedStrings,
    // Template text is owned by its segments. Nested expressions can switch string kinds.
    in_f_string: bool,
}

/// A template with a residual operand stays at its original whole-expression boundary.
pub(crate) fn prepare_strings(
    body: &[ast::Stmt],
    source: RegisteredSource<'_>,
    fields: &HashMap<TextSize, TemplateFieldText>,
) -> PreparedStrings {
    let mut preparation = StringPreparation {
        source,
        fields,
        prepared: PreparedStrings::default(),
        in_f_string: false,
    };
    preparation.visit_body(body);
    preparation.prepared
}

impl<'a> Visitor<'a> for StringPreparation<'_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        // Prepare nested t-strings before lowering their enclosing operands.
        let enclosing =
            std::mem::replace(&mut self.in_f_string, matches!(expr, ast::Expr::FString(_)));
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            visitor::walk_expr(self, expr)
        });
        self.in_f_string = enclosing;
        match expr {
            ast::Expr::TString(template) => {
                if let Some(segments) = template_segments(template, self.source, self.fields) {
                    if let Some(fields) = self.checked_fields(template) {
                        self.prepared.templates.insert(
                            expr.range(),
                            object(Builtin::RegisteredTemplate(segments), fields),
                        );
                    } else {
                        let specs = template
                            .value
                            .iter()
                            .flat_map(|part| part.elements.interpolations())
                            .map(|field| {
                                imre_ast::strings::template_format_spec(field).map(|literal| {
                                    match literal {
                                        Some(literal) => self.source.formatted_literal(literal),
                                        None => text(""),
                                    }
                                })
                            })
                            .collect();
                        self.prepared
                            .residual_templates
                            .insert(expr.range(), ResidualTemplate { segments, specs });
                    }
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

impl StringPreparation<'_> {
    fn checked_fields(&self, template: &ast::ExprTString) -> Option<Vec<Expr>> {
        let mut parts = Vec::new();
        for field in template
            .value
            .iter()
            .flat_map(|part| part.elements.interpolations())
        {
            let expression = self.fields.get(&field.start())?.expression.clone();
            parts.push(object(
                Builtin::RegisteredInterpolation {
                    expression,
                    conversion: interpolation_conversion(field),
                },
                vec![self.operand(&field.expression)?, self.format_spec(field)?],
            ));
        }
        Some(parts)
    }

    fn format_spec(&self, field: &ast::InterpolatedElement) -> Option<Expr> {
        let Some(spec) = &field.format_spec else {
            return Some(text(""));
        };
        let mut parts = Vec::new();
        for element in &spec.elements {
            let part = match element {
                ast::InterpolatedStringElement::Literal(literal) if literal.value.is_empty() => {
                    continue;
                }
                ast::InterpolatedStringElement::Literal(literal) => {
                    self.source.formatted_literal(literal)
                }
                ast::InterpolatedStringElement::Interpolation(nested) => {
                    if let Some(prefix) = &self.fields.get(&nested.start())?.debug_prefix {
                        parts.push(text(prefix));
                    }
                    let mut value = self.operand(&nested.expression)?;
                    if let Some(c) = interpolation_conversion(nested) {
                        value = object(Builtin::FormatConversion(c), vec![value]);
                    }
                    let value = object(Builtin::FormatField, vec![value, self.format_spec(nested)?]);
                    object(Builtin::RegisteredLocation(nested.range(), nested.start()), vec![value])
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

    fn operand(&self, expr: &ast::Expr) -> Option<Expr> {
        values::registered_value(
            expr,
            RegisteredSource {
                templates: Some(&self.prepared.templates),
                ..self.source
            },
        )
    }
}
