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
                let abi = match element.llvm() {
                    "i8" => 0,
                    "i1" => 1,
                    "i64" => 2,
                    "ptr" => 3,
                    "double" => 7,
                    _ => unreachable!("checked sort element"),
                };
                // Resource parameters take an owner, as in transfer_call_operands.
                let abi = abi | (u32::from(self.resource_types.contains(&element)) << 8);
                let target = self.instruction(format!("load ptr, ptr {}", callable.operand));
                let context = self.instruction(format!(
                    "getelementptr %callable, ptr {}, i32 0, i32 1",
                    callable.operand
                ));
                let context = self.instruction(format!("load ptr, ptr {context}"));
                let reverse = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                self.checked_line(format!("call void @imre_sort_by_comparator(ptr {}, i32 {}, ptr {target}, ptr {context}, i32 {abi}, i32 {}, i32 {reverse})",
                    list.operand, representation_kind(element, &self.module.types), representation_kind(returns, &self.module.types)));
                none()
            }
            Builtin::SortByKeys => {
                let element = self.module.types.element(args[0].value.ty).unwrap();
                let key = self.module.types.element(args[1].value.ty).unwrap();
                let reverse = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                self.checked_line(format!(
                    "call void @imre_sort_by_keys(ptr {}, i32 {}, ptr {}, i32 {}, i32 {reverse})",
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
            Builtin::StringFormat => {
                let (template, fields) = args.split_first().unwrap();
                let (pairs, tail) = fields.as_chunks::<2>();
                debug_assert!(tail.is_empty());
                if pairs.iter().any(|pair| pair[1].value.ty == Type::Int) {
                    let count = fields.len() / 2;
                    let storage = self.entry_alloca(format!("[{} x i64]", count * 3));
                    for (index, pair) in pairs.iter().enumerate() {
                        let name = &pair[0].value;
                        let value = &pair[1].value;
                        let word = self.word(value);
                        let kind = representation_kind(value.ty, &self.module.types).to_string();
                        for (offset, (ty, operand)) in [
                            ("ptr", name.operand.as_str()),
                            ("i64", word.as_str()),
                            ("i64", kind.as_str()),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let slot = self.instruction(format!(
                                "getelementptr i64, ptr {storage}, i64 {}",
                                index * 3 + offset
                            ));
                            self.line(format!("store {ty} {operand}, ptr {slot}"));
                        }
                    }
                    return Value {
                        ty: Type::Str,
                        operand: self.checked_instruction(format!(
                            "call ptr @imre_str_format_fields(ptr {}, ptr {storage}, i64 {count})",
                            template.value.operand
                        )),
                        small_integer: false,
                    };
                }
                let handles = self.entry_alloca(format!("[{} x ptr]", fields.len()));
                for (index, field) in fields.iter().enumerate() {
                    let slot =
                        self.instruction(format!("getelementptr ptr, ptr {handles}, i64 {index}"));
                    self.line(format!("store ptr {}, ptr {slot}", field.value.operand));
                }
                Value {
                    ty: Type::Str,
                    operand: self.checked_instruction(format!(
                        "call ptr @imre_str_format(ptr {}, ptr {handles}, i64 {})",
                        template.value.operand,
                        fields.len() / 2
                    )),
                    small_integer: false,
                }
            }
            Builtin::StringJoin => self.call(
                "imre_str_join",
                Type::Str,
                &[&args[0].value, &args[1].value],
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
                operand: self.instruction(format!(
                    "call ptr @imre_float_format(double {}, i32 {precision}, i32 {})",
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
