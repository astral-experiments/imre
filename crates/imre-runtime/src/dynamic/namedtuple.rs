//! Runtime-created tuple subclasses use the same positional layout as `typing.NamedTuple`.

use std::collections::HashSet;

use super::{Context, Error, Id, NONE, Result, Value, builtins};
use ruff_python_stdlib::{identifiers::is_identifier, keyword::is_keyword};

pub(super) const GETTER: &str = "collections._tuplegetter";
pub(super) const DOC_REPR: &str = "<member '__doc__' of 'collections._tuplegetter' objects>";
pub(super) const GETTER_METHODS: &[&str] = &["__get__", "__set__", "__delete__", "__reduce__"];

/// Return the shared native type attributes that describe the tuple field descriptor.
pub(super) fn class_attribute(ctx: &mut Context, class: Id, name: &str) -> Option<Id> {
    if name == "__base__" {
        return Some(ctx.builtin("object"));
    }
    if !matches!(name, "__doc__" | "__bases__" | "__mro__") {
        return None;
    }
    let key = format!("{GETTER}.{name}");
    if let Some(value) = ctx.builtins.get(&key) {
        return Some(*value);
    }
    let value = if name == "__doc__" {
        Value::TupleGetterDoc
    } else {
        let object = ctx.builtin("object");
        Value::Tuple(if name == "__bases__" {
            vec![object]
        } else {
            vec![class, object]
        })
    };
    let value = ctx.alloc(value);
    ctx.builtins.insert(key, value);
    Some(value)
}

/// Read the writable document through its member descriptor.
pub(super) fn document(ctx: &Context, receiver: Id) -> Result<Id> {
    if let Value::TupleGetter { doc, .. } = ctx.heap.get(receiver) {
        Ok(doc.unwrap_or(NONE))
    } else {
        Err(doc_receiver_error(ctx, receiver))
    }
}

fn doc_receiver_error(ctx: &Context, receiver: Id) -> Error {
    Error::new(
        "TypeError",
        format!(
            "descriptor '__doc__' for '{GETTER}' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        ),
    )
}

/// Replace or delete the traced document before releasing its previous weak edge.
pub(super) fn set_document(ctx: &mut Context, receiver: Id, value: Option<Id>) -> Result<Id> {
    let Value::TupleGetter { doc, .. } = ctx.heap.get_mut(receiver) else {
        return Err(doc_receiver_error(ctx, receiver));
    };
    if let Some(old) = std::mem::replace(doc, value) {
        ctx.heap.weak_edge_removed(old);
    }
    Ok(NONE)
}

/// Implement direct calls to the tuple getter's document member descriptor.
pub(super) fn doc_method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&member, rest)) = args.split_first() else {
        return Err(Error::new("TypeError", "missing member descriptor"));
    };
    if !matches!(ctx.heap.get(member), Value::TupleGetterDoc) || !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
    match (name, rest) {
        ("__get__", [obj] | [obj, _]) => {
            if *obj != NONE {
                document(ctx, *obj)
            } else if rest.get(1).copied().unwrap_or(NONE) == NONE {
                Err(Error::new("TypeError", "__get__(None, None) is invalid"))
            } else {
                Ok(member)
            }
        }
        ("__set__", [obj, value]) => set_document(ctx, *obj, Some(*value)),
        ("__delete__", [obj]) => set_document(ctx, *obj, None),
        ("__reduce__", []) => {
            let get = ctx.builtin("getattr");
            let class = ctx.builtin(GETTER);
            let doc = ctx.text("__doc__");
            let args = ctx.alloc(Value::Tuple(vec![class, doc]));
            Ok(ctx.alloc(Value::Tuple(vec![get, args])))
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}

/// Bind read-only tuple positions in a new class namespace.
pub(super) fn bind_fields(ctx: &mut Context, scope: Id, names: &[String]) {
    for (index, field) in names.iter().enumerate() {
        let doc = ctx.text(format!("Alias for field number {index}"));
        let getter = ctx.alloc(Value::TupleGetter {
            index: index as i64,
            doc: Some(doc),
        });
        ctx.bind(scope, field, getter);
    }
}

/// Construct the same descriptor as the collections module's field factory.
pub(super) fn new_getter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [raw, doc] = args else {
        return Err(Error::new(
            "TypeError",
            format!("_tuplegetter expected 2 arguments, got {}", args.len()),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_tuplegetter() takes no keyword arguments",
        ));
    }
    let index = ctx
        .index_integer(*raw)?
        .to_i64()
        .filter(|index| isize::try_from(*index).is_ok())
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    Ok(ctx.alloc(Value::TupleGetter {
        index,
        doc: Some(*doc),
    }))
}

/// Read the tuple payload without invoking the receiver's indexing override.
pub(super) fn read_field(ctx: &Context, index: i64, receiver: Id) -> Result<Id> {
    let object = ctx.scalar_value(receiver);
    let pos = usize::try_from(index).ok();
    let item = match ctx.heap.get(object) {
        Value::Tuple(items) | Value::NamedTuple { values: items, .. } => {
            pos.and_then(|i| items.get(i)).copied()
        }
        Value::Instance { class, fields, .. }
            if let Value::Class {
                namedtuple_fields: Some(names),
                ..
            } = ctx.heap.get(*class) =>
        {
            pos.and_then(|i| names.get(i))
                .and_then(|name| fields.get(name))
                .copied()
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor for index '{index}' for tuple subclasses doesn't apply to '{}' object",
                    ctx.type_name(receiver)
                ),
            ));
        }
    };
    item.ok_or_else(|| Error::new("IndexError", "tuple index out of range"))
}

/// Dispatch explicit descriptor protocol calls with the receiver retained across allocation.
pub(super) fn getter_method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new("TypeError", "missing _tuplegetter receiver"));
    };
    let Value::TupleGetter { index, doc } = *ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a _tuplegetter receiver"));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    match (name, rest) {
        ("__get__", [instance] | [instance, _]) => {
            if *instance != NONE {
                read_field(ctx, index, *instance)
            } else if rest.get(1).copied().unwrap_or(NONE) == NONE {
                Err(Error::new("TypeError", "__get__(None, None) is invalid"))
            } else {
                Ok(receiver)
            }
        }
        ("__set__", [_, _]) => Err(Error::new("AttributeError", "can't set attribute")),
        ("__delete__", [_]) => Err(Error::new("AttributeError", "can't delete attribute")),
        ("__reduce__", []) => {
            let doc = doc
                .ok_or_else(|| Error::new("SystemError", "NULL object passed to Py_BuildValue"))?;
            let class = ctx.builtin(GETTER);
            let index = ctx.int(index);
            let values = ctx.alloc(Value::Tuple(vec![index, doc]));
            Ok(ctx.alloc(Value::Tuple(vec![class, values])))
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}

/// Consume the complete iterable before inspecting its length, including errors from its iterator.
fn collect(ctx: &mut Context, source: Id) -> Result<Vec<Id>> {
    let iterator = ctx.iterator(source)?;
    ctx.heap.keep(iterator);
    let mut items = Vec::new();
    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.keep(item);
        items.push(item);
    }
    Ok(items)
}

fn string(ctx: &mut Context, object: Id) -> Result<String> {
    let constructor = ctx.builtin("str");
    let value = ctx.call(constructor, vec![object], Vec::new())?;
    Ok(ctx.str_value(value)?.to_owned())
}

fn validate(name: &str) -> Result<()> {
    if is_identifier(name) {
        return Ok(());
    }
    let reason = if is_keyword(name) {
        "cannot be a keyword"
    } else {
        "must be valid identifiers"
    };
    let repr = crate::objects::exceptions::string_repr(name);
    Err(Error::new(
        "ValueError",
        format!("Type names and field names {reason}: {repr}"),
    ))
}

/// Allocate a fresh class for each call, even when its typename and fields match an earlier declaration.
pub(super) fn factory(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [typename, fields, rename, defaults, module] = builtins::bind_method_arguments(
        "namedtuple",
        args,
        kwargs,
        ["typename", "field_names", "rename", "defaults", "module"],
        2,
    )?;
    let (Some(typename), Some(fields)) = (typename, fields) else {
        return Err(Error::new(
            "TypeError",
            "namedtuple() requires a typename and field_names",
        ));
    };
    let mut names: Vec<String> = if let Ok(text) = ctx.str_value(ctx.scalar_value(fields)) {
        text.split(|c| c == ',' || crate::objects::python_whitespace(c))
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        let iterator = ctx.iterator(fields)?;
        ctx.heap.keep(iterator);
        let mut names = Vec::new();
        while let Some(item) = ctx.next_item(iterator)? {
            ctx.heap.keep(item);
            names.push(string(ctx, item)?);
        }
        names
    };
    let typename = string(ctx, typename)?;
    let rename = ctx.optional_truth(rename, false)?;
    if rename {
        let mut seen = HashSet::new();
        for (i, name) in names.iter_mut().enumerate() {
            let duplicate = !seen.insert(name.clone());
            if duplicate || name.starts_with('_') || !is_identifier(name) {
                *name = format!("_{i}");
            }
        }
    }
    validate(&typename)?;
    for name in &names {
        validate(name)?;
    }
    let mut unique = HashSet::new();
    for name in &names {
        let message = if name.starts_with('_') && !rename {
            "Field names cannot start with an underscore"
        } else if !unique.insert(name) {
            "Encountered duplicate field name"
        } else {
            continue;
        };
        return Err(Error::new(
            "ValueError",
            format!(
                "{message}: {}",
                crate::objects::exceptions::string_repr(name)
            ),
        ));
    }
    let has_defaults = defaults.is_some_and(|id| id != NONE);
    let defaults = match defaults {
        Some(id) if id != NONE => collect(ctx, id)?,
        _ => Vec::new(),
    };
    if defaults.len() > names.len() {
        return Err(Error::new(
            "TypeError",
            "Got more default values than field names",
        ));
    }
    let module = module
        .filter(|&id| id != NONE)
        .unwrap_or_else(|| ctx.heap.caller_module());
    let module_name = ctx.str_value(module).unwrap_or("__main__").to_owned();
    let scope = ctx.scope(&module_name, None);
    ctx.heap.keep(scope);
    let name = ctx.text(&typename);
    ctx.bind(scope, "__name__", name);
    ctx.bind(scope, "__qualname__", name);
    ctx.bind(scope, "__module__", module);
    let field_ids: Vec<_> = names.iter().map(|n| ctx.text(n)).collect();
    let offset = field_ids.len() - defaults.len();
    let field_defaults = ctx.dict(
        field_ids[offset..]
            .iter()
            .zip(&defaults)
            .map(|(&k, &v)| (k, v))
            .collect(),
    );
    ctx.bind(scope, "_field_defaults", field_defaults);
    let fields = ctx.alloc(Value::Tuple(field_ids));
    ctx.bind(scope, "_fields", fields);
    ctx.bind(scope, "__match_args__", fields);
    bind_fields(ctx, scope, &names);
    bind_repr(ctx, scope, "collections", name, fields)?;
    let slots = ctx.alloc(Value::Tuple(Vec::new()));
    ctx.bind(scope, "__slots__", slots);
    let doc = ctx.text(format!("{typename}({})", names.join(", ")));
    ctx.bind(scope, "__doc__", doc);
    let make = ctx.builtin("_collections._make");
    let make = ctx.alloc(Value::ClassMethod(make));
    ctx.bind(scope, "_make", make);
    let defaults = has_defaults.then(|| ctx.alloc(Value::Tuple(defaults)));
    bind_new(ctx, scope, &typename, &names, defaults)?;
    let tuple = ctx.builtin("tuple");
    let class = ctx.allocate_class(typename, scope, vec![tuple])?;
    let Value::Class {
        namedtuple_fields, ..
    } = ctx.heap.get_mut(class)
    else {
        unreachable!()
    };
    *namedtuple_fields = Some(names.into());
    Ok(class)
}

/// Format the actual tuple payload against the declared field names.
pub(super) fn bind_repr(
    ctx: &mut Context,
    scope: Id,
    owner: &str,
    name: Id,
    fields: Id,
) -> Result<()> {
    let module = ctx.module(owner)?;
    let factory = ctx.lookup_attr(module, "_namedtuple_repr")?;
    let repr = ctx.call(factory, vec![name, fields], Vec::new())?;
    ctx.bind(scope, "__repr__", repr);
    Ok(())
}

/// Assemble constructor source and documentation without aborting on an oversized field list.
fn text_parts(parts: &[&str]) -> Result<String> {
    let mut output = String::new();
    for part in parts {
        super::operators::append_rendered_text(&mut output, part)?;
    }
    Ok(output)
}

/// Give the generated function the declared parameter names and ordinary Python argument binding.
pub(super) fn bind_new(
    ctx: &mut Context,
    scope: Id,
    typename: &str,
    names: &[String],
    defaults: Option<Id>,
) -> Result<()> {
    let mut fields = String::new();
    for name in names {
        if !fields.is_empty() {
            super::operators::append_rendered_text(&mut fields, ", ")?;
        }
        super::operators::append_rendered_text(&mut fields, name)?;
    }
    if names.len() == 1 {
        super::operators::append_rendered_text(&mut fields, ",")?;
    }
    let new = ctx.builtin("tuple.__new__");
    let allocate = ctx.text("_tuple_new");
    let module = ctx.text("__name__");
    let module_name = ctx.text(text_parts(&["namedtuple_", typename])?);
    let builtins = ctx.text("__builtins__");
    let empty = ctx.dict(Vec::new());
    let globals = ctx.dict(vec![
        (allocate, new),
        (module, module_name),
        (builtins, empty),
    ]);
    let source = ctx.text(text_parts(&[
        "def __new__(_cls, ",
        &fields,
        "):\n    return _tuple_new(_cls, (",
        &fields,
        "))",
    ])?);
    ctx.exec_source(source, globals, None, None)?;
    let new = ctx
        .dict_named_item(globals, "__new__")
        .expect("generated function");
    ctx.heap.keep(new);
    let key = ctx.text("__new__");
    ctx.delete_item(globals, key)?;
    let Value::Function { code, .. } = *ctx.heap.get(new) else {
        unreachable!()
    };
    let lambda = ctx.text("<lambda>");
    let Value::Code(code) = ctx.heap.get_mut(code) else {
        unreachable!()
    };
    code.name = lambda;
    code.qualname = lambda;
    let qualname = ctx.text(text_parts(&[typename, ".__new__"])?);
    let doc = ctx.text(text_parts(&[
        "Create new instance of ",
        typename,
        "(",
        &fields,
        ")",
    ])?);
    ctx.set_attr(new, "__qualname__", qualname)?;
    ctx.set_attr(new, "__doc__", doc)?;
    if let Some(defaults) = defaults {
        ctx.set_attr(new, "__defaults__", defaults)?;
    }
    let descriptor = super::static_method::construct(ctx, new)?;
    ctx.bind(scope, "__new__", descriptor);
    Ok(())
}

/// Allocate the requested tuple subtype from the iterable before invoking any initializer.
pub(super) fn allocate(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&class, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "tuple.__new__(): not enough arguments",
        ));
    };
    let tuple = ctx.builtin("tuple");
    let names = ctx.namedtuple_fields(class);
    if names.is_none() && !ctx.subclass(class, tuple) {
        return Err(ctx.invalid_new_subtype("tuple", class));
    }
    let storage = builtins::call(ctx, "tuple", args, kwargs)?;
    if class == tuple {
        return Ok(storage);
    }
    let Value::Tuple(items) = ctx.heap.get(storage) else {
        unreachable!()
    };
    let (fields, scalar) = if let Some(names) = names {
        let fields = names.iter().cloned().zip(items.iter().copied()).collect();
        (fields, (names.len() != items.len()).then_some(storage))
    } else {
        (Default::default(), Some(storage))
    };
    Ok(ctx.alloc(Value::Instance {
        class,
        fields,
        scalar,
        namespace: None,
        dictionary: None,
        property: None,
        weak_reference: None,
        memory_stream: None,
    }))
}

/// Build the descriptor's actual receiver class from the iterable's positional values.
pub(super) fn make(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(class), Some(iterable)] =
        builtins::bind_method_arguments("_make", args, kwargs, ["cls", "iterable"], 2)?
    else {
        return Err(Error::new("TypeError", "_make() requires an iterable"));
    };
    let names = ctx
        .namedtuple_fields(class)
        .ok_or_else(|| Error::new("TypeError", "expected a named tuple class"))?;
    let values = collect(ctx, iterable)?;
    if names.len() != values.len() {
        return Err(Error::new(
            "TypeError",
            format!("Expected {} arguments, got {}", names.len(), values.len()),
        ));
    }
    let fields = names.iter().cloned().zip(values).collect();
    Ok(ctx.alloc(Value::Instance {
        class,
        namespace: None,
        fields,
        dictionary: None,
        scalar: None,
        property: None,
        weak_reference: None,
        memory_stream: None,
    }))
}
