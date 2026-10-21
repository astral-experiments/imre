//! Exact callback adapters reuse the existing acyclic native-record ownership ABI.

use std::collections::BTreeSet;

use super::*;

fn adapter_name(function: usize, bound: bool) -> String {
    format!(
        "callable_{function}_{}",
        if bound { "bound" } else { "plain" }
    )
}

/// The callback borrows its record's receiver for the entire invocation. Plain
/// functions use the same hidden-context ABI, ignoring the null context word.
pub(super) fn adapters(module: &Module, sanitize: bool) -> String {
    let mut targets = BTreeSet::new();
    let mut invoked = false;
    for function in module.functions.iter().chain([&module.entry]) {
        crate::ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Callable { function, receiver } = &value.kind {
                targets.insert((*function, receiver.is_some()));
            }
            invoked |= matches!(value.kind, ExprKind::Invoke { .. });
        });
    }
    if targets.is_empty() && !invoked {
        return String::new();
    }
    let mut output = String::from(
        "%callable = type { ptr, ptr }\n\
         @kinds_callable = private unnamed_addr constant [2 x i32] [i32 0, i32 6]\n\
         @layout_callable = private constant { i64, ptr, ptr } { i64 2, ptr @kinds_callable, ptr null }\n",
    );
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
        let mut inputs = Vec::new();
        if bound {
            inputs.push("ptr %context".to_owned());
        }
        inputs.extend(arguments);
        let returns = function.returns.llvm();
        let attributes = if sanitize {
            " alwaysinline sanitize_address"
        } else {
            " alwaysinline"
        };
        writeln!(
            output,
            "define internal {returns} @{}({}){attributes} {{",
            adapter_name(id, bound),
            signature.join(", ")
        )
        .unwrap();
        writeln!(
            output,
            "  %result = call {returns} @fn_{id}({})",
            inputs.join(", ")
        )
        .unwrap();
        writeln!(output, "  ret {returns} %result\n}}\n").unwrap();
    }
    output
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
        let result = self.record(ty);
        let target = adapter_name(function, receiver.is_some());
        self.line(format!("store ptr @{target}, ptr {}", result.operand));
        let context = self.instruction(format!(
            "getelementptr %callable, ptr {}, i32 0, i32 1",
            result.operand
        ));
        let pointer = receiver
            .as_ref()
            .map_or("null", |receiver| receiver.operand.as_str());
        self.line(format!("store ptr {pointer}, ptr {context}"));
        if let Some(receiver) = receiver {
            self.forget(&receiver);
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
            format!("@{}", adapter_name(target.function, target.bound))
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
        assert!(!body.contains("call i8 @relar_error_pending("), "{body}");
        assert!(!body.contains("call void @relar_record_retain("), "{body}");
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
