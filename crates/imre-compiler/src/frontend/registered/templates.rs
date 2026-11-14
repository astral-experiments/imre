//! Prepare interpolated string literals and checked template operands before code generation.
use super::*;
use imre_ast::strings::interpolation_conversion;

impl RegisteredSource<'_> {
    pub(crate) fn formatted_literal(self, literal: &ast::InterpolatedStringLiteralElement) -> Expr {
        if let Some(replacements) = self.replacements.get(&literal.range()) {
            let mut bytes = Vec::new();
            values::push_codepoints(&mut bytes, &literal.value, Some(replacements));
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
    let replacements = source.replacements.get(&literal.range()).map(Vec::as_slice);
    values::push_codepoints(buffer, &literal.value, replacements);
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
        let mut strings = Vec::new();
        let mut interpolations = Vec::new();
        let mut segment = Vec::new();
        let string = |points| object(Builtin::RegisteredString(points), vec![]);
        for element in template.value.elements() {
            match element {
                ast::InterpolatedStringElement::Literal(literal) => {
                    literal_points(self, literal, &mut segment);
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    let spelling = fields.get(&field.start())?;
                    if let Some(prefix) = &spelling.debug_prefix {
                        values::push_codepoints(&mut segment, prefix, None);
                    }
                    strings.push(string(std::mem::take(&mut segment)));
                    interpolations.push(object(
                        Builtin::RegisteredInterpolation {
                            expression: spelling.expression.clone(),
                            conversion: interpolation_conversion(field),
                        },
                        vec![
                            operand(&field.expression)?,
                            self.template_spec(field, &mut operand)?,
                        ],
                    ));
                }
            }
        }
        strings.push(string(segment));
        let tuple = |items| Expr {
            ty: Type::PythonObject,
            kind: ExprKind::Tuple(items),
        };
        let value = object(
            Builtin::RegisteredTemplate,
            vec![tuple(strings), tuple(interpolations)],
        );
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
                        Builtin::FormatField(Default::default()),
                        vec![value, self.template_spec(nested, operand)?],
                    );
                    object(
                        Builtin::RegisteredLocation(nested.range(), nested.start(), None),
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
