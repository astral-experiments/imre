//! Typed library helpers borrow already evaluated operands for one runtime call.

use super::*;

impl Emitter<'_, '_> {
    pub(super) fn stdlib_builtin(&mut self, builtin: &Builtin, args: &[Operand]) -> Value {
        match builtin {
            Builtin::SortByComparator => {
                let list = &args[0].value;
                let callable = &args[1].value;
                let element = self.module.types.element(list.ty).unwrap();
                let (_, returns) = self.module.types.signature(callable.ty).unwrap();
                let abi = match element.llvm_type() {
                    LlvmType::I8 => 0,
                    LlvmType::I1 => 1,
                    LlvmType::I64 => 2,
                    LlvmType::Ptr => 3,
                    LlvmType::Double => 7,
                };
                // Resource parameters take an owner, as in transfer_call_operands.
                let abi = abi | (u32::from(self.resource_types.contains(&element)) << 8);
                let target = self.instruction(format!("load ptr, ptr {}", callable.operand));
                let context = self.callable_context_slot(callable);
                let context = self.instruction(format!("load ptr, ptr {context}"));
                let reverse = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                self.checked_line(format!("call void @imre_sort_by_comparator(ptr {}, i32 {}, ptr {target}, ptr {context}, i32 {abi}, i32 {}, i32 {reverse})",
                    list.operand, representation_kind(element, &self.module.types), representation_kind(returns, &self.module.types)));
                none()
            }
            Builtin::SortByKeys(source) => {
                let (values, keys, key, reverse) = match (source, args) {
                    (ir::SortKeys::ObjectText, [values, reverse]) => {
                        assert_eq!(
                            self.module.types.element(values.value.ty),
                            Some(Type::ObjectValue)
                        );
                        (values, values, Type::Str, reverse)
                    }
                    (ir::SortKeys::Stored, [values, keys, reverse]) => {
                        let key = self.module.types.element(keys.value.ty).unwrap();
                        (values, keys, key, reverse)
                    }
                    _ => unreachable!("invalid operands for sort key source"),
                };
                let element = self.module.types.element(values.value.ty).unwrap();
                let reverse = self.instruction(format!("zext i1 {} to i32", reverse.value.operand));
                self.checked_line(format!(
                    "call void @imre_sort_by_keys(ptr {}, i32 {}, ptr {}, i32 {}, i32 {reverse})",
                    values.value.operand,
                    representation_kind(element, &self.module.types),
                    keys.value.operand,
                    representation_kind(key, &self.module.types),
                ));
                none()
            }
            Builtin::SortDetach
            | Builtin::SortRestore
            | Builtin::ListExtendBuffer
            | Builtin::ListExtendSnapshot
            | Builtin::ListExtendUnhinted => {
                let list = &args[0].value;
                let kind = representation_kind(
                    self.module.types.element(list.ty).unwrap(),
                    &self.module.types,
                );
                if matches!(builtin, Builtin::ListExtendBuffer) {
                    Value {
                        ty: list.ty,
                        small_integer: false,
                        operand: self.checked_instruction(format!(
                            "call ptr @imre_list_extend_buffer(i32 {kind})"
                        )),
                    }
                } else if matches!(builtin, Builtin::ListExtendSnapshot) {
                    self.checked_line(format!(
                        "call void @imre_list_extend_snapshot(ptr {}, i32 {kind}, ptr {})",
                        list.operand, args[1].value.operand
                    ));
                    none()
                } else if matches!(builtin, Builtin::ListExtendUnhinted) {
                    self.line(format!(
                        "call void @imre_list_extend_unhinted(ptr {}, i32 {kind})",
                        list.operand
                    ));
                    none()
                } else if matches!(builtin, Builtin::SortDetach) {
                    Value {
                        ty: list.ty,
                        operand: self.checked_instruction(format!(
                            "call ptr @imre_sort_detach(ptr {}, i32 {kind})",
                            list.operand
                        )),
                        small_integer: false,
                    }
                } else {
                    let completed =
                        self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                    self.checked_line(format!(
                        "call void @imre_sort_restore(ptr {}, i32 {kind}, ptr {}, i32 {completed})",
                        list.operand, args[1].value.operand
                    ));
                    none()
                }
            }
            Builtin::FormatField(_) => {
                let value = &args[0].value;
                if args.len() == 1 {
                    if value.ty.is_path() {
                        return self.path_builtin(ir::PathOperation::Text, args, Type::Str);
                    }
                    assert_eq!(
                        value.ty,
                        Type::Str,
                        "fixed default format requires an exact string"
                    );
                    self.retain(value);
                    return value.clone();
                }
                let spec = &args[1].value;
                debug_assert_eq!(spec.ty, Type::Str);
                let format = if value.ty.is_format_scalar() {
                    "imre_format_scalar"
                } else {
                    "imre_format_object"
                };
                let kind = representation_kind(value.ty, &self.module.types);
                let word = self.word(value);
                Value {
                    ty: Type::Str,
                    operand: self.checked_pointer(format!(
                        "call ptr @{format}(i32 {kind}, i64 {word}, ptr {})",
                        spec.operand
                    )),
                    small_integer: false,
                }
            }
            Builtin::StringFormat(layout) => {
                let (template, fields) = args.split_first().unwrap();
                let count = fields.len();
                let tagged = fields.iter().any(|field| field.value.ty == Type::Int);
                let (function, slot_type, width) = if tagged {
                    ("imre_str_format_fields", "i64", 3)
                } else {
                    ("imre_str_format", "ptr", 2)
                };
                let table = self.entry_alloca(format!("[{} x {slot_type}]", count * width));
                for (index, field) in fields.iter().enumerate() {
                    let positional;
                    let name = match layout {
                        ir::RegisteredArguments::Keywords { names, .. } => names[index].as_str(),
                        ir::RegisteredArguments::Packed(packed) if !packed.keywords.is_empty() => {
                            packed.keywords[index]
                                .as_deref()
                                .expect("named format field")
                        }
                        _ => {
                            positional = format!("arg{index}");
                            &positional
                        }
                    };
                    let name = self.literal_root(name.as_bytes(), Type::Str);
                    let value = &field.value;
                    let tagged_field = tagged.then(|| {
                        (
                            self.word(value),
                            representation_kind(value.ty, &self.module.types),
                        )
                    });
                    let mut store = |offset: usize, ty: &str, operand: &str| {
                        let slot = self.instruction(format!(
                            "getelementptr {slot_type}, ptr {table}, i64 {}",
                            index * width + offset
                        ));
                        self.line(format!("store {ty} {operand}, ptr {slot}"));
                    };
                    store(0, "ptr", &name.operand);
                    if let Some((word, kind)) = tagged_field {
                        store(1, "i64", &word);
                        store(2, "i64", &kind.to_string());
                    } else {
                        store(1, "ptr", &value.operand);
                    }
                }
                Value {
                    ty: Type::Str,
                    operand: self.checked_instruction(format!(
                        "call ptr @{function}(ptr {}, ptr {table}, i64 {count})",
                        template.value.operand
                    )),
                    small_integer: false,
                }
            }
            Builtin::StringJoin => self.call(
                "imre_str_join",
                Type::Str,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::StringReplace if args.len() == 4 => self.call(
                "imre_str_replace_count",
                Type::Str,
                &[
                    &args[0].value,
                    &args[1].value,
                    &args[2].value,
                    &args[3].value,
                ],
            ),
            Builtin::StringReplace => self.call(
                "imre_str_replace",
                Type::Str,
                &[&args[0].value, &args[1].value, &args[2].value],
            ),
            Builtin::HexFormat { width, zero, upper } => Value {
                ty: Type::Str,
                operand: self.instruction(format!(
                    "call ptr @imre_int_hex_format(i64 {}, i32 {width}, i32 {})",
                    args[0].value.operand,
                    u32::from(*zero) | (u32::from(*upper) << 1)
                )),
                small_integer: false,
            },
            Builtin::FloatFormat {
                precision,
                percent,
                positive_sign,
            } => Value {
                ty: Type::Str,
                operand: self.checked_pointer(format!(
                    "call ptr @imre_float_format(double {}, i32 {precision}, i32 {})",
                    args[0].value.operand,
                    u32::from(*percent) | (u32::from(*positive_sign) << 8),
                )),
                small_integer: false,
            },
            Builtin::FloatCopySign => self.call(
                "llvm.copysign.f64",
                Type::Float,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::Abs if args[0].value.ty == Type::Int => self.integer_absolute(&args[0].value),
            Builtin::Abs => self.call("llvm.fabs.f64", Type::Float, &[&args[0].value]),
            _ => unreachable!("only typed stdlib operations enter this dispatch"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn checked_math_fabs_uses_the_float_intrinsic() {
        let source = r#"
from math import fabs

def total(x: float, n: int, flag: bool) -> float:
    return fabs(x) + fabs(n) + fabs(flag)

print(total(-0.0, -3, True))
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call double @llvm.fabs.f64(").count(), 3);
        assert!(llvm.contains("call double @imre_int_float("));
        assert!(llvm.contains("uitofp i1"));
    }

    #[test]
    fn checked_math_sqrt_uses_the_checked_numeric_operation() {
        let source = r#"
from math import sqrt

def roots(x: float, n: int, flag: bool) -> float:
    return sqrt(x) + sqrt(n) + sqrt(flag)

print(roots(-0.0, 9, True))
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call double @imre_float_sqrt(").count(), 3);
        assert!(llvm.contains("call double @imre_int_float("));
        assert!(llvm.contains("uitofp i1"));
    }

    #[test]
    fn checked_trigonometric_calls_lower_without_object_operations() {
        let source = r#"from math import sin, cos
def f(x: float, n: int, flag: bool) -> float:
    return sin(x) + cos(n) + sin(flag)
print(f(0.5, 1, True))
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call double @imre_float_sin(").count(), 2);
        assert_eq!(llvm.matches("call double @imre_float_cos(").count(), 1);
        assert!(llvm.contains("call double @imre_int_float("));
        assert!(llvm.contains("uitofp i1"));
    }

    #[test]
    fn float_copysign_uses_the_llvm_intrinsic() {
        let source = "from math import copysign\ndef sign(x: float, y: float) -> float:\n    return copysign(x, y)\nprint(sign(1.0, -0.0))";
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert!(llvm.contains("call double @llvm.copysign.f64"));
    }

    #[test]
    fn eager_sort_string_operations_and_numeric_formats_lower_together() {
        let source = r#"
def key(value: int) -> int:
    return -value
values = sorted([3, 1, 3, 2], key=key, reverse=True)
print(",".join(f"{value:.1f}" for value in values))
print(" α ".replace(" ", ""))
print(f"{abs(-0.125):.2%}")
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        for operation in [
            "call void @imre_sort_by_keys",
            "call ptr @imre_str_join",
            "call ptr @imre_str_replace",
            "call ptr @imre_float_format",
            "call double @llvm.fabs.f64",
        ] {
            assert!(llvm.contains(operation), "missing {operation}");
        }
    }

    #[test]
    fn fixed_format_fields_use_the_source_specification() {
        use crate::ir::{Builtin, ExprKind, Type, walk_expressions};

        let source = concat!(
            "def render(text: str, number: float, items: list[int], spec: str) -> str:\n",
            "    return f'{text}{number:{spec}}{items:{spec}}'\n",
            "print(render('v', 2.5, [1], ''))",
        );
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        let mut fields = Vec::new();
        walk_expressions(&module.functions[0].body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::FormatField(_), args) = &expr.kind {
                fields.push(args.iter().map(|arg| arg.ty).collect::<Vec<_>>());
            }
        });
        assert_eq!(
            fields,
            [
                vec![Type::Str],
                vec![Type::Float, Type::Str],
                vec![Type::Str], // nested {spec} for the float
                vec![Type::ListInt, Type::Str],
                vec![Type::Str], // nested {spec} for the list
            ]
        );
    }

    #[test]
    fn dynamic_and_large_float_precisions_use_native_formatting() {
        for source in [
            "print(f'{1.0:.1001f}')",
            "precision = 2\nprint(f'{1.0:.{precision}f}')",
        ] {
            let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.matches("call ptr @imre_format_scalar(i32 7,").count(),
                1
            );
        }
    }

    #[test]
    fn sorting_and_formatting_reject_unproved_dynamic_operations() {
        for source in [
            "values = sorted([1.0, 2.0])\nprint(len(values))",
            "values = sorted([1, 2], key=lambda value: float(value))\nprint(len(values))",
            "values = sorted([1, 2], reverse=1)\nprint(len(values))",
            "print(','.join([1, 2]))",
            "print('{x}'.format('positional'))",
            "print('{x}'.format(x=[1]))",
            "print('{x}'.format(x='first', x='second'))",
        ] {
            assert!(
                crate::compile(source, &crate::CompileOptions::default()).is_err(),
                "{source}"
            );
        }
    }
}
