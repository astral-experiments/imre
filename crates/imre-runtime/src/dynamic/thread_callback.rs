//! Move executable code and lock ownership into a fresh boxed context.
//! Only the compiler can certify that the function needs no surrounding heap.

use super::value::{Body, Code, Location};
use super::{Context, Error, Id, NONE, Result, Value, copy_label, ffi};
use crate::objects::locks::PrimitiveLock;
use std::collections::HashMap;
use std::sync::Arc;

enum Argument {
    Lock(Arc<PrimitiveLock>),
    Builtin(&'static str),
}

pub(super) struct Callback {
    entry: Code,
    module: String,
    name: String,
    qualname: String,
    filename: String,
    first_line: u32,
    signature: String,
    values: Vec<Argument>,
    config: serde_json::Value,
}

fn unsupported() -> Error {
    Error::new(
        "NotImplementedError",
        "this thread callback requires the boxed Python heap",
    )
}

impl Callback {
    pub(super) fn capture(
        ctx: &Context,
        target: Id,
        args: Id,
        pos: &[Id],
        kwargs: Option<Id>,
    ) -> Result<Option<(Self, Vec<Id>)>> {
        Self::capture_checked(ctx, target, args, pos, kwargs, false)
    }

    /// Thread.run cannot invoke its parent's excepthook, so only certified lock operations qualify.
    pub(super) fn capture_for_thread(
        ctx: &Context,
        target: Id,
        args: Id,
        kwargs: Id,
    ) -> Result<Option<(Self, Vec<Id>)>> {
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
    ) -> Result<Option<(Self, Vec<Id>)>> {
        let Value::Function {
            code,
            scope,
            defaults,
            closure,
            ..
        } = *ctx.heap.get(target)
        else {
            return Ok(None);
        };
        let code = ctx.code_object(code);
        let Body::Native(entry) = code.body else {
            return Ok(None);
        };
        if !code.freevars.is_empty()
            || code.coroutine
            || !code.closure_supported
            || (closure != NONE
                && !matches!(ctx.heap.get(closure), Value::Tuple(items) if items.is_empty()))
        {
            return Ok(None);
        }
        let signature = ctx.str_value(code.parameters)?;
        let meta: serde_json::Value = serde_json::from_str(signature)
            .map_err(|e| Error::new("SystemError", e.to_string()))?;
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
                Value::Native(name) if !high_level && name == "_thread._count" => {
                    Argument::Builtin("_thread._count")
                }
                Value::Native(name) if !high_level && name == "sys.exit" => {
                    Argument::Builtin("sys.exit")
                }
                _ => return Err(unsupported()),
            };
            roots.push(id);
            values.push(value);
        }
        Ok(Some((
            Self {
                entry,
                module: copy_label(&ctx.scope_data(scope).module)?,
                name: copy_label(ctx.str_value(code.name)?)?,
                qualname: copy_label(ctx.str_value(code.qualname)?)?,
                filename: copy_label(ctx.str_value(code.filename)?)?,
                first_line: code.first_line,
                signature: copy_label(signature)?,
                values,
                config: ctx.config.clone(),
            },
            roots,
        )))
    }

    pub(super) fn load(self) -> Result<(Context, Id, Vec<Id>)> {
        let mut ctx = ffi::new_context(self.config, std::iter::empty::<String>());
        let globals = ctx.scope(&self.module, None);
        let name = ctx.text(self.name);
        let qualname = ctx.text(self.qualname);
        let signature = ctx.text(self.signature);
        let code = ctx.prelinked_code(
            self.entry,
            globals,
            name,
            signature,
            (qualname, NONE),
            Location {
                file: self.filename.into(),
                line: self.first_line,
                span: None,
            },
        );
        let defaults = ctx.dict(Vec::new());
        let function =
            ctx.define_function(code, globals, name, defaults, qualname, NONE, NONE, false);
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
                    .entry(Arc::as_ptr(&lock))
                    .or_insert_with(|| ctx.alloc(Value::PrimitiveLock(lock))),
                Argument::Builtin(name) => ctx.builtin(name),
            };
            args.push(id);
        }
        Ok((ctx, function, args))
    }
}
