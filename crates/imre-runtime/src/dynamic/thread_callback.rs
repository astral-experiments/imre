//! Move executable code and lock ownership into a fresh boxed context.
//! Only the compiler can certify that the function needs no surrounding heap.

use super::thread_lock::Acquire;
use super::thread_worker::Operation;
use super::value::{Body, Code, CodeDefinition, CodeObject, WorkerStack};
use super::{CallParameters, Context, Error, Id, NONE, Result, Value, copy_label, ffi};
use crate::objects::locks::PrimitiveLock;
use crate::objects::logging::HandlerLock;
use std::collections::HashMap;
use std::sync::Arc;

enum Argument {
    Lock(Arc<PrimitiveLock>),
    RLock(Arc<HandlerLock>),
    Builtin(&'static str),
}

pub(super) struct Callback {
    frames: WorkerStack,
    entry: Code,
    definition: CodeDefinition,
    module: String,
    name: String,
    qualname: String,
    filename: Vec<u32>,
    first_line: u32,
    signature: String,
    values: Vec<Argument>,
    receiver: Option<Argument>,
    config: Arc<serde_json::Value>,
    gc_settings: Arc<super::gc_schedule::GcSettings>,
    interpreter: super::InterpreterState,
    allocation_traces: super::allocation_traces::AllocationTraces,
}

pub(super) fn unsupported() -> Error {
    Error::new(
        "NotImplementedError",
        "this thread callback requires the boxed Python heap",
    )
}

pub(super) fn unsupported_target() -> Error {
    Error::new(
        "NotImplementedError",
        "this thread target cannot run independently of the boxed Python heap",
    )
}

impl Callback {
    pub(super) fn capture(
        ctx: &Context,
        target: Id,
        roots: &[Id],
        pos: &[Id],
        kwargs: Option<Id>,
        high_level: bool,
    ) -> Result<Option<(Operation, Vec<Id>)>> {
        let (function, receiver) = match *ctx.heap.get(target) {
            Value::Method {
                callable, receiver, ..
            } => (callable, Some(receiver)),
            _ => (target, None),
        };
        let Value::Function {
            code: code_id,
            ref fields,
            scope,
            defaults,
            kwdefaults,
            ..
        } = *ctx.heap.get(function)
        else {
            return Ok(None);
        };
        let code = ctx.code_object(code_id);
        // Heap-local code operands do not cross the isolated worker interpreter.
        if matches!(code.body, Body::Native(_, operands) if operands != NONE)
            || code.is_coroutine()
            || !code.definition.closure_supported
        {
            return Ok(None);
        }
        let signature = ctx.str_value(code.parameters)?;
        let mut meta: serde_json::Value = serde_json::from_str(signature)
            .map_err(|e| Error::new("SystemError", e.to_string()))?;
        if let Some(receiver) = receiver
            && let Some(serde_json::Value::String(field)) = meta
                .get_mut("thread_bound_lock_field")
                .map(serde_json::Value::take)
        {
            if !matches!(code.body, Body::Native(_, NONE)) {
                return Ok(None);
            }
            if !code.definition.freevars().is_empty()
                || !pos.is_empty()
                || kwargs.is_some_and(
                    |id| !matches!(ctx.heap.get(id), Value::Dict(items) if items.is_empty()),
                )
            {
                return Ok(None);
            }
            // This operation executes only the lock acquire. A receiver check that
            // would raise in the Python frame still requires the worker's exception path.
            if meta.get("types").is_some() {
                let sig = code.call_parameters(|| CallParameters::from_metadata(&mut meta))?;
                let name = ctx.str_value(fields.qualname)?;
                let checked = ctx.check_supplied_types(function, &sig, name, |param| {
                    (sig.positional.first()?.as_str() == param).then_some(receiver)
                });
                if let Err(error) = checked {
                    if error.kind == "TypeError" {
                        return Ok(None);
                    }
                    return Err(error);
                }
            }
            let Some(operand) = ctx.plain_instance_field(receiver, &field) else {
                return Ok(None);
            };
            let Value::PrimitiveLock(lock) = ctx.heap.get(operand) else {
                return Ok(None);
            };
            // The lookup is bound before startup returns. The worker owns the native
            // lock and the parent retains the original method and Python lock wrapper.
            let roots = super::concat_ids(roots, &[operand])?;
            return Ok(Some((
                Operation::Acquire {
                    wait: Acquire::blocking(lock),
                    release: None,
                },
                roots,
            )));
        }
        if receiver.is_some() && !code.definition.freevars().is_empty() {
            return Ok(None);
        }
        let bindings = &ctx.scope_data(scope).closure;
        if let [name] = code.definition.freevars()
            && pos.is_empty()
            && kwargs.is_none_or(
                |id| matches!(ctx.heap.get(id), Value::Dict(entries) if entries.is_empty()),
            )
            && let Some(cell) = bindings.cells.get(name)
            && let Value::Cell(Some(id)) = *ctx.heap.get(*cell)
        {
            let entry = match &code.body {
                Body::Native(entry, NONE) if meta["thread_captured_locks"].as_array().is_some_and(|names|
                    matches!(names.as_slice(), [one] if one.as_str() == Some(name.as_str()))) =>
                    *entry,
                _ => return Ok(None),
            };
            let receiver = match ctx.heap.get(id) {
                Value::PrimitiveLock(lock) => Argument::Lock(Arc::clone(lock)),
                // Detached errors use the original low-level reporting policy.
                Value::HandlerLock(lock) if !high_level => Argument::RLock(Arc::clone(lock)),
                _ => return Ok(None),
            };
            let roots = super::concat_ids(roots, &[id])?;
            let mut callback = Self::from_code(ctx, code, scope, entry, Vec::new())?;
            callback.receiver = Some(receiver);
            return Ok(Some((Operation::Python(Box::new(callback)), roots)));
        }
        if !code.definition.freevars().is_empty() {
            if high_level
                && pos.is_empty()
                && let Some(kwargs) = kwargs
                && matches!(ctx.heap.get(kwargs), Value::Dict(entries) if entries.is_empty())
                && let Some(names) = meta["thread_captured_locks"].as_array()
                && bindings.cells.len() == code.definition.freevars().len()
                && bindings.cells.len() == names.len()
            {
                let operand = |name: &serde_json::Value| {
                    let cell = bindings.cells.get(name.as_str()?)?;
                    let Value::Cell(Some(id)) = *ctx.heap.get(*cell) else {
                        return None;
                    };
                    let Value::PrimitiveLock(lock) = ctx.heap.get(id) else {
                        return None;
                    };
                    Some((id, lock))
                };
                // Source calls without transferable operands still fall through to normal
                // target admission. Keep the existing diagnostic for certified native code.
                let unavailable = || match code.body {
                    Body::Native(..) => Err(unsupported()),
                    _ => Ok(None),
                };
                let [first, last] = names.as_slice() else {
                    return unavailable();
                };
                let (Some((_, release)), Some((id, lock))) = (operand(first), operand(last)) else {
                    return unavailable();
                };
                // No other native owner can race this held release before startup completes.
                if Arc::strong_count(release) != 1
                    || !release.locked()
                    || Arc::ptr_eq(release, lock)
                {
                    return unavailable();
                }
                // The prefix has been consumed when the parent resumes. Only the final
                // receiver must outlive a subsequent replacement of its closure cell.
                let roots = super::concat_ids(roots, &[id])?;
                return Ok(Some((
                    Operation::Acquire {
                        wait: Acquire::blocking(lock),
                        release: Some(Arc::clone(release)),
                    },
                    roots,
                )));
            }
            return Ok(None);
        }
        let Body::Native(entry, NONE) = code.body else {
            return Ok(None);
        };
        if bindings.environment {
            return Ok(None);
        }
        let lock_only = match meta["thread_lock_only"].as_bool() {
            Some(lock_only) => lock_only,
            None if meta["thread_parameters"].is_array() => {
                meta["thread_worker_locks"] == meta["thread_parameters"]
            }
            None => return Ok(None),
        };
        if high_level && !lock_only {
            return Ok(None);
        }
        let parameters = code.call_parameters(|| CallParameters::from_metadata(&mut meta))?;
        let shape = parameters.shape();
        if shape.varargs || shape.kwargs {
            return Ok(None);
        }
        // Earlier producers repeated the positional signature in the admission proof.
        if let Some(names) = meta["thread_parameters"].as_array()
            && (!shape.keyword_only.is_empty()
                || !names
                    .iter()
                    .map(|name| name.as_str())
                    .eq(shape.positional.iter().map(|name| Some(name.as_str()))))
        {
            return Ok(None);
        }
        let refused = if high_level && receiver.is_some() {
            unsupported_target
        } else {
            unsupported
        };
        let kw = kwargs
            .and_then(|id| ctx.dict_entries(id))
            .unwrap_or_default();
        let mut keys = smallvec::SmallVec::<[&str; 8]>::new();
        keys.try_reserve(kw.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        for &(key, _) in kw {
            let Value::Str(key) = ctx.heap.get(key) else {
                return Err(refused());
            };
            keys.push(key);
        }
        let num_defaults = ctx.tuple_elements(defaults).map_or(0, <[_]>::len);
        let binding = shape
            .bind(
                ctx.str_value(code.name)?,
                pos.len() + usize::from(receiver.is_some()),
                keys.iter().copied(),
                num_defaults,
                |name| ctx.dict_named_item(kwdefaults, name).is_some(),
            )
            .map_err(|error| match error {
                crate::source_arguments::Error::Memory => Error::new("MemoryError", ""),
                crate::source_arguments::Error::Type(_) => refused(),
            })?;
        let count = shape.positional.len() + shape.keyword_only.len();
        // Bind directly in the creator's root array. The worker receives only the
        // transferable values selected by these slots.
        let mut retained = Vec::new();
        retained
            .try_reserve_exact(roots.len() + count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        retained.extend_from_slice(roots);
        retained.extend(receiver);
        retained.extend_from_slice(pos);
        retained.resize(roots.len() + count, NONE);
        let bound = &mut retained[roots.len()..];
        for (&(_, value), slot) in kw.iter().zip(binding.keywords) {
            bound[slot.expect("nonvariadic keyword")] = value;
        }
        for slot in binding.defaults {
            bound[slot] = ctx.bound_default(&shape, slot, defaults, kwdefaults);
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for &id in bound.iter() {
            let value = match ctx.heap.get(id) {
                Value::PrimitiveLock(lock) => Argument::Lock(Arc::clone(lock)),
                Value::HandlerLock(lock) => Argument::RLock(Arc::clone(lock)),
                Value::Native(name) if !high_level => {
                    let symbol = [
                        "_signal.pause",
                        "_signal.set_wakeup_fd",
                        "_thread._count",
                        "_thread.exit",
                        "_thread.exit_thread",
                        "sys.exit",
                        "sys.get_coroutine_origin_tracking_depth",
                        "sys.set_coroutine_origin_tracking_depth",
                        "gc.enable",
                        "gc.disable",
                        "gc.isenabled",
                    ]
                    .into_iter()
                    .find(|symbol| *symbol == name)
                    .ok_or_else(refused)?;
                    Argument::Builtin(symbol)
                }
                _ => return Err(refused()),
            };
            values.push(value);
        }
        let callback = Self::from_code(ctx, code, scope, entry, values)?;
        Ok(Some((Operation::Python(Box::new(callback)), retained)))
    }

    fn from_code(
        ctx: &Context,
        code: &CodeObject,
        scope: Id,
        entry: Code,
        values: Vec<Argument>,
    ) -> Result<Self> {
        Ok(Self {
            frames: WorkerStack::reserve()?,
            entry,
            definition: code.definition.clone(),
            module: copy_label(&ctx.scope_data(scope).module)?,
            name: copy_label(ctx.str_value(code.name)?)?,
            qualname: copy_label(ctx.str_value(code.qualname)?)?,
            filename: ctx.string_codepoints(code.filename)?.into_owned(),
            first_line: code.first_line,
            signature: copy_label(ctx.str_value(code.parameters)?)?,
            values,
            receiver: None,
            config: Arc::clone(&ctx.config),
            gc_settings: ctx.heap.gc_settings.clone(),
            interpreter: ctx.interpreter.clone(),
            allocation_traces: ctx.heap.allocation_traces.worker(),
        })
    }

    pub(super) fn load(self) -> Result<(Context, Id, Vec<Id>, Vec<(String, Id)>)> {
        let heap = super::Heap::with_worker_stack(
            Some(self.frames),
            self.gc_settings,
            self.allocation_traces,
        );
        let mut ctx = ffi::new_context_with_heap(
            self.config,
            std::iter::empty::<String>(),
            heap,
            Some(self.interpreter),
        );
        let globals = ctx.scope(&self.module, None);
        // The worker owns this cell and its selected lock before it reports readiness.
        let cell = self.receiver.map(|receiver| {
            let value = match receiver {
                Argument::Lock(lock) => ctx.alloc(Value::PrimitiveLock(lock)),
                Argument::RLock(lock) => ctx.alloc(Value::HandlerLock(lock)),
                Argument::Builtin(_) => unreachable!("the terminal receiver is an exact lock"),
            };
            ctx.alloc(Value::Cell(Some(value)))
        });
        let name = ctx.text(self.name);
        let qualname = ctx.text(self.qualname);
        let signature = ctx.text(self.signature);
        let filename = ctx.text_codepoints(self.filename);
        let location = (filename, self.first_line);
        let code = ctx.realize_code(
            Body::Native(self.entry, NONE),
            self.definition,
            name,
            signature,
            (qualname, NONE),
            location,
        );
        let function = ctx.function_with_closure(
            code,
            globals,
            super::source_code::FunctionClosure::Cells(cell.as_slice()),
            (name, qualname),
            NONE,
            NONE,
        );
        // Preserve object identity when multiple parameters alias the same lock.
        let mut aliases = HashMap::new();
        aliases
            .try_reserve(self.values.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(self.values.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        for value in self.values {
            let id = match value {
                Argument::Lock(lock) => *aliases
                    .entry(Arc::as_ptr(&lock).cast::<()>())
                    .or_insert_with(|| ctx.alloc(Value::PrimitiveLock(lock))),
                Argument::RLock(lock) => *aliases
                    .entry(Arc::as_ptr(&lock).cast::<()>())
                    .or_insert_with(|| ctx.alloc(Value::HandlerLock(lock))),
                Argument::Builtin(name) => ctx.builtin(name),
            };
            slots.push(id);
        }
        let parameters = ctx.parsed_parameters(code)?;
        if parameters.varargs.is_some()
            || parameters.kwargs.is_some()
            || slots.len() != parameters.positional.len() + parameters.keyword_only.len()
        {
            return Err(unsupported());
        }
        // The creator already selected defaults and supplied operands in parameter order.
        let mut keywords = Vec::new();
        keywords
            .try_reserve_exact(parameters.keyword_only.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        for (name, value) in parameters
            .keyword_only
            .iter()
            .zip(slots.drain(parameters.positional.len()..))
        {
            keywords.push((copy_label(name)?, value));
        }
        Ok((ctx, function, slots, keywords))
    }
}
