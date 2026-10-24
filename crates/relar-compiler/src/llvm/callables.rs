//! Exact callback adapters reuse the existing acyclic native-record ownership ABI.

use std::collections::BTreeSet;

use super::*;

/// The callback borrows its record's receiver for the entire invocation. Plain
/// functions use the same hidden-context ABI, ignoring the null context word.
pub(super) fn adapters(module: &Module, sanitize: bool, output: &mut Output) {
    let mut targets = BTreeSet::new();
    let mut invoked = false;
    let mut enum_receiver = false;
    for function in module.functions.iter().chain([&module.entry]) {
        crate::ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Callable { function, receiver } = &value.kind {
                targets.insert((*function, receiver.is_some()));
                enum_receiver |= receiver
                    .as_ref()
                    .is_some_and(|r| matches!(r.ty, Type::StrEnum(_) | Type::Enum(_)));
            }
            invoked |= matches!(value.kind, ExprKind::Invoke { .. });
        });
    }
    if targets.is_empty() && !invoked {
        return;
    }
    output.common().push_str("%callable = type { ptr, ptr }\n");
    for (name, kind) in [("callable", 6)]
        .into_iter()
        .chain(enum_receiver.then_some(("callable_enum", 10)))
    {
        writeln!(
            output.entry_buffer(),
            "@kinds_{name} = private unnamed_addr constant [2 x i32] [i32 0, i32 {kind}]"
        )
        .unwrap();
        writeln!(
            output.global(
                output.entry,
                &format!("layout_{name}"),
                "constant",
                "{ i64, ptr, ptr }"
            ),
            "{{ i64 2, ptr @kinds_{name}, ptr null }}"
        )
        .unwrap();
    }
    for (id, bound) in targets {
        let function = &module.functions[id];
        let params = &function.params[usize::from(bound)..];
        let arguments = params
            .iter()
            .enumerate()
            .map(|(index, &slot)| format!("{} %arg{index}", function.locals[slot].llvm()))
            .collect::<Vec<_>>();
        let mut signature = vec!["ptr %context".to_owned()];
        signature.extend(arguments.iter().cloned());
        let scalar = bound
            && matches!(
                function.locals[function.params[0]],
                Type::StrEnum(_) | Type::Enum(_)
            );
        let mut inputs = Vec::new();
        if bound {
            inputs.push(if scalar { "i64 %tag" } else { "ptr %context" }.to_owned());
        }
        inputs.extend(arguments);
        let returns = function.returns.llvm();
        let attributes = if sanitize {
            " alwaysinline sanitize_address"
        } else {
            " alwaysinline"
        };
        let target = output.function_name(id);
        let name = output.adapter_name(id, bound);
        let body = output.function(
            output.function_owner(id),
            &name,
            returns,
            &signature.join(", "),
            attributes,
        );
        if scalar {
            body.push_str("  %tag = ptrtoint ptr %context to i64\n");
        }
        writeln!(
            body,
            "  %result = call {returns} @{target}({})",
            inputs.join(", ")
        )
        .unwrap();
        writeln!(body, "  ret {returns} %result\n}}").unwrap();
        output.common().push('\n');
    }
}

impl Emitter<'_, '_> {
    /// Captures the receiver once and transfers its owner into the immutable
    /// callable record. Its destructor uses the ordinary record descriptor.
    pub(super) fn callable_value(
        &mut self,
        ty: Type,
        function: usize,
        receiver: Option<&Expr>,
    ) -> Value {
        let receiver = receiver.map(|receiver| self.expr(receiver));
        // The hidden context is one pointer-sized ABI word. A StrEnum stores its
        // nonowning tag in that word; only its adapter converts the bits back.
        let scalar = receiver
            .as_ref()
            .is_some_and(|r| matches!(r.ty, Type::StrEnum(_) | Type::Enum(_)));
        let result = self.record_with_layout(ty, if scalar { "callable_enum" } else { "callable" });
        let target = self.output.adapter_ref(function, receiver.is_some());
        self.line(format!("store ptr @{target}, ptr {}", result.operand));
        let context = self.instruction(format!(
            "getelementptr %callable, ptr {}, i32 0, i32 1",
            result.operand
        ));
        if let Some(receiver) = receiver {
            self.line(format!(
                "store {} {}, ptr {context}",
                receiver.ty.llvm(),
                receiver.operand
            ));
            self.forget(&receiver);
        } else {
            self.line(format!("store ptr null, ptr {context}"));
        }
        result
    }

    /// The callee is captured before arguments. Stable local owners permit a
    /// borrow; temporaries and expression-rebound slots keep independent owners.
    /// A singleton closed target exposes its body to LLVM without dynamic dispatch.
    pub(super) fn invoke_callable(
        &mut self,
        callable: &Expr,
        args: &CallArgs,
        returns: Type,
    ) -> Value {
        let targets = self.callable_targets.get(callable.ty).to_vec();
        let callable = self.operand(callable);
        let values = self.call_operands(args);
        let target = if let [target] = targets.as_slice() {
            format!(
                "@{}",
                self.output.adapter_ref(target.function, target.bound)
            )
        } else {
            self.instruction(format!("load ptr, ptr {}", callable.value.operand))
        };
        let context = self.instruction(format!(
            "getelementptr %callable, ptr {}, i32 0, i32 1",
            callable.value.operand
        ));
        let context = self.instruction(format!("load ptr, ptr {context}"));
        let mut inputs = vec![format!("ptr {context}")];
        inputs.extend(args.order.iter().map(|&index| {
            let value = &values[index].value;
            format!("{} {}", value.ty.llvm(), value.operand)
        }));
        let instruction = format!("call {} {target}({})", returns.llvm(), inputs.join(", "));
        let may_raise = targets.is_empty()
            || targets
                .iter()
                .any(|target| self.effects.may_raise(target.function));
        let operand = if may_raise {
            self.checked_instruction(instruction)
        } else {
            self.instruction(instruction)
        };
        let result = self.owned(Value {
            ty: returns,
            operand,
            small_integer: false,
        });
        for value in &values {
            self.release_operand(value);
        }
        self.release_operand(&callable);
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn known_infallible_target_avoids_dispatch_and_redundant_callable_owners() {
        let output = compile(
            r#"
from typing import Callable
def increment(value: int) -> int:
    return value + 1
def apply(callback: Callable[[int], int], value: int) -> int:
    return callback(value)
print(apply(increment, 4))
"#,
            &CompileOptions::default(),
        )
        .unwrap();
        let body = output
            .split_once("@fn_1(")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        assert!(body.contains("call i64 @callable_0_plain("), "{body}");
        assert!(!body.contains("call i32 @relar_error_pending("), "{body}");
        assert!(!body.contains("call void @relar_record_retain("), "{body}");
    }

    #[test]
    fn immediately_consumed_map_and_filter_call_known_functions_directly() {
        let output = compile(
            "def add(x: int) -> int:\n    return x + 1\ndef keep(x: int) -> int:\n    return x - 1\ndef transform(xs: list[int]) -> list[int]:\n    return list(map(add, filter(keep, xs)))\n",
            &CompileOptions::default(),
        )
        .unwrap();
        let body = output
            .split_once("define internal ptr @fn_2(")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        assert!(
            body.contains("call i64 @fn_0(") && body.contains("call i64 @fn_1("),
            "{body}"
        );
        assert!(!body.contains("@relar_record_new"), "{body}");
    }

    #[test]
    fn unused_higher_order_functions_still_define_the_callable_layout() {
        let output = compile(
            "from typing import Callable\ndef apply(callback: Callable[[int], int], value: int) -> int:\n    return callback(value)\n",
            &CompileOptions::default(),
        ).unwrap();
        assert!(output.contains("%callable = type { ptr, ptr }"));
    }
}
