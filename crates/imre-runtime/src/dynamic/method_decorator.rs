//! Metadata shared by classmethod and staticmethod descriptors.
use super::Context;
use super::value::{Error, FALSE, Id, Result, TRUE, Value};

/// Copy the eagerly assigned function metadata without evaluating its annotations.
fn copy_metadata(ctx: &mut Context, callable: Id) -> Result<Id> {
    let dictionary = ctx.dict(Vec::new());
    for name in ["__module__", "__name__", "__qualname__", "__doc__"] {
        match ctx.lookup_attr(callable, name) {
            Ok(value) => {
                let key = ctx.text(name);
                ctx.put_item(dictionary, key, value)?;
            }
            Err(error) if ctx.is_attribute_error(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(dictionary)
}

pub(super) fn class_method(ctx: &mut Context, callable: Id) -> Result<Id> {
    let dictionary = copy_metadata(ctx, callable)?;
    Ok(ctx.alloc(Value::ClassMethod {
        callable,
        dictionary,
    }))
}

pub(super) fn static_method(ctx: &mut Context, callable: Id) -> Result<Id> {
    let dictionary = copy_metadata(ctx, callable)?;
    Ok(ctx.alloc(Value::StaticMethod {
        callable,
        dictionary,
    }))
}

/// Resolve descriptor members before looking in the wrapper's writable dictionary.
pub(super) fn attribute(
    ctx: &mut Context,
    callable: Id,
    dictionary: Id,
    name: &str,
) -> Result<Option<Id>> {
    match name {
        "__func__" | "__wrapped__" => return Ok(Some(callable)),
        "__dict__" => return Ok(Some(dictionary)),
        "__isabstractmethod__" => {
            let required = ctx.is_abstract_method(callable)?;
            return Ok(Some(if required { TRUE } else { FALSE }));
        }
        _ => {}
    }
    let key = ctx.text(name);
    let storage = ctx
        .dict_storage(dictionary)
        .expect("method wrapper dictionary");
    // A callback may replace the wrapper dictionary while this lookup is in progress.
    ctx.heap.keep(dictionary);
    let (_, index) = ctx.find_dictionary_key(storage, key, None)?;
    if let Some(index) = index {
        return Ok(Some(ctx.dict_entries(storage).unwrap()[index].1));
    }
    if matches!(name, "__annotations__" | "__annotate__") {
        let value = ctx.lookup_attr(callable, name)?;
        ctx.put_item(storage, key, value)?;
        return Ok(Some(value));
    }
    Ok(None)
}

/// Keep the underlying callable read-only while allowing changes to the wrapper metadata.
pub(super) fn update_attribute(
    ctx: &mut Context,
    wrapper: Id,
    dictionary: Id,
    name: &str,
    value: Option<Id>,
) -> Result<()> {
    match name {
        "__func__" | "__wrapped__" => Err(Error::new("AttributeError", "readonly attribute")),
        "__isabstractmethod__" => Err(Error::new(
            "AttributeError",
            format!(
                "attribute '__isabstractmethod__' of '{}' objects is not writable",
                ctx.type_name(wrapper)
            ),
        )),
        "__dict__" => {
            let Some(value) = value else {
                return Err(Error::new("TypeError", "cannot delete __dict__"));
            };
            if ctx.dict_storage(value).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__dict__ must be set to a dictionary, not a '{}'",
                        ctx.type_name(value)
                    ),
                ));
            }
            let (Value::StaticMethod { dictionary, .. } | Value::ClassMethod { dictionary, .. }) =
                ctx.heap.get_mut(wrapper)
            else {
                unreachable!()
            };
            let previous = std::mem::replace(dictionary, value);
            if previous != value {
                ctx.heap.weak_edge_removed(previous);
            }
            Ok(())
        }
        _ => {
            let storage = ctx
                .dict_storage(dictionary)
                .expect("method wrapper dictionary");
            if let Some(value) = value {
                let key = ctx.text(name);
                ctx.put_item(storage, key, value)
            } else {
                ctx.delete_dict_attribute(wrapper, storage, name)
            }
        }
    }
}
