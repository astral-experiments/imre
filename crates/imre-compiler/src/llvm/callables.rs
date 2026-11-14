//! Exact callback adapters reuse the existing acyclic native-record ownership ABI.

use super::*;

/// The fixed calling convention either names its function or reads an adapter and
/// context from a callable captured before its arguments.
pub(super) enum NativeCallTarget {
    Function(usize),
    Callable(Operand),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Entry {
    Function,
    Adapter { bound: bool, cold: bool },
}

impl Entry {
    fn reference(self, output: &mut Output<'_>, module: &Module, id: usize) -> String {
        match self {
            Self::Function => output.function_ref(module, id),
            Self::Adapter { bound, .. } => output.adapter_ref(id, bound),
        }
    }
}

/// Definitions referenced while lowering callable construction, invocation and inspection.
#[derive(Default)]
pub(super) struct Uses {
    targets: BTreeSet<(usize, Entry)>,
    layouts: BTreeSet<&'static str>,
    context: bool,
    pub(super) inspected: bool,
    pub(super) marked: BTreeSet<usize>,
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
    // Bound adapters consume the receiver word. An unbound adapter ignores it.
    output
        .common()
        .push_str("%callable = type { ptr, ptr, [0 x ptr] }\n");
    if uses.layouts.contains("python_function") || uses.layouts.contains("cache_callable") {
        // A function link owns its referent and can participate in a cycle.
        output.entry_buffer().push_str("@semantics_python_function = private constant { i32, i32, ptr, i64, ptr, i64, ptr } { i32 3, i32 0, ptr null, i64 0, ptr null, i64 0, ptr null }\n");
    }
    for (name, kinds, length) in [
        ("python_function", "i32 0, i32 6", 2),
        ("callable", "i32 0, i32 6", 2),
        ("class_factory", "i32 0, i32 11", 2),
        ("callable_enum", "i32 0, i32 10", 2),
        ("callable_object", "i32 0, i32 5", 2),
        ("callable_list", "i32 0, i32 4", 2),
        ("empty_partial", "i32 0, i32 0, i32 6", 3),
        ("cache_callable", "i32 0, i32 6, i32 6, i32 0, i32 0", 5),
    ] {
        if !uses.layouts.contains(name) {
            continue;
        }
        writeln!(
            output.entry_buffer(),
            "@kinds_{name} = private unnamed_addr constant [{length} x i32] [{kinds}]"
        )
        .unwrap();
        let semantics = if matches!(name, "python_function" | "cache_callable") {
            "@semantics_python_function"
        } else {
            "null"
        };
        writeln!(
            output.global(
                output.entry,
                &format!("layout_{name}"),
                "constant",
                "{ i64, ptr, ptr }"
            ),
            "{{ i64 {length}, ptr @kinds_{name}, ptr {semantics} }}"
        )
        .unwrap();
    }
    for &(id, entry) in &uses.targets {
        let Entry::Adapter { bound, cold } = entry else {
            continue;
        };
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
        output.define_function(
            output.function_owner(id),
            &name,
            returns,
            &signature.join(", "),
            attributes,
            |body| {
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
                writeln!(body, "  ret {returns} %result").unwrap();
            },
        );
        output.common().push('\n');
    }
}

/// A callback retains its two-word calling ABI. Only programs inspecting a
/// callback emit this table of async definitions, partial frames and markers.
fn coroutine_table(
    module: &Module,
    targets: &BTreeSet<(usize, Entry)>,
    marked: &BTreeSet<usize>,
    output: &mut Output,
) {
    use crate::types::CoroutineTarget;
    let previous = output.current;
    output.current = output.entry;
    let entry = "{ ptr, i64, ptr }";
    let mut rows = Vec::new();
    for &(id, callable) in targets {
        let slot = match module.types.coroutine_target(id) {
            Some(CoroutineTarget::Coroutine) => "-1".to_owned(),
            Some(CoroutineTarget::Partial(slot)) => slot.to_string(),
            None if marked.contains(&id) => "-2".to_owned(),
            None => continue,
        };
        let adapter = callable.reference(output, module, id);
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

    fn callable_entry(&mut self, function: usize, bound: bool, ty: Type) -> String {
        let (_, returns) = self.module.types.signature(ty).unwrap();
        let cold = matches!(returns, Type::Coroutine(_));
        let body = &self.module.functions[function];
        // The callable roots its receiver. A borrowed pointer is already the
        // source method's first native parameter and needs no forwarding body.
        let borrowed_receiver = bound
            && !cold
            && body.params.first().is_some_and(|&slot| {
                let receiver = body.locals[slot];
                matches!(receiver.llvm_type(), LlvmType::Ptr)
                    && !self.resource_types.contains(&receiver)
            });
        let entry = if borrowed_receiver {
            Entry::Function
        } else {
            Entry::Adapter { bound, cold }
        };
        self.callable_uses.targets.insert((function, entry));
        entry.reference(self.output, self.module, function)
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
        if let Some((slot, class)) = self.module.types.constructor_slot(function, ty) {
            let slot = self.output.global_ref(format!("module_value_{slot}"));
            let name = self.literal_root(
                self.module.class_metadata[class].qualified_name.as_bytes(),
                Type::Str,
            );
            let layout = self.callable_layout("class_factory");
            let target = self.callable_entry(function, false, ty);
            let operand = self.checked_pointer(format!(
                "call ptr @imre_record_class_value(ptr @{slot}, ptr @{layout}, ptr @{target}, ptr {})",
                name.operand
            ));
            return self.owned(Value {
                ty,
                operand,
                small_integer: false,
            });
        }
        let bound = receiver.is_some();
        let mut receiver = receiver.map(|receiver| self.expr(receiver));
        // The hidden context is one pointer-sized ABI word. A StrEnum stores its
        // nonowning tag in that word; only its adapter converts the bits back.
        let scalar = receiver
            .as_ref()
            .is_some_and(|r| matches!(r.ty, Type::IntEnum(_) | Type::StrEnum(_) | Type::Enum(_)));
        let policy = self.module.types.cache_adapter(function);
        let layout = if policy.is_some() {
            "cache_callable"
        } else if receiver.is_none() && self.linked_functions.contains(&function) {
            "python_function"
        } else if receiver
            .as_ref()
            .is_some_and(|r| r.ty == Type::BuiltinClass)
        {
            "class_factory"
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
        let target = self.callable_entry(function, bound, ty);
        self.line(format!("store ptr @{target}, ptr {}", result.operand));
        let context = self.callable_context_slot(&result);
        if let Some(policy) = policy {
            let source = if let Some(frame) = &receiver {
                self.instruction(format!("load ptr, ptr {}", frame.operand))
            } else {
                // The adapter invokes the declared body directly. Its context still owns
                // the original Python callable if the public link is later replaced.
                let raw = self
                    .module
                    .types
                    .original_cache_body(function)
                    .expect("declared cache body");
                let signature = self.module.types.wrapped_link_type(ty);
                let original = self.callable_value(signature, raw, None);
                let operand = original.operand.clone();
                receiver = Some(original);
                operand
            };
            self.line(format!(
                "call void @imre_callable_init_cache(ptr {}, ptr {source}, i64 {}, i32 {})",
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

    /// Transfer resources only after every operand has been captured in source order.
    pub(super) fn native_call(
        &mut self,
        target: NativeCallTarget,
        args: &CallArgs,
        returns: Type,
    ) -> Value {
        // A field reader can borrow a headerless value-struct snapshot directly.
        if let NativeCallTarget::Function(id) = target
            && let [arg] = args.values.as_slice()
            && self.module.types.is_value_struct(arg.ty)
            && structs::field_reader(&self.module.functions[id])
            && let Some(snapshot) = self.struct_snapshot(arg)
        {
            return self.user_call(
                id,
                returns,
                &[&snapshot],
                args.exhaustive_enum_match,
                args.site.fixed(),
            );
        }
        let mut values = self.call_operands(args);
        self.transfer_call_operands(&mut values);
        let inputs: Vec<_> = args
            .order
            .iter()
            .map(|&index| &values[index].value)
            .collect();
        let result = match (&target, &args.site) {
            (NativeCallTarget::Function(id), _)
            | (NativeCallTarget::Callable(_), ir::CallSite::SavedFunction(_, id)) => self
                .user_call(
                    *id,
                    returns,
                    &inputs,
                    args.exhaustive_enum_match,
                    args.site.fixed(),
                ),
            (NativeCallTarget::Callable(callable), _) => {
                let targets = self.callable_targets.get(callable.value.ty).to_vec();
                let entry = if let [target] = targets.as_slice() {
                    format!(
                        "@{}",
                        self.callable_entry(target.function, target.bound, callable.value.ty)
                    )
                } else {
                    self.instruction(format!("load ptr, ptr {}", callable.value.operand))
                };
                let context = self.callable_context_slot(&callable.value);
                let context = self.instruction(format!("load ptr, ptr {context}"));
                let may_raise = targets.is_empty()
                    || targets
                        .iter()
                        .any(|target| self.effects.may_raise(target.function));
                self.call_with_effect(
                    &entry,
                    Some(&context),
                    returns,
                    &inputs,
                    may_raise,
                    super::CallSite {
                        origin: args.site.fixed(),
                        inline: false,
                    },
                )
            }
        };
        for value in &values {
            self.release_operand(value);
        }
        if let NativeCallTarget::Callable(callable) = target {
            self.release_operand(&callable);
        }
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
        assert!(output.contains("%callable = type { ptr, ptr, [0 x ptr] }"));
        assert!(!output.contains("@layout_callable ="));
    }
}
