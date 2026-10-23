//! Exact callback adapters reuse the existing acyclic native-record ownership ABI.

use std::collections::BTreeSet;

use super::*;
use crate::callables::Target;

const MAX_LOOP_NODES: usize = 192;
const MAX_TARGET_NODES: usize = 8;

fn adapter_name(function: usize, bound: bool) -> String {
    format!(
        "callable_{function}_{}",
        if bound { "bound" } else { "plain" }
    )
}

/// Bound inlining growth to two short, straight-line return expressions.
/// Aggregate construction, nested calls, and control flow retain generic dispatch.
fn small_pair(module: &Module, targets: &[Target]) -> bool {
    let mut nodes = 0;
    for target in targets {
        let body = &module.functions[target.function].body;
        if !matches!(body.as_slice(), [Statement::Return(_)]) {
            return false;
        }
        let mut simple = true;
        crate::ir::walk_expressions(body, &mut |value| {
            nodes += 1;
            simple &= matches!(
                value.kind,
                ExprKind::Local(_)
                    | ExprKind::Int(_)
                    | ExprKind::Float(_)
                    | ExprKind::Bool(_)
                    | ExprKind::Field(..)
                    | ExprKind::Negate(_)
                    | ExprKind::Binary(Binary::Add | Binary::Subtract | Binary::Multiply, ..)
            );
        });
        if !simple || nodes > MAX_TARGET_NODES {
            return false;
        }
    }
    true
}

/// The callback borrows its record's receiver for the entire invocation. Plain
/// functions use the same hidden-context ABI, ignoring the null context word.
pub(super) fn adapters(module: &Module, sanitize: bool) -> String {
    let mut targets = BTreeSet::new();
    let mut invoked = false;
    let mut enum_receiver = false;
    for function in module.functions.iter().chain([&module.entry]) {
        crate::ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Callable { function, receiver } = &value.kind {
                targets.insert((*function, receiver.is_some()));
                enum_receiver |= receiver
                    .as_ref()
                    .is_some_and(|r| matches!(r.ty, Type::StrEnum(_)));
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
    if enum_receiver {
        output.push_str("@kinds_callable_enum = private unnamed_addr constant [2 x i32] [i32 0, i32 10]\n\
             @layout_callable_enum = private constant { i64, ptr, ptr } { i64 2, ptr @kinds_callable_enum, ptr null }\n");
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
        let scalar = bound && matches!(function.locals[function.params[0]], Type::StrEnum(_));
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
        writeln!(
            output,
            "define internal {returns} @{}({}){attributes} {{",
            adapter_name(id, bound),
            signature.join(", ")
        )
        .unwrap();
        if scalar {
            output.push_str("  %tag = ptrtoint ptr %context to i64\n");
        }
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
    /// Version one bounded loop around an invocation of an unchanged, initialized
    /// local. Both arms still read the original receiver and use ordinary cleanup.
    pub(super) fn version_callback_loop(
        &mut self,
        statement: &Statement,
        prefix: &[Statement],
    ) -> bool {
        if !self.can_version_callback {
            return false;
        }
        let (Statement::For { body, .. } | Statement::While(_, body, _)) = statement else {
            return false;
        };
        let mut candidate = None;
        crate::ir::walk_expressions(body, &mut |value| {
            if candidate.is_none()
                && let ExprKind::Invoke { callable, .. } = &value.kind
                && let ExprKind::Local(slot) = callable.kind
                && let [first, second] = self.callable_targets.get(callable.ty)
                && self.function_units.is_none_or(|units| {
                    units[first.function] == self.current_unit
                        && units[second.function] == self.current_unit
                })
                && small_pair(self.module, &[*first, *second])
            {
                candidate = Some((slot, std::ptr::from_ref(callable.as_ref()), *first, *second));
            }
        });
        let Some((slot, site, first, second)) = candidate else {
            return false;
        };
        let whole_loop = std::slice::from_ref(statement);
        let mut nodes = 0;
        crate::borrows::statements(whole_loop, &mut |_| nodes += 1);
        crate::ir::walk_expressions(whole_loop, &mut |_| nodes += 1);
        if nodes > MAX_LOOP_NODES
            || crate::borrows::written_in(whole_loop, self.locals.len(), |_| false)[slot]
        {
            return false;
        }
        // Lazy pipelines capture their callback in a private local before the
        // loop. A dominating capture of a stable parameter also proves non-null
        // on empty inputs, so the guard may read the target before iteration.
        if !self.borrowed_params[slot] {
            let Some(capture) = prefix.iter().rposition(|stmt| {
                matches!(stmt,
                Statement::Assign(dest, Expr { kind: ExprKind::Local(src), .. })
                    if *dest == slot && self.borrowed_params[*src] && self.locals[*src] == self.locals[slot])
            }) else {
                return false;
            };
            if crate::borrows::written_in(&prefix[capture + 1..], self.locals.len(), |_| false)
                [slot]
            {
                return false;
            }
        }
        self.can_version_callback = false;
        let callable = self.instruction(format!("load ptr, ptr %local{slot}"));
        let target = self.instruction(format!("load ptr, ptr {callable}"));
        let adapter = adapter_name(first.function, first.bound);
        let is_first = self.instruction(format!("icmp eq ptr {target}, @{adapter}"));
        let (yes, no, end) = (self.label(), self.label(), self.label());
        self.branch(&is_first, &yes, &no);
        for (label, target) in [(&yes, first), (&no, second)] {
            self.start(label);
            self.callback_version = Some((site, target));
            self.statement(statement);
            if !self.terminated {
                self.jump(&end);
            }
        }
        self.callback_version = None;
        self.start(&end);
        true
    }

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
            .is_some_and(|r| matches!(r.ty, Type::StrEnum(_)));
        let result = self.record_with_layout(ty, if scalar { "callable_enum" } else { "callable" });
        let target = adapter_name(function, receiver.is_some());
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
        let direct = match (targets.as_slice(), self.callback_version) {
            ([target], _) => Some(*target),
            (_, Some((site, target))) if std::ptr::eq(site, callable) => Some(target),
            _ => None,
        };
        let callable = self.operand(callable);
        let values = self.call_operands(args);
        let target = if let Some(target) = direct {
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
    fn specializes_a_small_loop_with_two_cheap_stable_targets() {
        let call = "acc += fn(x)";
        // Independent effects exceed the loop budget without building a long
        // chain of inferred additions on the same accumulator.
        let large_loop = format!("{call}{}", "\n        print(x)".repeat(64));
        for (expression, body, specialized) in [
            ("x + 1", call, true),
            ("x * x + x * x + x * x", call, false),
            ("x + 1", "acc += fn(x)\n        fn = identity", false),
            ("x + 1", large_loop.as_str(), false),
            ("x + 1", "acc += x\n    else:\n        acc += fn(1)", false),
        ] {
            let source = format!(
                r#"
from typing import Callable
def identity(x: int) -> int:
    return x
def other(x: int) -> int:
    return {expression}
def total(fn: Callable[[int], int], xs: list[int]) -> int:
    acc = fn(0)
    for x in xs:
        {body}
    return acc
print(total(identity, [1]))
print(total(other, [2]))
"#
            );
            let ir = compile(&source, &CompileOptions::default()).unwrap();
            let body = ir.split_once("define internal i64 @fn_2(").unwrap().1;
            let body = body.split_once("\n}").unwrap().0;
            assert_eq!(body.contains("call i64 @callable_0_plain("), specialized);
            assert_eq!(body.contains("call i64 @callable_1_plain("), specialized);
            assert_eq!(
                body.matches("call i64 %").count(),
                if specialized { 1 } else { 2 }
            );
        }
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
