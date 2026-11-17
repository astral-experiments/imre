//! Move executable code and lock ownership into a fresh boxed context.
//! Only the compiler can certify that the function needs no surrounding heap.

use super::thread_lock::Lock;
use super::thread_worker::{Operation, WorkerStatus};
use super::value::{Body, Code, CodeDefinition, CodeObject, ParkedExecution, WorkerStack};
use super::{CallParameters, Context, Error, Id, NONE, Result, Value, copy_label, ffi};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

pub(super) enum Argument {
    Lock(Lock),
    Builtin(&'static str),
    /// Another bound slot already holds this Python object.
    Alias(usize),
}

impl Argument {
    /// Resolve aliases from the earlier formal in this same child frame.
    pub(super) fn realize(
        self,
        ctx: &mut Context,
        local: Id,
        shape: &crate::source_arguments::Shape<'_, String>,
    ) -> Id {
        match self {
            Self::Lock(lock) => ctx.alloc(lock.into_value()),
            Self::Builtin(name) => ctx.builtin(name),
            Self::Alias(first) => ctx.scope_data(local).names[shape.parameter(first)],
        }
    }
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
    captured_lock: Option<Lock>,
    gc_settings: Arc<super::gc_schedule::GcSettings>,
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

/// Admission remains owner-local until canonical call preparation has selected an operation.
pub(super) enum Capture {
    Ready(Operation, Vec<Id>),
    Projected {
        operand: Id,
        selector: String,
        roots: Vec<Id>,
    },
}

/// Prepare an already selected native callable against an exact primitive receiver.
/// Preparation classifies the operation but never runs it or a Python callback.
pub(super) fn prepare_primitive_acquire(
    ctx: &mut Context,
    callable: Id,
    bound_receiver: Id,
    expected: Id,
) -> Result<Option<Operation>> {
    if !matches!(ctx.heap.get(expected), Value::PrimitiveLock(_)) {
        return Ok(None);
    }
    let (callable, receiver) = match *ctx.heap.get(callable) {
        Value::Method {
            callable, receiver, ..
        } if bound_receiver == NONE => (callable, receiver),
        _ => (callable, bound_receiver),
    };
    // A forged bound native callable could perform conversions or reenter Python during
    // preparation. Restrict preparation to the exact receiver's canonical method family.
    if receiver != expected
        || !matches!(ctx.heap.get(callable), Value::Native(symbol)
            if symbol.strip_prefix(super::thread_lock::CLASS)
                .and_then(|suffix| suffix.strip_prefix('.'))
                .is_some_and(super::thread_lock::is_method))
    {
        return Ok(None);
    }
    let args = [receiver];
    let super::calls::RegisteredCall::Acquire(acquire) =
        ctx.prepare_call(callable, std::borrow::Cow::Borrowed(&args), Vec::new())?
    else {
        return Ok(None);
    };
    Ok(acquire
        .native_primitive(ctx, expected)
        .map(|wait| Operation::Acquire {
            wait,
            release: None,
        }))
}

impl Capture {
    /// Resolve only the projected case after releasing the immutable capture borrows.
    pub(super) fn resolve(self, ctx: &mut Context) -> Result<Option<(Operation, Vec<Id>)>> {
        let (operand, selector, roots) = match self {
            Self::Ready(operation, roots) => return Ok(Some((operation, roots))),
            Self::Projected {
                operand,
                selector,
                roots,
            } => (operand, selector, roots),
        };
        let mark = ctx.heap.temporary_mark();
        let result = (|| {
            let (callable, receiver) = ctx.builtin_method_target(operand, &selector)?;
            prepare_primitive_acquire(ctx, callable, receiver, operand)
                .map(|operation| operation.map(|operation| (operation, roots)))
        })();
        ctx.heap.release_temporaries_since(mark);
        match result {
            Err(error) if error.kind != "MemoryError" => Ok(None),
            value => value,
        }
    }
}

impl Callback {
    pub(super) fn capture(
        ctx: &Context,
        target: &Value,
        roots: &[Id],
        pos: &[Id],
        kwargs: Option<Id>,
        high_level: bool,
    ) -> Result<Option<Capture>> {
        let (callable, method) = match *target {
            Value::Method {
                callable, receiver, ..
            } => {
                // Reaping the last receiver reference on the creator would invoke __del__ there.
                if ctx.heap.is_pending_finalizer(receiver) {
                    return Ok(None);
                }
                (ctx.heap.get(callable), Some((callable, receiver)))
            }
            _ => (target, None),
        };
        let Value::Function {
            code: code_id,
            ref fields,
            scope,
            defaults,
            kwdefaults,
            ..
        } = *callable
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
        if let Some((function, receiver)) = method
            && let Some(serde_json::Value::Array(mut site)) = meta
                .get_mut("thread_projected_call")
                .map(serde_json::Value::take)
        {
            let [
                serde_json::Value::String(field),
                serde_json::Value::String(selector),
            ] = site.as_mut_slice()
            else {
                return Ok(None);
            };
            let field = std::mem::take(field);
            let selector = std::mem::take(selector);
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
                let sig = code
                    .definition
                    .call_parameters(|| CallParameters::from_metadata(&mut meta))?;
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
            let Value::PrimitiveLock(_) = ctx.heap.get(operand) else {
                return Ok(None);
            };
            // Retain the original method and projected wrapper until actual worker exit.
            // Canonical resolution remains on the creator before startup publication.
            let roots = super::concat_ids(roots, &[operand])?;
            return Ok(Some(Capture::Projected {
                operand,
                selector,
                roots,
            }));
        }
        let receiver = method.map(|(_, receiver)| receiver);
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
            let Some(lock) = Lock::from_value(ctx.heap.get(id)) else {
                return Ok(None);
            };
            // Detached errors use the original low-level reporting policy.
            if high_level && matches!(lock, Lock::Reentrant(_)) {
                return Ok(None);
            }
            let roots = super::concat_ids(roots, &[id])?;
            let mut callback = Self::from_code(ctx, code, scope, entry, Vec::new())?;
            callback.captured_lock = Some(lock);
            return Ok(Some(Capture::Ready(
                Operation::Python(Box::new(callback)),
                roots,
            )));
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
                return Ok(Some(Capture::Ready(
                    Operation::Acquire {
                        wait: Arc::clone(lock),
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
        let parameters = code
            .definition
            .call_parameters(|| CallParameters::from_metadata(&mut meta))?;
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
                |_, name| ctx.dict_named_item(kwdefaults, name).is_some(),
                crate::objects::exceptions::keyword_suggestion,
            )
            .map_err(|error| match error {
                crate::source_arguments::Error::Memory => Error::new("MemoryError", ""),
                _ => refused(),
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
        let mut first_slots = HashMap::new();
        first_slots
            .try_reserve(count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        // Keep one creator root per distinct bound value, in first occurrence order.
        let mut kept = 0;
        for slot in 0..bound.len() {
            let id = bound[slot];
            let first = *first_slots.entry(id).or_insert(slot);
            if first != slot {
                values.push(Argument::Alias(first));
                continue;
            }
            bound[kept] = id;
            kept += 1;
            let value = match ctx.heap.get(id) {
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
                value => Argument::Lock(Lock::from_value(value).ok_or_else(refused)?),
            };
            values.push(value);
        }
        retained.truncate(roots.len() + kept);
        let callback = Self::from_code(ctx, code, scope, entry, values)?;
        Ok(Some(Capture::Ready(
            Operation::Python(Box::new(callback)),
            retained,
        )))
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
            captured_lock: None,
            gc_settings: ctx.heap.gc_settings.clone(),
            allocation_traces: ctx.heap.allocation_traces.worker(),
        })
    }

    /// Prepare and park the registered call on this pthread. Only its creator can open the gate.
    /// Startup failures go to the creator. Call errors belong to the published worker.
    pub(super) fn run(
        self,
        interpreter: Arc<super::InterpreterState>,
        fault_workers: &Arc<()>,
        enter: impl FnOnce() -> bool + Send,
        detached_label: Option<String>,
    ) -> Result<WorkerStatus> {
        // Captured entries bypass argument selection. Older native code must still
        // have an empty signature to enter with only its closure cell.
        if self.captured_lock.is_some()
            && self
                .definition
                .call_parameters(|| CallParameters::from_signature(&self.signature))?
                .argument_slots()
                .next()
                .is_some()
        {
            return Err(unsupported());
        }
        let heap = super::Heap::with_worker_stack(
            Some(self.frames),
            self.gc_settings,
            self.allocation_traces,
        );
        let mut ctx = ffi::new_worker_context(interpreter, heap);
        let mark = ctx.heap.temporary_mark();
        let globals = ctx.scope(&self.module, None);
        let cell = self.captured_lock.map(|lock| {
            let value = ctx.alloc(lock.into_value());
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
            (globals, ctx.root_module_name(globals)),
            super::source_code::FunctionClosure::Cells(cell.as_slice()),
            (name, qualname),
            NONE,
            NONE,
        );
        ctx.heap.observe_fault_workers(fault_workers);
        let call = ffi::OperandCall::prepare(&mut ctx, mark, function, |ctx| {
            ctx.prepare_function_call(
                function,
                super::calls::FunctionArguments::Bound(self.values),
            )
        });
        if let Err(error) = &call {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
        }
        // SAFETY: only the native publication crosses the wait. The registered frame
        // and any exception roots stay in this pthread's parked execution.
        if !unsafe { ParkedExecution::wait(&raw mut ctx, enter) }? {
            return Ok(Ok(()));
        }
        // SAFETY: this pthread has resumed the same context and its marked caller.
        let result =
            call.and_then(|call| unsafe { call.run(&raw mut ctx, Context::finish_call_operands) });
        let mut status = Ok(());
        if let Err(error) = result {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
            if let Some(label) = detached_label {
                if error.kind != "SystemExit" {
                    let _ = writeln!(
                        crate::sys::stderr_writer(),
                        "Exception ignored in thread started by {label}:\n{}",
                        error.report(&ctx.heap)
                    );
                }
            } else {
                status = Err(error.report(&ctx.heap).to_string());
            }
        }
        ctx.unwind_operands(mark);
        Ok(status)
    }
}
