//! Bind a closed checked function at its original Python code site.

use super::{Function, Output};
use crate::frontend::RegisteredNative;
use crate::ir::{Binary, Type};

impl Output {
    /// The fixed and mixed bodies share the same arithmetic helpers in this LLVM module.
    pub(super) fn native_prelude(&mut self) {
        if self.declarations.retained_native || self.declarations.checked_integer_arithmetic {
            self.functions.push_str(
                "declare i64 @imre_dyn_exact_int_word(ptr, i64)\ndeclare i64 @imre_dyn_take_int(ptr, i64)\n",
            );
        }
        if self.declarations.retained_native {
            self.functions
                .push_str(&crate::llvm::registered_native_prelude());
            self.functions.push_str(concat!(
                "declare i1 @imre_dyn_native_bindings(ptr, i64, ptr, i64)\n",
                "declare ptr @imre_dyn_exact_str_owner(ptr, i64)\n",
                "declare i1 @imre_dyn_finish_native_int(ptr, i64, ptr)\n",
            ));
        } else if self.declarations.checked_integer_arithmetic {
            self.functions.push_str(concat!(
                "declare void @imre_int_retain(i64)\n",
                "declare void @imre_int_release(i64)\n",
                "declare i64 @imre_int_binary(i64, i64, i32)\n",
                "declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64)\n",
                "!0 = !{!\"branch_weights\", i32 1, i32 2000}\n",
                "!1 = !{!\"branch_weights\", i32 2000, i32 1}\n",
            ));
            self.functions
                .push_str(&crate::llvm::integer_arithmetic_helper(Binary::Add));
        }
    }
}

impl Function<'_, '_> {
    pub(super) fn select_native(&mut self, native: &RegisteredNative) {
        let Ok(code) = crate::llvm::registered_native(
            &native.module,
            self.output.count as usize,
            self.source.program_bodies.sanitize,
            &native.frame_lists,
            |offset| self.source.positions.line(offset),
        ) else {
            return;
        };
        self.output.declarations.retained_native = true;
        self.output.functions.push_str(&code.definitions);
        let names = native.builtins.join("\0");
        let bindings = self.output.data(names.as_bytes());
        let ready = self.value(&format!(
            "call i1 @imre_dyn_native_bindings(ptr %ctx, i64 %scope, ptr {bindings}, i64 {})",
            names.len()
        ));
        let fallback = self.name("python_args");
        let bind = self.name("native_args");
        self.branch(&ready, &bind, &fallback);
        self.label(&bind);
        let mut args = vec![
            "ptr %ctx".to_owned(),
            "i64 %scope".to_owned(),
            format!("ptr {}", self.source.positions.file),
        ];
        let mut text_owners = Vec::new();
        let root = &native.module.functions[0];
        // Resolve every bound argument before acquiring any native representation.
        let bound: Vec<_> = root
            .params
            .iter()
            .map(|&slot| (root.locals[slot], self.lookup(&native.local_names[&slot])))
            .collect();
        for (parameter, object) in bound {
            let (ty, projected, zero) = match parameter {
                Type::Str => (
                    "ptr",
                    self.value(&format!(
                        "call ptr @imre_dyn_exact_str_owner(ptr %ctx, i64 {object})"
                    )),
                    "null",
                ),
                Type::Int => (
                    "i64",
                    self.value(&format!(
                        "call i64 @imre_dyn_exact_int_word(ptr %ctx, i64 {object})"
                    )),
                    "0",
                ),
                _ => unreachable!("checked registered argument representation"),
            };
            let exact = self.value(&format!("icmp ne {ty} {projected}, {zero}"));
            let next = self.name("native_arg");
            if text_owners.is_empty() {
                self.branch(&exact, &next, &fallback);
            } else {
                let release = self.name("release_args");
                self.branch(&exact, &next, &release);
                self.label(&release);
                self.release_native_texts(&text_owners);
                self.jump(&fallback);
            }
            self.label(&next);
            args.push(format!("{ty} {projected}"));
            if parameter == Type::Str {
                text_owners.push(projected);
            }
        }
        let raw = self.value(&format!("call i64 @{}({})", code.symbol, args.join(", ")));
        let output = self.value("alloca i64");
        let transferred = self.value(&format!(
            "call i1 @imre_dyn_finish_native_int(ptr %ctx, i64 {raw}, ptr {output})"
        ));
        self.release_native_texts(&text_owners);
        let registered = self.name("native_result");
        // The admitted operations raise plain builtin exceptions. Preserve the native
        // pending owner and unwind immediately if the ABI declines to transfer one.
        let failed = self.body.error_target.clone();
        self.branch(&transferred, &registered, &failed);
        self.label(&registered);
        self.check_error();
        let object = self.value(&format!("load i64, ptr {output}"));
        self.return_value(&object);
        self.label(&fallback);
        self.check_error();
    }

    fn release_native_texts(&mut self, owners: &[String]) {
        for owner in owners {
            self.line(&format!("call void @imre_str_release(ptr {owner})"));
        }
    }
}
