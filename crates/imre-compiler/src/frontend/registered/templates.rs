//! Prepare t-string constants and their interpolation operands before code generation.
use super::*;
use ast::visitor::{self, Visitor};
use imre_ast::strings::{TemplateFieldText, interpolation_conversion};

struct Templates<'a> {
    source: RegisteredSource<'a>,
    fields: &'a HashMap<TextSize, TemplateFieldText>,
    prepared: HashMap<TextRange, Expr>,
}

/// A template with a residual operand stays at its original whole-expression boundary.
pub(crate) fn prepare_templates(
    body: &[ast::Stmt],
    source: RegisteredSource<'_>,
    fields: &HashMap<TextSize, TemplateFieldText>,
) -> HashMap<TextRange, Expr> {
    let mut templates = Templates {
        source,
        fields,
        prepared: HashMap::new(),
    };
    templates.visit_body(body);
    templates.prepared
}

impl<'a> Visitor<'a> for Templates<'_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        // Prepare nested t-strings before lowering their enclosing operands.
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            visitor::walk_expr(self, expr)
        });
        if let ast::Expr::TString(template) = expr
            && let Some(value) = self.template(template)
        {
            self.prepared.insert(expr.range(), value);
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

pub(crate) fn template_segments(
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

impl Templates<'_> {
    fn template(&self, template: &ast::ExprTString) -> Option<Expr> {
        let strings = template_segments(template, self.source, self.fields)?;
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
        Some(object(Builtin::RegisteredTemplate(strings), parts))
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
                    let mut points = Vec::new();
                    literal_points(self.source, literal, &mut points);
                    object(Builtin::RegisteredString(points), vec![])
                }
                ast::InterpolatedStringElement::Interpolation(nested) => {
                    if let Some(prefix) = &self.fields.get(&nested.start())?.debug_prefix {
                        parts.push(text(prefix));
                    }
                    let mut value = self.operand(&nested.expression)?;
                    if let Some(c) = interpolation_conversion(nested) {
                        value = object(Builtin::RegisteredFormatConversion(c), vec![value]);
                    }
                    object(
                        Builtin::RegisteredFormat,
                        vec![value, self.format_spec(nested)?],
                    )
                }
            };
            parts.push(part);
        }
        match parts.len() {
            0 => Some(text("")),
            1 => parts.pop(),
            _ => Some(object(Builtin::RegisteredFormatJoin, parts)),
        }
    }

    fn operand(&self, expr: &ast::Expr) -> Option<Expr> {
        values::registered_value(
            expr,
            RegisteredSource {
                templates: Some(&self.prepared),
                ..self.source
            },
        )
    }
}
