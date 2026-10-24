//! Typed library helpers borrow already evaluated operands for one runtime call.

use super::*;

impl Emitter<'_, '_> {
    pub(super) fn stdlib_builtin(&mut self, builtin: &Builtin, args: &[Operand]) -> Value {
        match builtin {
            Builtin::SortByKeys => {
                let element = self.module.types.element(args[0].value.ty).unwrap();
                let key = self.module.types.element(args[1].value.ty).unwrap();
                let reverse = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                self.checked_line(format!(
                    "call void @relar_sort_by_keys(ptr {}, i32 {}, ptr {}, i32 {}, i32 {reverse})",
                    args[0].value.operand,
                    representation_kind(element, &self.module.types),
                    args[1].value.operand,
                    representation_kind(key, &self.module.types),
                ));
                none()
            }
            Builtin::SortDetach | Builtin::SortRestore | Builtin::ListExtendUnhinted => {
                let list = &args[0].value;
                let kind = representation_kind(
                    self.module.types.element(list.ty).unwrap(),
                    &self.module.types,
                );
                if matches!(builtin, Builtin::ListExtendUnhinted) {
                    self.line(format!(
                        "call void @relar_list_extend_unhinted(ptr {}, i32 {kind})",
                        list.operand
                    ));
                    none()
                } else if matches!(builtin, Builtin::SortDetach) {
                    Value {
                        ty: list.ty,
                        operand: self.instruction(format!(
                            "call ptr @relar_sort_detach(ptr {}, i32 {kind})",
                            list.operand
                        )),
                        small_integer: false,
                    }
                } else {
                    let completed =
                        self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                    self.checked_line(format!(
                        "call void @relar_sort_restore(ptr {}, i32 {kind}, ptr {}, i32 {completed})",
                        list.operand, args[1].value.operand
                    ));
                    none()
                }
            }
            Builtin::StringFormat => {
                let (template, fields) = args.split_first().unwrap();
                let handles = self.entry_alloca(format!("[{} x ptr]", fields.len()));
                for (index, field) in fields.iter().enumerate() {
                    let slot =
                        self.instruction(format!("getelementptr ptr, ptr {handles}, i64 {index}"));
                    self.line(format!("store ptr {}, ptr {slot}", field.value.operand));
                }
                Value {
                    ty: Type::Str,
                    operand: self.checked_instruction(format!(
                        "call ptr @relar_str_format(ptr {}, ptr {handles}, i64 {})",
                        template.value.operand,
                        fields.len() / 2
                    )),
                    small_integer: false,
                }
            }
            Builtin::StringJoin => self.call(
                "relar_str_join",
                Type::Str,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::StringReplace => self.call(
                "relar_str_replace",
                Type::Str,
                &[&args[0].value, &args[1].value, &args[2].value],
            ),
            Builtin::FloatFormat {
                precision,
                percent,
                positive_sign,
            } => Value {
                ty: Type::Str,
                operand: self.instruction(format!(
                    "call ptr @relar_float_format(double {}, i32 {precision}, i32 {})",
                    args[0].value.operand,
                    u32::from(*percent) | (u32::from(*positive_sign) << 8),
                )),
                small_integer: false,
            },
            Builtin::Abs if args[0].value.ty == Type::Int => {
                self.call("int_abs", Type::Int, &[&args[0].value])
            }
            Builtin::Abs => self.call("llvm.fabs.f64", Type::Float, &[&args[0].value]),
            _ => unreachable!("only typed stdlib operations enter this dispatch"),
        }
    }
}

#[cfg(test)]
mod tests {
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
            "call void @relar_sort_by_keys",
            "call ptr @relar_str_join",
            "call ptr @relar_str_replace",
            "call ptr @relar_float_format",
            "call double @llvm.fabs.f64",
        ] {
            assert!(llvm.contains(operation), "missing {operation}");
        }
    }

    #[test]
    fn sorting_and_formatting_reject_unproved_dynamic_operations() {
        for source in [
            "values = sorted([1.0, 2.0])\nprint(len(values))",
            "values = sorted([1, 2], key=lambda value: float(value))\nprint(len(values))",
            "values = sorted([1, 2], reverse=1)\nprint(len(values))",
            "print(f'{1.0:.1001f}')",
            "precision = 2\nprint(f'{1.0:.{precision}f}')",
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
