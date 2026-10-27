//! Native allocation and initialization slots inherited by Python exception classes.

use super::*;

/// Allocate the requested exception subtype before its Python initializer runs.
pub(super) fn allocate(ctx: &mut Context, owner: &str, args: &[Id]) -> Result<Id> {
    let Some((&class, args)) = args.split_first() else {
        return Err(type_error(format!(
            "{owner}.__new__(): not enough arguments"
        )));
    };
    let name = match ctx.heap.get(class) {
        Value::Class { name, .. } => name.clone(),
        Value::Native(name) if is_builtin_type(name) => name.clone(),
        _ => {
            return Err(type_error(format!(
                "{owner}.__new__(X): X is not a type object ({})",
                ctx.type_name(class)
            )));
        }
    };
    let base = ctx.builtin(owner);
    if !ctx.subclass(class, base) {
        return Err(type_error(format!(
            "{owner}.__new__({name}): {name} is not a subtype of {owner}"
        )));
    }
    for specialized in ["OSError", "MemoryError"] {
        if owner != specialized {
            let base = ctx.builtin(specialized);
            if ctx.subclass(class, base) {
                return Err(type_error(format!(
                    "{owner}.__new__({name}) is not safe, use {name}.__new__()"
                )));
            }
        }
    }
    let import = ctx.builtin("ImportError");
    let mut error = ctx.exception(&name, args);
    // StopIteration.__new__ retains the arguments; only its initializer sets value.
    error.set_stop_value(NONE);
    if ctx.subclass(class, import) {
        error.tracing().import_message = Some(NONE);
    }
    if matches!(ctx.heap.get(class), Value::Class { .. }) {
        error.class = Some(class);
    }
    Ok(ctx.alloc(Value::Raised(error)))
}

/// Replace the argument payload without losing the exception's class or traceback.
pub(super) fn initialize(
    ctx: &mut Context,
    owner: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&object, args)) = args.split_first() else {
        return Err(type_error(format!(
            "descriptor '__init__' of '{owner}' object needs an argument"
        )));
    };
    let base = ctx.builtin(owner);
    if !ctx.instance_of(object, base) {
        return Err(type_error(format!(
            "descriptor '__init__' requires a '{owner}' object but received a '{}'",
            ctx.type_name(object)
        )));
    }
    let Value::Raised(error) = ctx.heap.get(object) else {
        unreachable!()
    };
    let kind = error.kind.clone();
    if owner != "ImportError" && !kwargs.is_empty() {
        return Err(type_error(format!("{kind}() takes no keyword arguments")));
    }
    // Import failures created by the loader can still have a lazy message.
    // BaseException.__init__ replaces only the args, even if msg was never read.
    let _ = ctx.import_error_message(object, None);
    let initialized = ctx.exception(&kind, args);
    let Value::Raised(error) = ctx.heap.get_mut(object) else {
        unreachable!()
    };
    if owner == "StopIteration" {
        error.set_stop_value(args.first().copied().unwrap_or(NONE));
    }
    error.args = initialized.args;
    error.message = initialized.message;
    if owner == "ImportError" {
        initialize_import(error, args, kwargs)?;
    }
    Ok(NONE)
}

/// Set ImportError's independent message and keyword fields after args is replaced.
pub(super) fn initialize_import(
    error: &mut Error,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<()> {
    if kwargs.len() > IMPORT_KEYWORDS.len() {
        return Err(type_error(format!(
            "ImportError() takes at most 3 keyword arguments ({} given)",
            kwargs.len()
        )));
    }
    let mut names = [NONE; IMPORT_KEYWORDS.len()];
    for (key, value) in kwargs {
        let Some(index) = IMPORT_KEYWORDS.iter().position(|&name| name == key) else {
            return Err(type_error(format!(
                "ImportError() got an unexpected keyword argument '{key}'"
            )));
        };
        names[index] = *value;
    }
    error.import = None;
    let trace = error.tracing();
    trace.import_message = Some(match args {
        [one] => *one,
        _ => NONE,
    });
    trace.import_names = (!kwargs.is_empty()).then_some(names);
    Ok(())
}
