//! Native warning state belongs to the interpreter and persists across fresh imports.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::logging::HandlerLock;
use std::sync::{Arc, atomic::Ordering};

mod display;
pub(in crate::dynamic) mod explicit;
pub(in crate::dynamic) mod warn;

pub(in crate::dynamic) struct WarningLocation {
    pub(in crate::dynamic) filename: Id,
    pub(in crate::dynamic) lineno: i32,
    pub(in crate::dynamic) line: Option<Id>,
}

pub(in crate::dynamic) struct State {
    pub(in crate::dynamic) filters: Id,
    pub(in crate::dynamic) once_registry: Id,
    pub(in crate::dynamic) default_action: Id,
    pub(in crate::dynamic) context: Id,
    pub(in crate::dynamic) lock: Id,
    pub(in crate::dynamic) version: u64,
}

impl State {
    pub(in crate::dynamic) fn roots(&self) -> impl Iterator<Item = &Id> {
        [
            &self.filters,
            &self.once_registry,
            &self.default_action,
            &self.context,
            &self.lock,
        ]
        .into_iter()
    }
}

/// Fresh imports bind new methods while retaining the same interpreter-owned objects.
pub(in crate::dynamic) fn ensure_state(ctx: &mut Context) -> Result<&mut State> {
    if ctx.warning_state.is_none() {
        let main = ctx.text("__main__");
        let default_action = ctx.text("default");
        let ignore = ctx.text("ignore");
        let zero = ctx.int(0);
        let mut entries = Vec::new();
        for (action, category, module) in [
            (default_action, "DeprecationWarning", main),
            (ignore, "DeprecationWarning", NONE),
            (ignore, "PendingDeprecationWarning", NONE),
            (ignore, "ImportWarning", NONE),
            (ignore, "ResourceWarning", NONE),
        ] {
            let category = ctx.builtin(category);
            entries.push(ctx.alloc(Value::Tuple(vec![action, NONE, category, module, zero])));
        }
        let filters = ctx.alloc(Value::List(entries));
        let once_registry = ctx.dict(Vec::new());
        let context_name = ctx.text("_warnings_context");
        let context = super::contextvars::call(ctx, "ContextVar", &[context_name], &[])?;
        let lock = ctx.alloc(Value::HandlerLock(Default::default()));
        ctx.warning_state = Some(State {
            filters,
            once_registry,
            default_action,
            context,
            lock,
            version: 0,
        });
    }
    Ok(ctx
        .warning_state
        .as_mut()
        .expect("initialized warnings state"))
}

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let state = ensure_state(ctx)?;
    for (name, value) in [
        ("filters", state.filters),
        ("_defaultaction", state.default_action),
        ("_onceregistry", state.once_registry),
        ("_warnings_context", state.context),
    ] {
        ctx.bind(scope, name, value);
    }
    let doc = ctx.text("_warnings provides basic warning filtering support.\nIt is a helper module to speed up interpreter start-up.");
    ctx.bind(scope, "__doc__", doc);
    for name in [
        "warn",
        "warn_explicit",
        "_acquire_lock",
        "_release_lock",
        "_filters_mutated_lock_held",
    ] {
        let callable = ctx.builtin(&format!("_warnings.{name}"));
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, function);
    }
    Ok(())
}

/// Argument Clinic metadata exposed by the five module-bound methods.
pub(in crate::dynamic) fn method_metadata(
    name: &str,
) -> Option<(&'static str, Option<&'static str>)> {
    match name {
        "warn" => Some((
            "($module, /, message, category=None, stacklevel=1, source=None, *,\n     skip_file_prefixes=<unrepresentable>)",
            Some(
                "Issue a warning, or maybe ignore it or raise an exception.\n\n  message\n    Text of the warning message.\n  category\n    The Warning category subclass. Defaults to UserWarning.\n  stacklevel\n    How far up the call stack to make this warning appear. A value of 2 for\n    example attributes the warning to the caller of the code calling warn().\n  source\n    If supplied, the destroyed object which emitted a ResourceWarning\n  skip_file_prefixes\n    An optional tuple of module filename prefixes indicating frames to skip\n    during stacklevel computations for stack frame attribution.",
            ),
        )),
        "warn_explicit" => Some((
            "($module, /, message, category, filename, lineno,\n              module=<unrepresentable>, registry=None,\n              module_globals=None, source=None)",
            Some("Issue a warning, or maybe ignore it or raise an exception."),
        )),
        "_acquire_lock" | "_release_lock" | "_filters_mutated_lock_held" => {
            Some(("($module, /)", None))
        }
        _ => None,
    }
}

fn lock(ctx: &mut Context) -> Result<Arc<HandlerLock>> {
    let id = ensure_state(ctx)?.lock;
    let Value::HandlerLock(lock) = ctx.heap.get(id) else {
        unreachable!("private warnings lock")
    };
    Ok(lock.clone())
}

pub(in crate::dynamic) fn with_lock<T>(
    ctx: &mut Context,
    operation: impl FnOnce(&mut Context) -> Result<T>,
) -> Result<T> {
    let lock = lock(ctx)?;
    lock.acquire(true, None)?;
    let result = operation(ctx);
    // An invoked Python hook can release the lock itself. Preserve its result or exception.
    let _ = lock.release();
    result
}

/// Consult the current public module; callers handle missing state attributes.
pub(in crate::dynamic) fn warnings_attr(
    ctx: &mut Context,
    attr: &str,
    try_import: bool,
) -> Result<Option<Id>> {
    let module = if try_import && !ctx.finalizing.load(Ordering::Acquire) {
        match ctx.import_with_builtins("warnings") {
            Ok(module) => module,
            Err(error) if ctx.is_builtin_error(&error, "ImportError") => return Ok(None),
            Err(error) => return Err(error),
        }
    } else {
        let name = ctx.heap.code_name("warnings");
        let Some(module) = ctx.registered_module(name)? else {
            return Ok(None);
        };
        module
    };
    match ctx.lookup_attr(module, attr) {
        Ok(value) => Ok(Some(value)),
        Err(error) if ctx.is_attribute_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((_, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "expected a _warnings module receiver",
        ));
    };
    match name {
        "warn" => return warn::warn(ctx, args, kwargs),
        "warn_explicit" => return explicit::warn_explicit(ctx, args, kwargs),
        _ => {}
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("_warnings.{name}() takes no keyword arguments"),
        ));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "_warnings.{name}() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    let lock = lock(ctx)?;
    match name {
        "_acquire_lock" => {
            lock.acquire(true, None)?;
        }
        "_release_lock" => lock.release()?,
        "_filters_mutated_lock_held" => {
            if lock.state().is_none() {
                return Err(Error::new("RuntimeError", "warnings lock is not held"));
            }
            ensure_state(ctx)?.version += 1;
        }
        _ => unreachable!("unregistered _warnings function"),
    }
    Ok(NONE)
}
