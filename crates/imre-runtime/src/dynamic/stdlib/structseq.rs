//! Struct sequences with named fields outside their visible tuple.
use super::{Context, Error, Id, NONE, Result, Value, builtins, os, time};

pub(in crate::dynamic) struct Layout {
    pub(in crate::dynamic) class: &'static str,
    pub(super) fields: &'static [&'static str],
    pub(super) extra: &'static [&'static str],
    pub(super) unnamed: usize,
}

pub(in crate::dynamic) fn layout(class: &str) -> Option<&'static Layout> {
    match class {
        os::stat::CLASS => Some(&os::stat::FILE),
        time::STRUCT_TIME => Some(&time::LAYOUT),
        #[cfg(target_os = "linux")]
        os::stat::VFS_CLASS => Some(&os::stat::VFS),
        _ => None,
    }
}

impl Layout {
    pub(in crate::dynamic) fn name(&self) -> &'static str {
        self.class
            .rsplit_once('.')
            .expect("qualified struct sequence")
            .1
    }

    pub(in crate::dynamic) fn methods(&self) -> &'static [&'static str] {
        if matches!(self.class, os::stat::CLASS | time::STRUCT_TIME) {
            &["__repr__", "__reduce__", "__replace__"]
        } else {
            &["__repr__", "__reduce__"]
        }
    }

    pub(in crate::dynamic) fn match_fields(&self) -> &'static [&'static str] {
        &self.fields[..self.fields.len() - self.unnamed]
    }
}

/// Publish the members on the native class itself so vars(), descriptors and
/// ordinary field lookup share the same objects.
pub(in crate::dynamic) fn install_type(ctx: &mut Context, class: Id, layout: &Layout, doc: &str) {
    let mut entries = Vec::new();
    for &name in std::iter::once(&"__new__").chain(layout.methods()) {
        let method = ctx.builtin(&format!("{}.{name}", layout.class));
        entries.push((ctx.heap.code_name(name), method));
    }
    for &field in layout.match_fields().iter().chain(layout.extra) {
        let member = ctx.readonly_member(layout.class, field);
        entries.push((ctx.heap.code_name(field), member));
    }
    let module = layout
        .class
        .rsplit_once('.')
        .expect("qualified struct sequence")
        .0;
    entries.push((ctx.heap.code_name("__module__"), ctx.heap.code_name(module)));
    entries.push((ctx.heap.code_name("__doc__"), ctx.text(doc)));
    for name in ["n_sequence_fields", "n_fields", "n_unnamed_fields"] {
        let n = count_attribute(layout, name).expect("field count");
        entries.push((ctx.heap.code_name(name), ctx.int(n)));
    }
    let fields = layout
        .match_fields()
        .iter()
        .map(|field| ctx.heap.code_name(field))
        .collect();
    let fields = ctx.alloc(Value::Tuple(fields));
    entries.push((ctx.heap.code_name("__match_args__"), fields));
    let namespace = ctx.dict(entries);
    ctx.native_type_dicts.insert(class, namespace);
}

pub(super) fn wrap(ctx: &mut Context, layout: &Layout, values: Vec<Id>, extras: Vec<Id>) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: Some(layout.class),
        names: layout.fields.iter().map(|&name| name.to_owned()).collect(),
        values,
        extras: layout.extra.iter().copied().zip(extras).collect(),
    })
}

pub(in crate::dynamic) fn count_attribute(layout: &Layout, name: &str) -> Option<i64> {
    Some(match name {
        "n_fields" => (layout.fields.len() + layout.extra.len()) as i64,
        "n_sequence_fields" => layout.fields.len() as i64,
        "n_unnamed_fields" => layout.unnamed as i64,
        _ => return None,
    })
}

pub(in crate::dynamic::stdlib) fn construct(
    ctx: &mut Context,
    layout: &Layout,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let class = layout.class;
    let [Some(sequence), attributes] =
        builtins::bind_builtin_arguments("structseq", args, kwargs, ["sequence", "dict"], 1)?
    else {
        unreachable!()
    };
    let mut values = ctx.fast_sequence_values(sequence, "constructor requires a sequence")?;
    let dictionary = attributes
        .map(|value| {
            ctx.dict_storage(value).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!("{class}() takes a dict as second arg, if any"),
                )
            })
        })
        .transpose()?;
    let len = values.len();
    let max = layout.fields.len() + layout.extra.len();
    if !(layout.fields.len()..=max).contains(&len) {
        let (bound, limit) = if len < layout.fields.len() {
            ("least", layout.fields.len())
        } else {
            ("most", max)
        };
        return Err(Error::new(
            "TypeError",
            format!("{class}() takes an at {bound} {limit}-sequence ({len}-sequence given)"),
        ));
    }
    // Python equality during dictionary lookup may mutate the input sequence or trigger GC.
    if dictionary.is_some() {
        ctx.heap
            .reserve_temporaries(values.len() + layout.extra.len())?;
        for &value in &values {
            ctx.heap.keep(value);
        }
    }
    let mut extras = values.split_off(layout.fields.len());
    let positional = extras.len();
    extras.resize(layout.extra.len(), NONE);
    if let Some(dict) = dictionary
        && !ctx
            .dict_entries(dict)
            .expect("dictionary storage")
            .is_empty()
    {
        let mut found = 0;
        for (field, slot) in layout.extra[positional..]
            .iter()
            .zip(&mut extras[positional..])
        {
            let key = ctx.text(*field);
            if let (_, Some(i)) = ctx.find_dictionary_key(dict, key, None)? {
                *slot = ctx.dict_entries(dict).expect("dictionary storage")[i].1;
                ctx.heap.keep(*slot);
                found += 1;
            }
        }
        if ctx.dict_entries(dict).expect("dictionary storage").len() > found {
            return Err(Error::new(
                "TypeError",
                format!("{class}() got duplicate or unexpected field name(s)"),
            ));
        }
    }
    // Integer times remain the tuple elements and supply missing named times in a constructed result.
    if class == os::stat::CLASS {
        for (slot, &fallback) in extras[..3].iter_mut().zip(&values[7..]) {
            if *slot == NONE {
                *slot = fallback;
            }
        }
    }
    Ok(wrap(ctx, layout, values, extras))
}

/// Call a struct sequence method on an instance of its declaring class.
pub(in crate::dynamic::stdlib) fn method(
    ctx: &mut Context,
    layout: &Layout,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let class = layout.class;
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!(
                "unbound method {}.{method}() needs an argument",
                layout.name()
            ),
        ));
    };
    let (values, extras) = match ctx.heap.get(receiver) {
        Value::NamedTuple {
            class: Some(owner),
            values,
            extras,
            ..
        } if *owner == class => (values, extras),
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' for '{class}' objects doesn't apply to a '{}' object",
                    ctx.type_name(receiver)
                ),
            ));
        }
    };
    if method == "__replace__" {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                "__replace__() takes no positional arguments",
            ));
        }
        if layout.unnamed != 0 {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__replace__() is not supported for {class} because it has unnamed field(s)"
                ),
            ));
        }
        let mut values = values.clone();
        let mut extras: Vec<_> = extras.iter().map(|&(_, value)| value).collect();
        let mut unknown = Vec::new();
        for (name, value) in kwargs {
            if let Some(index) = layout.fields.iter().position(|&field| name == field) {
                values[index] = *value;
            } else if let Some(index) = layout.extra.iter().position(|&field| name == field) {
                extras[index] = *value;
            } else {
                unknown.push(ctx.text(name));
            }
        }
        if !unknown.is_empty() {
            let keys = ctx.alloc(Value::List(unknown));
            return Err(Error::new(
                "TypeError",
                format!("Got unexpected field name(s): {}", ctx.repr(keys)),
            ));
        }
        return Ok(wrap(ctx, layout, values, extras));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{}.__reduce__() takes no keyword arguments", layout.name()),
        ));
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.__reduce__() takes no arguments ({} given)",
                layout.name(),
                rest.len()
            ),
        ));
    }

    let extras = extras.clone();
    let sequence = ctx.alloc(Value::Tuple(values.clone()));
    let fields = extras
        .into_iter()
        .map(|(name, value)| (ctx.text(name), value))
        .collect();
    let fields = ctx.dict(fields);
    let args = ctx.alloc(Value::Tuple(vec![sequence, fields]));
    let class = ctx.builtin(class);
    Ok(ctx.alloc(Value::Tuple(vec![class, args])))
}

/// The shared immutable descriptor exposes the named field independently of the sequence index.
pub(in crate::dynamic) fn member(ctx: &mut Context, layout: &Layout, name: &str) -> Option<Id> {
    let &field = layout
        .match_fields()
        .iter()
        .chain(layout.extra)
        .find(|&&field| field == name)?;
    Some(ctx.readonly_member(layout.class, field))
}

pub(in crate::dynamic) fn field_names(layout: &Layout) -> impl Iterator<Item = &'static str> {
    layout.fields.iter().chain(layout.extra).copied()
}
