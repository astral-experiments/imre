//! Move executable code and lock ownership into a fresh boxed context.
//! Only the compiler can certify that the function needs no surrounding heap.

use super::source_code::CapturedAcquire;
use super::thread_worker::Operation;
use super::value::{Body, Code, CodeObject, Location};
use super::{Context, Error, Id, NONE, Result, Value, copy_label, ffi};
use crate::objects::locks::PrimitiveLock;
use crate::objects::logging::HandlerLock;
use std::collections::HashMap;
use std::sync::Arc;

enum Argument {
    Lock(Arc<PrimitiveLock>),
    RLock(Arc<HandlerLock>),
    Builtin(&'static str),
}

enum Entry {
    Native(Code),
    Source(CapturedAcquire),
}

pub(super) struct Callback {
    entry: Entry,
    module: String,
    name: String,
    qualname: String,
    filename: String,
    first_line: u32,
    signature: String,
    values: Vec<Argument>,
    receiver: Option<(String, Argument)>,
    config: serde_json::Value,
    automatic_gc: Arc<std::sync::atomic::AtomicBool>,
    finalizing: Arc<std::sync::atomic::AtomicBool>,
}

fn unsupported() -> Error {
    Error::new(
        "NotImplementedError",
        "this thread callback requires the boxed Python heap",
    )
}

/// Read an ordinary instance field only when its lookup cannot execute Python.
fn plain_field(ctx: &Context, receiver: Id, name: &str) -> Option<Id> {
    let Value::Instance {
        class,
        fields,
        namespace: None,
        ..
    } = ctx.heap.get(receiver)
    else {
        return None;
    };
    let Value::Class {
        scope, ancestors, ..
    } = ctx.heap.get(*class)
    else {
        return None;
    };
    if ancestors
        .iter()
        .any(|base| ctx.builtins.get("object") != Some(base))
        || ctx
            .scope_data(*scope)
            .class_namespace
            .as_ref()
            .is_some_and(|ns| ns.slot_names.is_some())
        || ctx.class_attr(*class, "__getattribute__").is_some()
        || ctx.class_attr(*class, "__getattr__").is_some()
        || ctx.class_attr(*class, name).is_some()
    {
        return None;
    }
    fields.get(name).copied()
}

impl Callback {
    pub(super) fn capture(
        ctx: &Context,
        target: Id,
        args: Id,
        pos: &[Id],
        kwargs: Option<Id>,
    ) -> Result<Option<(Operation, Vec<Id>)>> {
        Self::capture_checked(ctx, target, args, pos, kwargs, false)
    }

    /// Thread.run cannot invoke its parent's excepthook, so only certified lock operations qualify.
    pub(super) fn capture_for_thread(
        ctx: &Context,
        target: Id,
        args: Id,
        kwargs: Id,
    ) -> Result<Option<(Operation, Vec<Id>)>> {
        let (Value::Tuple(pos) | Value::List(pos)) = ctx.heap.get(args) else {
            return Err(unsupported());
        };
        if !matches!(ctx.heap.get(kwargs), Value::Dict(_)) {
            return Err(unsupported());
        }
        Self::capture_checked(ctx, target, args, pos, Some(kwargs), true)
    }

    fn capture_checked(
        ctx: &Context,
        target: Id,
        args: Id,
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
            code,
            scope,
            defaults,
            closure,
            ..
        } = *ctx.heap.get(function)
        else {
            return Ok(None);
        };
        let code = ctx.code_object(code);
        if code.coroutine || !code.closure_supported {
            return Ok(None);
        }
        let signature = ctx.str_value(code.parameters)?;
        let meta: serde_json::Value = serde_json::from_str(signature)
            .map_err(|e| Error::new("SystemError", e.to_string()))?;
        if let Some(receiver) = receiver {
            if !matches!(code.body, Body::Native(_)) {
                return Ok(None);
            }
            let Some(field) = meta["thread_bound_lock_field"].as_str() else {
                return Ok(None);
            };
            if !code.freevars.is_empty()
                || !pos.is_empty()
                || kwargs.is_some_and(
                    |id| !matches!(ctx.heap.get(id), Value::Dict(items) if items.is_empty()),
                )
            {
                return Ok(None);
            }
            let Some(operand) = plain_field(ctx, receiver, field) else {
                return Ok(None);
            };
            let Value::PrimitiveLock(lock) = ctx.heap.get(operand) else {
                return Ok(None);
            };
            // The lookup is bound before startup returns. The worker owns the native
            // lock and the parent retains the original method and Python lock wrapper.
            let mut roots = vec![target, args, operand];
            roots.extend(kwargs);
            return Ok(Some((
                Operation::Acquire {
                    lock: Arc::clone(lock),
                    release: None,
                },
                roots,
            )));
        }
        if let [name] = code.freevars.as_slice()
            && pos.is_empty()
            && kwargs.is_none_or(
                |id| matches!(ctx.heap.get(id), Value::Dict(entries) if entries.is_empty()),
            )
            && let Value::Tuple(cells) = ctx.heap.get(closure)
            && let [cell] = cells.as_slice()
            && let Value::Cell(Some(id)) = *ctx.heap.get(*cell)
        {
            let entry = match &code.body {
                Body::Native(entry) if meta["thread_captured_locks"].as_array().is_some_and(|names|
                    matches!(names.as_slice(), [one] if one.as_str() == Some(name.as_str()))) =>
                    Entry::Native(*entry),
                Body::Source(program) if ["posonly", "positional", "kwonly"].iter().all(|key|
                    meta[*key].as_array().is_some_and(Vec::is_empty))
                    && meta["varargs"].is_null() && meta["kwargs"].is_null() => {
                    let Some(original) = program.captured_acquire(name) else {
                        return Ok(None);
                    };
                    Entry::Source(original)
                }
                _ => return Ok(None),
            };
            let receiver = match ctx.heap.get(id) {
                Value::PrimitiveLock(lock) => Argument::Lock(Arc::clone(lock)),
                // Detached errors use the original low-level reporting policy.
                Value::HandlerLock(lock) if !high_level => Argument::RLock(Arc::clone(lock)),
                _ => return Ok(None),
            };
            let mut roots = vec![target, args, id];
            roots.extend(kwargs);
            let mut callback = Self::from_code(ctx, code, scope, entry, Vec::new())?;
            callback.receiver = Some((copy_label(name)?, receiver));
            return Ok(Some((Operation::Python(Box::new(callback)), roots)));
        }
        let Body::Native(entry) = code.body else {
            return Ok(None);
        };
        if !code.freevars.is_empty() {
            if high_level
                && pos.is_empty()
                && let Some(kwargs) = kwargs
                && matches!(ctx.heap.get(kwargs), Value::Dict(entries) if entries.is_empty())
                && let Some(names) = meta["thread_captured_locks"].as_array()
                && let Value::Tuple(cells) = ctx.heap.get(closure)
                && cells.len() == code.freevars.len()
                && cells.len() == names.len()
            {
                // Code freevars are in canonical name order; the proof is in execution order.
                let operand = |name: &serde_json::Value| {
                    let index = code
                        .freevars
                        .iter()
                        .position(|n| Some(n.as_str()) == name.as_str())?;
                    let Value::Cell(Some(id)) = *ctx.heap.get(cells[index]) else {
                        return None;
                    };
                    let Value::PrimitiveLock(lock) = ctx.heap.get(id) else {
                        return None;
                    };
                    Some((id, lock))
                };
                let [first, last] = names.as_slice() else {
                    return Err(unsupported());
                };
                let (_, release) = operand(first).ok_or_else(unsupported)?;
                let (id, lock) = operand(last).ok_or_else(unsupported)?;
                // No other native owner can race this held release before startup completes.
                if Arc::strong_count(release) != 1
                    || !release.locked()
                    || Arc::ptr_eq(release, lock)
                {
                    return Err(unsupported());
                }
                // The prefix has been consumed when the parent resumes. Only the final
                // receiver must outlive a subsequent replacement of its closure cell.
                return Ok(Some((
                    Operation::Acquire {
                        lock: Arc::clone(lock),
                        release: Some(Arc::clone(release)),
                    },
                    vec![target, args, kwargs, id],
                )));
            }
            return Ok(None);
        }
        if closure != NONE
            && !matches!(ctx.heap.get(closure), Value::Tuple(items) if items.is_empty())
        {
            return Ok(None);
        }
        let Some(names) = meta["thread_parameters"].as_array() else {
            return Ok(None);
        };
        if high_level && meta["thread_worker_locks"].as_array() != Some(names) {
            return Ok(None);
        }
        let kw = kwargs
            .and_then(|id| ctx.dict_entries(id))
            .unwrap_or_default();
        if pos.len() > names.len() {
            return Err(unsupported());
        }
        let posonly = meta["posonly"].as_array().map_or(0, Vec::len);
        let mut bound = Vec::new();
        bound
            .try_reserve_exact(names.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        bound.extend(pos.iter().copied().map(Some));
        bound.resize(names.len(), None);
        for &(key, value) in kw.iter() {
            let Value::Str(key) = ctx.heap.get(key) else {
                return Err(unsupported());
            };
            let index = names
                .iter()
                .position(|n| n.as_str() == Some(key))
                .ok_or_else(unsupported)?;
            if index < posonly || bound[index].replace(value).is_some() {
                return Err(unsupported());
            }
        }
        if let Value::Tuple(defaults) = ctx.heap.get(defaults) {
            for (slot, default) in bound.iter_mut().rev().zip(defaults.iter().rev()) {
                slot.get_or_insert(*default);
            }
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(bound.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mut roots = vec![target, args];
        roots.extend(kwargs);
        for slot in bound {
            let id = slot.ok_or_else(unsupported)?;
            let value = match ctx.heap.get(id) {
                Value::PrimitiveLock(lock) => Argument::Lock(Arc::clone(lock)),
                Value::HandlerLock(lock) => Argument::RLock(Arc::clone(lock)),
                Value::Native(name) if !high_level => {
                    let symbol = [
                        "_thread._count",
                        "sys.exit",
                        "gc.enable",
                        "gc.disable",
                        "gc.isenabled",
                    ]
                    .into_iter()
                    .find(|symbol| *symbol == name)
                    .ok_or_else(unsupported)?;
                    Argument::Builtin(symbol)
                }
                _ => return Err(unsupported()),
            };
            roots.push(id);
            values.push(value);
        }
        let callback = Self::from_code(ctx, code, scope, Entry::Native(entry), values)?;
        Ok(Some((Operation::Python(Box::new(callback)), roots)))
    }

    fn from_code(
        ctx: &Context,
        code: &CodeObject,
        scope: Id,
        entry: Entry,
        values: Vec<Argument>,
    ) -> Result<Self> {
        Ok(Self {
            entry,
            module: copy_label(&ctx.scope_data(scope).module)?,
            name: copy_label(ctx.str_value(code.name)?)?,
            qualname: copy_label(ctx.str_value(code.qualname)?)?,
            filename: copy_label(ctx.str_value(code.filename)?)?,
            first_line: code.first_line,
            signature: copy_label(ctx.str_value(code.parameters)?)?,
            values,
            receiver: None,
            config: ctx.config.clone(),
            automatic_gc: ctx.heap.automatic_gc.clone(),
            finalizing: ctx.finalizing.clone(),
        })
    }

    pub(super) fn awaits_receiver(&self) -> bool {
        self.receiver.is_some()
    }

    pub(super) fn load(self) -> Result<(Context, Id, Vec<Id>)> {
        let mut ctx = ffi::new_context(self.config, std::iter::empty::<String>());
        ctx.heap.automatic_gc = self.automatic_gc;
        ctx.finalizing = self.finalizing;
        let globals = ctx.scope(&self.module, None);
        let scope = if let Some((name, receiver)) = self.receiver {
            let value = match receiver {
                Argument::Lock(lock) => ctx.alloc(Value::PrimitiveLock(lock)),
                Argument::RLock(lock) => ctx.alloc(Value::HandlerLock(lock)),
                Argument::Builtin(_) => unreachable!("the terminal receiver is an exact lock"),
            };
            let cell = ctx.alloc(Value::Cell(Some(value)));
            ctx.closure_environment(globals, vec![name], &[cell])
        } else {
            globals
        };
        let name = ctx.text(self.name);
        let qualname = ctx.text(self.qualname);
        let signature = ctx.text(self.signature);
        let location = Location {
            file: self.filename.into(),
            line: self.first_line,
            span: None,
        };
        let code = match self.entry {
            Entry::Native(entry) => {
                ctx.prelinked_code(entry, scope, name, signature, (qualname, NONE), location)
            }
            Entry::Source(original) => {
                original.load(&mut ctx, scope, name, signature, qualname, location)?
            }
        };
        let defaults = ctx.dict(Vec::new());
        let function =
            ctx.define_function(code, scope, name, defaults, qualname, NONE, NONE, false);
        // Preserve object identity when multiple parameters alias the same lock.
        let mut aliases = HashMap::new();
        aliases
            .try_reserve(self.values.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mut args = Vec::new();
        args.try_reserve_exact(self.values.len())
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
            args.push(id);
        }
        Ok((ctx, function, args))
    }
}
