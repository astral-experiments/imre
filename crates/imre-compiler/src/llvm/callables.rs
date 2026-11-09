//! Exact callback adapters reuse the existing acyclic native-record ownership ABI.

use super::*;

/// Definitions referenced while lowering callable construction, invocation and inspection.
#[derive(Default)]
pub(super) struct Uses {
    /// Function index, bound receiver, and cold coroutine calling convention.
    targets: BTreeSet<(usize, bool, bool)>,
    layouts: BTreeSet<&'static str>,
    context: bool,
    pub(super) inspected: bool,
    pub(super) marked: BTreeSet<usize>,
}

impl Uses {
    pub(super) fn is_empty(&self) -> bool {
        self.targets.is_empty()
            && self.layouts.is_empty()
            && self.marked.is_empty()
            && !self.context
            && !self.inspected
    }
}

/// Bound adapters pass the record's receiver according to the target's native
/// convention. Cold adapters take the callable owner from the invocation frame.
pub(super) fn adapters(emitter: &mut Emitter<'_, '_>) {
    let uses = &emitter.callable_uses;
    let output = &mut *emitter.output;
    let module = emitter.module;
    for id in &uses.marked {
        output
            .global(
                output.entry,
                &format!("coroutine_mark_{id}"),
                "global",
                "i8",
            )
            .push_str("0\n");
    }
    if uses.inspected {
        coroutine_table(module, &uses.targets, &uses.marked, output);
    }
    if uses.targets.is_empty() && uses.layouts.is_empty() && !uses.context {
        return;
    }
    output.common().push_str("%callable = type { ptr, ptr }\n");
    if uses.layouts.contains("python_function") {
        // Keep the ordinary adapter/context ABI. The optional third field owns
        // __wrapped__; kind 3 identifies this link when reclaiming a cycle.
        output.entry_buffer().push_str("@semantics_python_function = private constant { i32, i32, ptr, i64, ptr, i64, ptr } { i32 3, i32 0, ptr null, i64 0, ptr null, i64 0, ptr null }\n@kinds_python_function = private constant [3 x i32] [i32 0, i32 6, i32 6]\n");
        output
            .global(
                output.entry,
                "layout_python_function",
                "constant",
                "{ i64, ptr, ptr }",
            )
            .push_str("{ i64 3, ptr @kinds_python_function, ptr @semantics_python_function }\n");
    }
    for (name, kinds, length) in [
        ("callable", "i32 0, i32 6", 2),
        ("callable_enum", "i32 0, i32 10", 2),
        ("callable_object", "i32 0, i32 5", 2),
        ("callable_list", "i32 0, i32 4", 2),
        ("empty_partial", "i32 0, i32 0, i32 6", 3),
        ("cache_callable", "i32 0, i32 6, i32 0, i32 0", 4),
    ] {
        if !uses.layouts.contains(name) {
            continue;
        }
        writeln!(
            output.entry_buffer(),
            "@kinds_{name} = private unnamed_addr constant [{length} x i32] [{kinds}]"
        )
        .unwrap();
        writeln!(
            output.global(
                output.entry,
                &format!("layout_{name}"),
                "constant",
                "{ i64, ptr, ptr }"
            ),
            "{{ i64 {length}, ptr @kinds_{name}, ptr null }}"
        )
        .unwrap();
    }
    for &(id, bound, cold) in &uses.targets {
        let function = &module.functions[id];
        let params = &function.params[usize::from(bound)..];
        let arguments = params
            .iter()
            .enumerate()
            .map(|(index, &slot)| format!("{} %arg{index}", function.locals[slot].llvm()))
            .collect::<Vec<_>>();
        let first_param = if cold {
            "ptr %callable"
        } else {
            "ptr %context"
        };
        let mut signature = vec![first_param.to_owned()];
        signature.extend(arguments.iter().cloned());
        let scalar = bound
            && matches!(
                function.locals[function.params[0]],
                Type::IntEnum(_) | Type::StrEnum(_) | Type::Enum(_)
            );
        let mut inputs = Vec::new();
        if bound {
            inputs.push(if scalar { "i64 %tag" } else { "ptr %context" }.to_owned());
        }
        inputs.extend(arguments);
        let returns = function.returns.llvm();
        let attributes = if emitter.sanitize {
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
        if cold && bound {
            body.push_str("  %slot = getelementptr %callable, ptr %callable, i32 0, i32 1\n  %context = load ptr, ptr %slot\n");
        }
        let owns_receiver = bound
            && emitter
                .resource_types
                .contains(&function.locals[function.params[0]]);
        if scalar {
            body.push_str("  %tag = ptrtoint ptr %context to i64\n");
        } else if owns_receiver {
            let family = if matches!(function.locals[function.params[0]], Type::Deque(_)) {
                "object"
            } else {
                "record"
            };
            writeln!(body, "  call void @imre_{family}_retain(ptr %context)").unwrap();
        }
        if cold && owns_receiver {
            // The callee now owns self. A temporary bound method must not keep
            // the previous self alive after its parameter is rebound.
            body.push_str("  call void @imre_record_release(ptr %callable)\n");
        }
        writeln!(
            body,
            "  %result = call {returns} @{target}({})",
            inputs.join(", ")
        )
        .unwrap();
        if cold && !owns_receiver {
            // A non-resource receiver can stay borrowed until the call returns.
            body.push_str("  call void @imre_record_release(ptr %callable)\n");
        }
        writeln!(body, "  ret {returns} %result\n}}").unwrap();
        output.common().push('\n');
    }
}

/// A callback retains its two-word calling ABI. Only programs inspecting a
/// callback emit this table of async definitions, partial frames and markers.
fn coroutine_table(
    module: &Module,
    targets: &BTreeSet<(usize, bool, bool)>,
    marked: &BTreeSet<usize>,
    output: &mut Output,
) {
    use crate::types::CoroutineTarget;
    let previous = output.current;
    output.current = output.entry;
    let entry = "{ ptr, i64, ptr }";
    let mut rows = Vec::new();
    for &(id, bound, _) in targets {
        let slot = match module.types.coroutine_target(id) {
            Some(CoroutineTarget::Coroutine) => "-1".to_owned(),
            Some(CoroutineTarget::Partial(slot)) => slot.to_string(),
            None if marked.contains(&id) => "-2".to_owned(),
            None => continue,
        };
        let adapter = output.adapter_ref(id, bound);
        let mark = if marked.contains(&id) {
            format!("@coroutine_mark_{id}")
        } else {
            "null".to_owned()
        };
        rows.push(format!(
            "{entry} {{ ptr @{adapter}, i64 {slot}, ptr {mark} }}"
        ));
    }
    rows.push(format!("{entry} zeroinitializer"));
    let table = output.global(
        output.entry,
        "coroutine_targets",
        "constant",
        format_args!("[{} x {entry}]", rows.len()),
    );
    writeln!(table, "[{}]", rows.join(", ")).unwrap();
    output.current = previous;
}

impl Emitter<'_, '_> {
    pub(super) fn callable_layout(&mut self, name: &'static str) -> String {
        self.callable_uses.layouts.insert(name);
        self.output.global_ref(format!("layout_{name}"))
    }

    fn callable_adapter(&mut self, function: usize, bound: bool, ty: Type) -> String {
        let (_, returns) = self.module.types.signature(ty).unwrap();
        self.callable_uses
            .targets
            .insert((function, bound, matches!(returns, Type::Coroutine(_))));
        self.output.adapter_ref(function, bound)
    }

    /// Locate the receiver in the two-word prefix shared by native callable records.
    pub(super) fn callable_context_slot(&mut self, callable: &Value) -> String {
        self.callable_uses.context = true;
        self.instruction(format!(
            "getelementptr %callable, ptr {}, i32 0, i32 1",
            callable.operand
        ))
    }

    /// Borrow the target's adapter and context for the lifetime of its owning third word.
    pub(super) fn empty_partial(&mut self, target: &Value) -> Value {
        let layout = self.callable_layout("empty_partial");
        let wrapper = self.record_with_layout(target.ty, layout);
        let prefix = self.instruction(format!("load %callable, ptr {}", target.operand));
        self.line(format!("store %callable {prefix}, ptr {}", wrapper.operand));
        let owner = self.instruction(format!("getelementptr ptr, ptr {}, i64 2", wrapper.operand));
        self.retain(target);
        self.line(format!("store ptr {}, ptr {owner}", target.operand));
        wrapper
    }

    /// Captures the receiver once and transfers its owner into the immutable
    /// callable record. Its destructor uses the ordinary record descriptor.
    pub(super) fn callable_value(
        &mut self,
        ty: Type,
        function: usize,
        receiver: Option<&Expr>,
    ) -> Value {
        if let Some(slot) = self.module.types.constructor_slot(function, ty) {
            let slot = self.output.global_ref(format!("module_value_{slot}"));
            let layout = self.callable_layout("callable");
            let target = self.callable_adapter(function, false, ty);
            let operand = self.checked_pointer(format!(
                "call ptr @imre_record_class_factory(ptr @{slot}, ptr @{layout}, ptr @{target})"
            ));
            return self.owned(Value {
                ty,
                operand,
                small_integer: false,
            });
        }
        let bound = receiver.is_some();
        let receiver = receiver.map(|receiver| self.expr(receiver));
        // The hidden context is one pointer-sized ABI word. A StrEnum stores its
        // nonowning tag in that word; only its adapter converts the bits back.
        let scalar = receiver
            .as_ref()
            .is_some_and(|r| matches!(r.ty, Type::IntEnum(_) | Type::StrEnum(_) | Type::Enum(_)));
        let policy = self.module.types.cache_adapter(function);
        let layout = if policy.is_some() {
            "cache_callable"
        } else if receiver.is_none() && self.function_link_types.contains(&ty) {
            "python_function"
        } else if scalar {
            "callable_enum"
        } else if receiver
            .as_ref()
            .is_some_and(|r| matches!(r.ty, Type::Deque(_) | Type::List(_) | Type::FrozenStrings))
        {
            "callable_object"
        } else if receiver.as_ref().is_some_and(|r| r.ty == Type::ListInt) {
            "callable_list"
        } else {
            "callable"
        };
        let layout = self.callable_layout(layout);
        let result = self.record_with_layout(ty, layout);
        let target = self.callable_adapter(function, bound, ty);
        self.line(format!("store ptr @{target}, ptr {}", result.operand));
        let context = self.callable_context_slot(&result);
        if let Some(policy) = policy {
            // Applied caches capture the actual wrappee in the first field of
            // their frame. A statically decorated declaration has no frame.
            let source = receiver.as_ref().map_or_else(
                || "null".to_owned(),
                |frame| self.instruction(format!("load ptr, ptr {}", frame.operand)),
            );
            self.line(format!(
                "call void @imre_callable_copy_cache_policy(ptr {}, ptr {source}, i64 {}, i32 {})",
                result.operand,
                policy.capacity,
                u32::from(policy.typed)
            ));
        }
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
        if !bound
            && policy.is_none()
            && self.module.types.needs_function_metadata(ty, function)
            && let Some(site) = self.module.types.function_site(function)
        {
            let module = self.pooled_literal(site.module.as_bytes(), Type::Str);
            let name = self.pooled_literal(site.qualname.as_bytes(), Type::Str);
            let short_name = self.pooled_literal(
                site.qualname.rsplit('.').next().unwrap().as_bytes(),
                Type::Str,
            );
            self.line(format!("call void @imre_typing_declare(ptr {}, i32 {function}, ptr {}, ptr {}, ptr {}, i32 {})",
                result.operand,module.operand,name.operand,short_name.operand,site.line));
            self.release(&short_name);
            self.release(&name);
            self.release(&module);
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
        let (callable, mut values) =
            operations::call_arguments(self, callable, &args.values, Self::operand);
        self.transfer_call_operands(&mut values);
        let target = if let [target] = targets.as_slice() {
            format!(
                "@{}",
                self.callable_adapter(target.function, target.bound, callable.value.ty)
            )
        } else {
            self.instruction(format!("load ptr, ptr {}", callable.value.operand))
        };
        let context = self.callable_context_slot(&callable.value);
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
        assert!(!body.contains("call i32 @imre_error_pending("), "{body}");
        assert!(!body.contains("call void @imre_record_retain("), "{body}");
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
        assert!(!body.contains("@imre_record_new"), "{body}");
    }

    #[test]
    fn unused_higher_order_functions_still_define_the_callable_layout() {
        let output = compile(
            "from typing import Callable\ndef apply(callback: Callable[[int], int], value: int) -> int:\n    return callback(value)\n",
            &CompileOptions::default(),
        ).unwrap();
        assert!(output.contains("%callable = type { ptr, ptr }"));
        assert!(!output.contains("@layout_callable ="));
    }
}
