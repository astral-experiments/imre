//! A static method unwraps to the same callable for class and instance access.
use super::Context;
use super::value::{Error, FALSE, Id, Result, TRUE, Value};

/// Copy the eagerly assigned function metadata without evaluating its annotations.
pub(super) fn construct(ctx: &mut Context, callable: Id) -> Result<Id> {
    let dictionary = ctx.dict(Vec::new());
    for name in ["__module__", "__name__", "__qualname__", "__doc__"] {
        match ctx.lookup_attr(callable, name) {
            Ok(value) => {
                let key = ctx.text(name);
                ctx.put_item(dictionary, key, value)?;
            }
            Err(error) if error.kind == "AttributeError" => {}
            Err(error) => return Err(error),
        }
    }
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
            let required = match ctx.lookup_attr(callable, name) {
                Ok(value) => ctx.truth(value)?,
                Err(error) if error.kind == "AttributeError" => false,
                Err(error) => return Err(error),
            };
            return Ok(Some(if required { TRUE } else { FALSE }));
        }
        _ => {}
    }
    let key = ctx.text(name);
    let storage = ctx
        .dict_storage(dictionary)
        .expect("staticmethod dictionary");
    if let Some((_, value)) = ctx
        .dict_entries(storage)
        .unwrap()
        .iter()
        .find(|(stored, _)| ctx.equal(*stored, key))
    {
        return Ok(Some(*value));
    }
    if matches!(name, "__annotations__" | "__annotate__") {
        match ctx.lookup_attr(callable, name) {
            Ok(value) => {
                ctx.put_item(storage, key, value)?;
                return Ok(Some(value));
            }
            Err(error) if error.kind == "AttributeError" => {}
            Err(error) => return Err(error),
        }
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
            "attribute '__isabstractmethod__' of 'staticmethod' objects is not writable",
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
            let Value::StaticMethod { dictionary, .. } = ctx.heap.get_mut(wrapper) else {
                unreachable!()
            };
            *dictionary = value;
            Ok(())
        }
        _ => {
            let storage = ctx
                .dict_storage(dictionary)
                .expect("staticmethod dictionary");
            if let Some(value) = value {
                let key = ctx.text(name);
                ctx.put_item(storage, key, value)
            } else {
                ctx.delete_dict_attribute(wrapper, storage, name)
            }
        }
    }
}
