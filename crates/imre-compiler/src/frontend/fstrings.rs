//! Check f-string fields in Python evaluation order before choosing their representation.
use super::*;

#[derive(Clone, Copy)]
pub(crate) enum FStringLiteral<'a> {
    Adjacent(&'a ast::StringLiteral),
    Middle(&'a ast::InterpolatedStringLiteralElement, bool),
    Text(&'a str),
}

impl<'a> FStringLiteral<'a> {
    pub(super) fn text(self) -> &'a str {
        match self {
            Self::Adjacent(lit) => lit,
            Self::Middle(lit, _) => &lit.value,
            Self::Text(text) => text,
        }
    }
}

#[derive(Default)]
struct Parts<'a> {
    literals: Vec<FStringLiteral<'a>>,
    values: Vec<Expr>,
}

struct Fields<C, E> {
    context: C,
    native_module: Option<ModuleId>,
    literal: fn(&mut C, &[FStringLiteral<'_>]) -> StdResult<Expr, E>,
    operand: fn(&mut C, &ast::Expr) -> StdResult<Expr, E>,
    scalar:
        fn(&mut C, &ast::InterpolatedElement, ast::ConversionFlag) -> StdResult<Option<Expr>, E>,
    conversion: fn(&C, Expr, char) -> Expr,
}

impl<C, E> Fields<C, E> {
    fn operation(&self, op: Builtin, operands: Vec<Expr>) -> Expr {
        Expr {
            ty: if self.native_module.is_some() {
                Type::Str
            } else {
                Type::PythonObject
            },
            kind: ExprKind::Builtin(op, operands),
        }
    }

    fn flush(&mut self, parts: &mut Parts<'_>) -> StdResult<(), E> {
        if parts.literals.iter().any(|part| !part.text().is_empty()) {
            parts
                .values
                .push((self.literal)(&mut self.context, &parts.literals)?);
        }
        parts.literals.clear();
        Ok(())
    }

    fn finish(&mut self, mut parts: Parts<'_>) -> StdResult<Expr, E> {
        self.flush(&mut parts)?;
        match parts.values.len() {
            0 => (self.literal)(&mut self.context, &[]),
            1 => Ok(parts.values.pop().unwrap()),
            _ => Ok(self.operation(Builtin::FormatJoin, parts.values)),
        }
    }

    fn field(&mut self, field: &ast::InterpolatedElement, raw: bool) -> StdResult<Expr, E> {
        let conversion = if field.conversion == ast::ConversionFlag::None
            && field.debug_text.is_some()
            && field.format_spec.is_none()
        {
            ast::ConversionFlag::Repr
        } else {
            field.conversion
        };
        let mut value = if let Some(scalar) = (self.scalar)(&mut self.context, field, conversion)? {
            scalar
        } else {
            let mut value = (self.operand)(&mut self.context, &field.expression)?;
            if let Some(flag) = conversion.to_char() {
                value = (self.conversion)(&self.context, value, flag);
            }
            let mut spec = Parts::default();
            if let Some(elements) = &field.format_spec {
                self.elements(&mut spec, &elements.elements, raw)?;
            }
            let spec = self.finish(spec)?;
            self.operation(Builtin::FormatField, vec![value, spec])
        };
        if let Some(module) = self.native_module {
            if let ExprKind::Builtin(op @ Builtin::FormatField, args) = &mut value.kind
                && (!args[0].ty.is_format_scalar() || args[1].ty != Type::Str)
            {
                *op = Builtin::ObjectCall(ir::ObjectCall::FormatField, module, field.range());
            }
        } else {
            value = self.operation(
                Builtin::RegisteredLocation(field.range(), field.start()),
                vec![value],
            );
        }
        Ok(value)
    }

    fn elements<'a>(
        &mut self,
        parts: &mut Parts<'a>,
        elements: &'a [ast::InterpolatedStringElement],
        raw: bool,
    ) -> StdResult<(), E> {
        for element in elements {
            match element {
                ast::InterpolatedStringElement::Literal(lit) => {
                    parts.literals.push(FStringLiteral::Middle(lit, raw));
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(debug) = &field.debug_text {
                        parts.literals.push(FStringLiteral::Text(debug.as_str()));
                    }
                    self.flush(parts)?;
                    parts.values.push(self.field(field, raw)?);
                }
            }
        }
        Ok(())
    }

    /// Adjacent literals share their constant; a lone field keeps its formatted object.
    fn lower(&mut self, value: &ast::ExprFString) -> StdResult<Expr, E> {
        let mut parts = Parts::default();
        for part in value.value.iter() {
            match part {
                ast::FStringPartRef::Literal(lit) => {
                    parts.literals.push(FStringLiteral::Adjacent(lit))
                }
                ast::FStringPartRef::FString(f) => self.elements(
                    &mut parts,
                    &f.elements,
                    matches!(f.flags.prefix(), ast::str_prefix::FStringPrefix::Raw { .. }),
                )?,
            }
        }
        self.finish(parts)
    }
}

pub(super) fn registered_fstring<E>(
    value: &ast::ExprFString,
    source: RegisteredSource<'_>,
    operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Expr, E> {
    Fields {
        context: (source, operand),
        native_module: None,
        literal: |(source, _), pieces| Ok(source.string(pieces.iter().copied())),
        operand: |(_, child), expr| child(expr),
        scalar: |_, _, _| Ok(None),
        conversion: |_, value, flag| object_conversion(value, flag),
    }
    .lower(value)
}

fn object_conversion(value: Expr, flag: char) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(Builtin::FormatConversion(flag), vec![value]),
    }
}

impl Checker<'_> {
    fn field_conversion(&self, value: Expr, flag: char) -> Expr {
        let op = match flag {
            's' if value.ty == Type::Str => return value,
            's' if value.ty.is_format_scalar()
                || matches!(value.ty, Type::StrLiteral(_) | Type::Bytes | Type::None) =>
            {
                Builtin::Str
            }
            'r' | 'a' if self.has_builtin_repr(value.ty) => Builtin::Repr,
            's' if matches!(
                value.ty,
                Type::ListInt | Type::List(_) | Type::Dict(_) | Type::Tuple(_)
            ) && self.has_builtin_repr(value.ty) =>
            {
                Builtin::Repr
            }
            _ => return object_conversion(value, flag),
        };
        let shown = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(op, vec![value]),
        };
        if flag == 'a' {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::AsciiEscape, vec![shown]),
            }
        } else {
            shown
        }
    }

    pub(super) fn fstring(&mut self, value: &ast::ExprFString) -> Result<Expr, String> {
        Fields {
            native_module: Some(self.module),
            context: self,
            literal: |checker, pieces| {
                let mut text = String::new();
                for &piece in pieces {
                    match piece {
                        FStringLiteral::Adjacent(lit) => check_string_literal(lit, checker.source)?,
                        FStringLiteral::Middle(lit, false) => {
                            check_string_escapes(lit.range(), checker.source)?
                        }
                        _ => {}
                    }
                    text.push_str(piece.text());
                }
                Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(text),
                })
            },
            operand: |checker, expression| checker.expr(expression),
            scalar: |checker, field, conversion| checker.formatted_scalar(field, conversion),
            conversion: |checker, value, flag| checker.field_conversion(value, flag),
        }
        .lower(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_literals_recover_each_original_span() {
        let text = r#""prefix" f"{{\ud800}}�{{\ud801}}" "\udc00""#;
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let ast::Stmt::Expr(statement) = &module.parsed.syntax().body[0] else {
            panic!("expected a formatted expression");
        };
        let ast::Expr::FString(value) = &*statement.value else {
            panic!("expected an f-string");
        };
        let source = RegisteredSource {
            templates: None,
            lambdas: None,
            text,
            replacements: &literal_replacements(text, module.parsed.tokens()),
            has_templates: false,
            method_imports: &Default::default(),
            private_names: None,
        };
        let expression = registered_fstring(value, source, |_| Err(())).unwrap();
        let ExprKind::Builtin(Builtin::RegisteredString(bytes), _) = expression.kind else {
            panic!("expected one constant with restored code points");
        };
        let points: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect();
        assert_eq!(
            points,
            [
                0x70, 0x72, 0x65, 0x66, 0x69, 0x78, 0x7b, 0xd800, 0x7d, 0xfffd, 0x7b, 0xd801, 0x7d,
                0xdc00
            ]
        );
    }
    #[test]
    fn dynamic_spec_keeps_the_following_list_contract() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app.py");
        let source = "def render(number: int, spec: str) -> str:
    return f'{number:{spec}}'

values: list[int] = [3]
print(render(8, input()))
values.append(True)
print(values)
";
        std::fs::write(&app, source).unwrap();
        let error = crate::check_file(&app, None).unwrap_err();
        assert!(
            error.contains("app.py:6:15") && error.contains("expected int, got bool"),
            "{error}"
        );
        std::fs::write(&app, source.replace("append(True)", "append(4)")).unwrap();
        crate::check_file(&app, None).unwrap();
    }

    #[test]
    fn dynamic_scalar_spec_keeps_the_fixed_float_sibling() {
        let source = "def fixed(ratio: float) -> str:
    return f'{ratio:.2f}'

def render(text: str, width: str) -> str:
    return f'{text!a:{width}}'

print(fixed(2.75), render('é', input()))
";
        let graph = SourceGraph::single(source).unwrap();
        let module = super::super::lower(&graph).unwrap();
        assert!(ir::required_object_call(&module).is_none());
        let mut fixed = false;
        let mut field = false;
        for function in &module.functions {
            ir::walk_expressions(&function.body, &mut |expr| match expr.kind {
                ExprKind::Builtin(Builtin::FloatFormat { .. }, _) => fixed = true,
                ExprKind::Builtin(Builtin::FormatField, _) => field = true,
                _ => {}
            });
        }
        assert!(fixed && field);
        let options = crate::CompileOptions::default();
        crate::check(source).unwrap();
        crate::compile(source, &options).unwrap();

        let unsupported = "def render(data: bytes, spec: str) -> str:
    return f'{data:{spec}}'

print(render(b'ok', input()))
";
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.py");
        std::fs::write(&app, unsupported).unwrap();
        let message = "2:14: Imre: this f-string field requires native Python object operations";
        for error in [
            crate::check(unsupported).unwrap_err(),
            crate::compile(unsupported, &options).unwrap_err(),
            crate::check_file(&app, None).unwrap_err(),
            crate::compile_file(&app, None, &options).err().unwrap(),
        ] {
            assert!(error.ends_with(message), "{error}");
        }
    }
}
