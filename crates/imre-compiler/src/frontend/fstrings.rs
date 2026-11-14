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

pub(super) enum FieldValue {
    Formatted(Expr),
    Operand(Expr),
}

struct Fields<C, E> {
    context: C,
    result: Type,
    literal: fn(&mut C, &[FStringLiteral<'_>]) -> StdResult<Expr, E>,
    prepare: fn(&mut C, &ast::InterpolatedElement, ast::ConversionFlag) -> StdResult<FieldValue, E>,
    conversion: fn(&C, Expr, char) -> Expr,
    format: fn(&mut C, &ast::InterpolatedElement, Type, Type) -> Builtin,
}

impl<C, E> Fields<C, E> {
    fn operation(&self, op: Builtin, operands: Vec<Expr>) -> Expr {
        Expr {
            ty: self.result,
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
            // Joining multiple pieces produces an exact str even when a field returned a subclass.
            _ => Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::FormatJoin, parts.values),
            }),
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
        let value = match (self.prepare)(&mut self.context, field, conversion)? {
            FieldValue::Formatted(text) => text,
            FieldValue::Operand(mut value) => {
                if let Some(flag) = conversion.to_char() {
                    value = (self.conversion)(&self.context, value, flag);
                }
                let operands = match &field.format_spec {
                    Some(elements) => {
                        let mut spec = Parts::default();
                        self.elements(&mut spec, &elements.elements, raw)?;
                        vec![value, self.finish(spec)?]
                    }
                    None => vec![value],
                };
                let spec = operands.get(1).map_or(Type::Str, |spec| spec.ty);
                let op = (self.format)(&mut self.context, field, operands[0].ty, spec);
                self.operation(op, operands)
            }
        };
        if self.result == Type::PythonObject {
            return Ok(registered::values::located(field.range(), field.start(), value));
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
        result: Type::PythonObject,
        literal: |(source, _), pieces| Ok(source.string(pieces.iter().copied())),
        prepare: |(_, child), field, _| child(&field.expression).map(FieldValue::Operand),
        conversion: |_, value, flag| object_conversion(value, flag),
        format: |_, _, _, _| Builtin::FormatField(Default::default()),
    }
    .lower(value)
}

pub(super) fn object_conversion(value: Expr, flag: char) -> Expr {
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(Builtin::FormatConversion(flag), vec![value]),
    }
}

impl Checker<'_> {
    fn field_format(
        &mut self,
        field: &ast::InterpolatedElement,
        value: Type,
        spec: Type,
    ) -> Builtin {
        if spec == Type::Str {
            if value.is_format_scalar() {
                return Builtin::FormatField(Default::default());
            }
            if self.inherits_object_format(value)
                && let Some(methods) = self.field_repr_methods(value)
            {
                self.calls.extend(&methods);
                return Builtin::FormatField(ir::FieldFormat {
                    repr_methods: methods,
                    origin: matches!(value, Type::Optional(_))
                        .then(|| Box::new(self.fixed_origin(field.range(), None))),
                });
            }
        }
        Builtin::ObjectCall(ir::ObjectCall::FormatField, self.module, field.range())
    }

    fn inherits_object_format(&self, ty: Type) -> bool {
        // None also inherits object.__format__, so a nullable operand is eligible
        // when its present value inherits the same method.
        let ty = self.types.payload(ty).unwrap_or(ty);
        matches!(ty, Type::None | Type::ListInt | Type::List(_)) || self.types.is_plain_tuple(ty)
    }

    /// The native str service calls repr on members. Each possible override must
    /// have its actual checked body in the concrete record's display descriptor.
    fn field_repr_methods(&self, root: Type) -> Option<Box<[usize]>> {
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        let mut methods = Vec::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            if let Type::Class(id) = ty {
                for layout in self.classes.concrete_layouts(id) {
                    if self.types.is_value_struct(Type::Class(layout)) {
                        return None;
                    }
                    let (owner, definition) = self.classes.repr_override(layout)?;
                    let method = self.classes.methods[layout].get("__repr__")?;
                    let context = method.method?;
                    let class = &self.classes.declarations.definitions[context.class];
                    if method.asynchronous
                        || context.kind(self.classes.declarations)
                            != crate::classes::MethodKind::Instance
                        || class.method_origins[context.index] != owner
                        || class.methods[context.index].range() != definition.range()
                    {
                        return None;
                    }
                    methods.push(method.id);
                }
            } else if self.inherits_object_format(ty) || matches!(ty, Type::Optional(_)) {
                pending.extend(self.types.children(ty));
            } else if !ty.is_format_scalar() && !matches!(ty, Type::StrLiteral(_)) {
                return None;
            }
        }
        methods.sort_unstable();
        methods.dedup();
        Some(methods.into_boxed_slice())
    }

    fn field_conversion(&self, value: Expr, flag: char) -> Expr {
        let op = match flag {
            's' if value.ty == Type::Str => return value,
            's' if value.ty.is_format_scalar()
                || matches!(value.ty, Type::StrLiteral(_) | Type::None) =>
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
            result: Type::Str,
            context: self,
            literal: |checker, pieces| {
                let mut text = String::new();
                for &piece in pieces {
                    match piece {
                        FStringLiteral::Adjacent(lit) => check_string_literal(lit, checker.source)?,
                        FStringLiteral::Middle(lit, false) => {
                            check_string_escapes(&lit.value, lit.range(), checker.source)?
                        }
                        _ => {}
                    }
                    text.push_str(piece.text());
                }
                let value = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(text),
                };
                if let [FStringLiteral::Middle(literal, _)] = pieces
                    && let Some(sites) = &mut checker.fixed_sites
                {
                    sites.leaf(literal.range(), &value);
                }
                Ok(value)
            },
            prepare: |checker, field, conversion| {
                checker.with_fixed_site(field.range(), |checker| {
                    checker.prepare_field(field, conversion)
                })
            },
            conversion: |checker, value, flag| checker.field_conversion(value, flag),
            format: |checker, field, value, spec| checker.field_format(field, value, spec),
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
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            replacements: &module.registered_values().replacements,
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
        let mut fixed = false;
        let mut field = false;
        for function in module.functions.iter().chain([&module.entry]) {
            ir::walk_expressions(&function.body, &mut |expr| match expr.kind {
                ExprKind::Builtin(Builtin::ObjectCall(..), _) => panic!("unexpected object call"),
                ExprKind::Builtin(Builtin::FloatFormat { .. }, _) => fixed = true,
                ExprKind::Builtin(Builtin::FormatField(_), _) => field = true,
                _ => {}
            });
        }
        assert!(fixed && field);
        let options = crate::CompileOptions::default();
        crate::check(source).unwrap();
        crate::compile(source, &options).unwrap();

        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.py");
        for (ty, argument, operation) in [
            ("bytes", "b'ok'", "call ptr @imre_format_scalar(i32 8,"),
            ("list[int]", "[7]", "call ptr @imre_format_object("),
            (
                "list[int] | None",
                "None",
                "call ptr @imre_format_object(i32 260,",
            ),
            (
                "list[int] | None",
                "[17, 19]",
                "call ptr @imre_format_object(i32 260,",
            ),
            (
                "tuple[int, str]",
                "(7, 'word')",
                "call ptr @imre_format_object(",
            ),
        ] {
            let source = format!(
                "def render(data: {ty}, spec: str) -> str:\n    return f'{{data:{{spec}}}}'\n\nprint(render({argument}, input()))\n"
            );
            std::fs::write(&app, &source).unwrap();
            crate::check(&source).unwrap();
            crate::check_file(&app, None).unwrap();
            for llvm in [
                crate::compile(&source, &options).unwrap(),
                crate::compile_file(&app, None, &options).unwrap().llvm_ir,
            ] {
                assert_eq!(llvm.matches(operation).count(), 1);
                let field_frame = llvm.lines().any(|line| {
                    line.contains("call ptr @imre_exception_at_frame(ptr null,")
                        && line.ends_with("i32 2, i32 13, i32 13, i32 26)")
                });
                assert_eq!(field_frame, ty == "list[int] | None");
                assert!(
                    !llvm
                        .lines()
                        .any(|line| line.contains("call ") && line.contains("@imre_dyn_"))
                );
            }
        }

        let unsupported = "def render(data: int | None, spec: str) -> str:
    return f'{data:{spec}}'

print(render(None, input()))
";
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

    #[test]
    fn field_member_repr_checks_initializer_reads() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.py");
        std::fs::write(&app, "from provider import printed\nprint(printed)\n").unwrap();
        std::fs::write(
            directory.path().join("provider.py"),
            "class Member:
    def __repr__(self) -> str:
        return label

def show(value: Member, spec: str) -> str:
    return f'{[value]:{spec}}'

printed: str = show(Member(), '')
label: str = input()
",
        )
        .unwrap();
        let error = crate::check_file(&app, None).unwrap_err();
        assert!(
            error.ends_with("provider.py:8:1: Imre: module initialization may read a value before it is initialized"),
            "{error}"
        );
    }
}
