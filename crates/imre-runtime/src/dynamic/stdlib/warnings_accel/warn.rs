use super::WarningLocation;
use crate::dynamic::{
    Context, builtins,
    value::{Error, Id, Location, NONE, Result, Value},
};

/// Bind the native warn entry point before consulting the caller's globals or filters.
pub(in crate::dynamic) fn warn(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let names = [
        "message",
        "category",
        "stacklevel",
        "source",
        "skip_file_prefixes",
    ];
    let [message, category, level, source, prefixes] = if args.len() > 4 {
        builtins::bind_method_arguments("warn", args, kwargs, names, 4)?
    } else {
        builtins::bind_builtin_arguments("warn", args, kwargs, names, 1)?
    };
    let message = message.expect("required warning message");
    let mut level = match level {
        Some(index) => super::super::os::ssize_t(ctx, index)?,
        None => 1,
    };
    let prefixes = match prefixes {
        Some(value) => ctx.tuple_items(value)?.ok_or_else(|| {
            let actual = if value == NONE {
                "None".into()
            } else {
                super::super::diagnostic_type_name(ctx, value)
            };
            Error::new(
                "TypeError",
                format!("warn() argument 'skip_file_prefixes' must be tuple, not {actual}"),
            )
        })?,
        None => Vec::new(),
    };
    let warning = ctx.builtin("Warning");
    let category = if is_warning(ctx, message)? {
        ctx.exact_type(message)?
    } else {
        category
            .filter(|id| *id != NONE)
            .unwrap_or_else(|| ctx.builtin("UserWarning"))
    };
    if !ctx
        .python_subclass_check(category, warning)
        .unwrap_or(false)
    {
        return Err(Error::new(
            "TypeError",
            format!(
                "category must be a Warning subclass, not '{}'",
                super::super::diagnostic_type_name(ctx, category)
            ),
        ));
    }
    for &prefix in &prefixes {
        if ctx.string_storage(prefix).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "Found non-str '{}' in skip_file_prefixes.",
                    super::super::diagnostic_type_name(ctx, prefix)
                ),
            ));
        }
    }
    if !prefixes.is_empty() {
        level = level.max(2);
    }
    let (filename, lineno, globals) = match caller(ctx, level, &prefixes) {
        Some((location, scope)) => (
            ctx.text(location.file.as_ref()),
            location.line as i32,
            ctx.module_globals(scope)?,
        ),
        None => {
            let sys = match ctx.threads.system_module {
                Some(sys) => sys,
                None => ctx.module("sys")?,
            };
            let Value::Module { scope, .. } = *ctx.heap.get(sys) else {
                unreachable!("interpreter sys module")
            };
            (ctx.text("<sys>"), 0, ctx.module_globals(scope)?)
        }
    };
    let registry = ctx.emission_warning_registry(globals)?;
    ctx.heap.keep(registry);
    let storage = ctx
        .dict_storage(globals)
        .expect("frame globals are a dictionary");
    let name = ctx.heap.code_name("__name__");
    let (_, index) = ctx.find_dictionary_key(storage, name, None)?;
    let module = index
        .map(|i| ctx.stored_entries(storage)[i].1)
        .filter(|id| *id == NONE || ctx.string_storage(*id).is_some())
        .unwrap_or_else(|| ctx.text("<string>"));
    ctx.heap.keep(module);
    super::explicit::emit(
        ctx,
        message,
        category,
        WarningLocation {
            filename,
            lineno,
            line: None,
        },
        Some(module),
        registry,
        source.unwrap_or(NONE),
    )
}

/// Like PyObject_IsInstance with Warning, honor an object's reported class after the real type.
pub(super) fn is_warning(ctx: &mut Context, message: Id) -> Result<bool> {
    let warning = ctx.builtin("Warning");
    let actual = ctx.exact_type(message)?;
    if ctx.direct_subclass(actual, warning) {
        return Ok(true);
    }
    match ctx.lookup_attr(message, "__class__") {
        Ok(reported) => Ok(reported != actual
            && ctx.is_type_object(reported)
            && ctx.direct_subclass(reported, warning)),
        Err(error) if ctx.is_attribute_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn caller(ctx: &Context, level: isize, prefixes: &[Id]) -> Option<(Location, Id)> {
    let mut frames = ctx.heap.warning_frames();
    let first = frames.next()?;
    // The C frame walk pre-decrements the signed stack level before its first step.
    let steps = level.wrapping_sub(1);
    let frame = if steps <= 0 {
        first
    } else {
        let internal = |file: &str| file.contains("importlib") && file.contains("_bootstrap");
        let skip = !internal(&first.0.file);
        frames
            .filter(|(location, _)| {
                !skip
                    || !(internal(&location.file)
                        || prefixes.iter().any(|&id| {
                            // A lone surrogate cannot prefix a native UTF-8 code filename.
                            ctx.str_value(id)
                                .is_ok_and(|prefix| location.file.starts_with(prefix))
                        }))
            })
            .nth((steps - 1) as usize)?
    };
    Some(frame)
}
