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
    if owner == "BaseExceptionGroup" {
        return allocate_group(ctx, class, args);
    }
    for specialized in ["OSError", "MemoryError", "BaseExceptionGroup"] {
        if owner != specialized {
            let base = ctx.builtin(specialized);
            if ctx.subclass(class, base) {
                return Err(type_error(format!(
                    "{owner}.__new__({name}) is not safe, use {name}.__new__()"
                )));
            }
        }
    }
    let python_class = matches!(ctx.heap.get(class), Value::Class { .. }).then_some(class);
    let mut error = if owner == "OSError" {
        let args = if python_class.is_some() && ctx.deferred_os_init(class) {
            &[]
        } else {
            args
        };
        ctx.os_exception(&name, python_class, args, None)?
    } else {
        ctx.exception(&name, args)
    };
    // Allocation retains args. The initializers set StopIteration.value and SystemExit.code.
    error.set_stop_value(NONE);
    error.exit_code = NONE;
    let import = ctx.builtin("ImportError");
    if ctx.subclass(class, import) {
        error.tracing().import_message = Some(NONE);
    }
    error.class = python_class.unwrap_or(NONE);
    Ok(ctx.alloc(Value::Raised(error)))
}

/// Construct an exception group, retaining the original args and a separate child tuple.
pub(super) fn allocate_group(ctx: &mut Context, mut class: Id, args: &[Id]) -> Result<Id> {
    let [message, source] = *args else {
        return Err(type_error(format!(
            "BaseExceptionGroup.__new__() takes exactly 2 arguments ({} given)",
            args.len()
        )));
    };
    if ctx.string_storage(message).is_none() {
        return Err(type_error(format!(
            "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
            ctx.type_name(message)
        )));
    }
    if !ctx.is_sequence(source) {
        return Err(type_error(
            "second argument (exceptions) must be a sequence",
        ));
    }
    let source_type = ctx.exact_type(source)?;
    let list = ctx.builtin("list");
    let tuple = ctx.builtin("tuple");
    let sequence_repr = if ctx.subclass(source_type, list) || ctx.subclass(source_type, tuple) {
        None
    } else {
        Some(super::text::construct(ctx, "repr", &[source], &[])?)
    };
    let members = super::collections::construct(ctx, "tuple", &[source], &[])?;
    let exception = ctx.builtin("Exception");
    let Some(children) = ctx.tuple_items(members)? else {
        unreachable!()
    };
    if children.is_empty() {
        return Err(Error::new(
            "ValueError",
            "second argument (exceptions) must be a non-empty sequence",
        ));
    }
    let mut only_exceptions = true;
    for (index, child) in children.into_iter().enumerate() {
        if !matches!(ctx.heap.get(child), Value::Raised(_)) {
            return Err(Error::new(
                "ValueError",
                format!("Item {index} of second argument (exceptions) is not an exception"),
            ));
        }
        only_exceptions &= ctx.python_instance_check(child, exception)?;
    }

    if class == ctx.builtin("BaseExceptionGroup") && only_exceptions {
        class = ctx.builtin("ExceptionGroup");
    } else if !only_exceptions && ctx.subclass(class, exception) {
        return Err(type_error(if class == ctx.builtin("ExceptionGroup") {
            "Cannot nest BaseExceptions in an ExceptionGroup".to_owned()
        } else {
            format!("Cannot nest BaseExceptions in '{}'", ctx.class_name(class))
        }));
    }
    let name = ctx.class_name(class).to_owned();
    let mut error = Error::new(&name, "");
    error.args = Some(ctx.alloc(Value::Tuple(args.to_vec())));
    if matches!(ctx.heap.get(class), Value::Class { .. }) {
        error.class = class;
    }
    error.tracing().group = Some(crate::dynamic::value::GroupData {
        message,
        exceptions: members,
        sequence_repr,
    });
    error.message = ctx.exception_message(&error);
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
    let class = error.class();
    let prior = error.os_args.clone();
    if owner == "OSError" && !class.is_some_and(|id| ctx.deferred_os_init(id)) {
        return Ok(NONE);
    }
    if owner != "ImportError" && !kwargs.is_empty() {
        return Err(type_error(format!("{kind}() takes no keyword arguments")));
    }
    let _ = ctx.import_error_message(object, None);
    let initialized = if owner == "OSError" {
        ctx.os_exception(&kind, class, args, prior.as_deref())?
    } else {
        ctx.exception(&kind, args)
    };
    let Value::Raised(error) = ctx.heap.get_mut(object) else {
        unreachable!()
    };
    if owner == "StopIteration" {
        error.set_stop_value(args.first().copied().unwrap_or(NONE));
    }
    if owner == "OSError" {
        error.os_args = initialized.os_args;
    }
    error.args = initialized.args;
    error.message = initialized.message;
    if owner == "ImportError" {
        initialize_import(error, args, kwargs)?;
    } else if owner == "SystemExit" {
        let old = error.exit_code;
        error.initialize_exit_code(args);
        if old != error.exit_code {
            ctx.heap.weak_edge_removed(old);
        }
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

impl Context {
    /// Append the original string to the exception's Python-visible notes list.
    pub(in crate::dynamic) fn add_exception_note(&mut self, exception: Id, note: Id) -> Result<()> {
        if self.string_storage(note).is_none() {
            let kind = if note == NONE {
                "None"
            } else {
                let class = self.exact_type(note)?;
                self.class_name(class)
            };
            let kind = &kind[..kind.floor_char_boundary(50)];
            return Err(type_error(format!(
                "add_note() argument must be str, not {kind}"
            )));
        }
        let key = self.heap.string_literal("__notes__")?;
        let notes = match self.lookup_named_attr(exception, "__notes__", Some(key)) {
            Ok(notes) => notes,
            Err(error) if self.is_attribute_error(&error) => {
                let notes = self.alloc(Value::List(Vec::new()));
                let mark = self.heap.temporary_mark();
                let result = self.set_named_attr(exception, "__notes__", Some(key), notes);
                self.heap.release_temporaries_since(mark);
                result?;
                notes
            }
            Err(error) => return Err(error),
        };
        if self.list_storage(notes).is_none() {
            return Err(type_error("Cannot add note: __notes__ is not a list"));
        }
        collections::method(self, "list.append", &[notes, note], &[])?;
        Ok(())
    }
}
