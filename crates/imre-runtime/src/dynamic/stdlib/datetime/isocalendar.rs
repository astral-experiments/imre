//! The private tuple subtype owned by each initialization of the C datetime module.

use super::{Context, Error, Id, Result, Value, calendar};
use crate::dynamic::{builtins, weakref};

pub(in crate::dynamic) const CLASS: &str = "datetime.IsoCalendarDate";
const NAME: &str = "IsoCalendarDate";
const MODULE_REF: &str = "_datetime.current_module_ref";
const FIELDS: [&str; 3] = ["year", "week", "weekday"];
const DOC: &str = "The result of date.isocalendar() or datetime.isocalendar()\n\nThis object may be accessed either as a tuple of\n  ((year, week, weekday)\nor via the object attributes as named in the above tuple.";

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id) -> Result<()> {
    let scope = ctx.scope("datetime", None);
    for (key, text) in [
        ("__module__", "datetime"),
        ("__qualname__", NAME),
        ("__doc__", DOC),
    ] {
        let value = ctx.text(text);
        ctx.bind(scope, key, value);
    }
    let tuple = ctx.builtin("tuple");
    let owner = ctx.allocate_class(NAME.into(), scope, vec![tuple])?;
    let Value::Class { heap_type, .. } = ctx.heap.get_mut(owner) else {
        unreachable!()
    };
    *heap_type = Some(CLASS);
    let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    let namespace = frame.class_namespace.as_mut().unwrap();
    namespace.slot_names = Some([].into());
    namespace.defining_module = Some(module);
    for name in [
        "__new__",
        "__repr__",
        "__reduce__",
        "year",
        "week",
        "weekday",
    ] {
        let method = ctx.alloc(Value::HeapTypeMethod { owner, name });
        let member = if name == "__new__" {
            ctx.alloc(Value::Method {
                callable: method,
                receiver: owner,
                explicit: false,
            })
        } else if FIELDS.contains(&name) {
            ctx.alloc(Value::GetSetDescriptor {
                get: method,
                set: None,
                owner: CLASS,
                name,
            })
        } else {
            method
        };
        ctx.bind(scope, name, member);
    }

    let Value::Module { module_state, .. } = ctx.heap.get_mut(module) else {
        unreachable!()
    };
    *module_state = Some(owner);
    let weak = weakref::call(ctx, weakref::REFERENCE_TYPE, &[module], &[])?;
    ctx.builtins.insert(MODULE_REF.into(), weak);
    Ok(())
}

fn current_class(ctx: &mut Context) -> Result<Id> {
    let current = ctx.builtins.get(MODULE_REF).and_then(|&id| {
        if let Value::Weak(reference) = ctx.heap.get(id) {
            reference.target
        } else {
            None
        }
    });
    let module = match current {
        Some(module) => module,
        None => ctx.module("_datetime")?,
    };
    match *ctx.heap.get(module) {
        Value::Module {
            module_state: Some(class),
            ..
        } if ctx.heap.get(class).heap_type() == Some(CLASS) => Ok(class),
        _ => Err(Error::new(
            "ImportError",
            "_datetime module has no calendar state",
        )),
    }
}

fn record(ctx: &mut Context, owner: Id, parts: [i32; 3]) -> Id {
    let values = parts.map(|i| ctx.int(i64::from(i)));
    let tuple = ctx.alloc(Value::Tuple(values.into()));
    let record = ctx.instance(owner, None);
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(record) else {
        unreachable!()
    };
    *scalar = Some(tuple);
    record
}

pub(super) fn from_date(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (&this, rest) = args.split_first().ok_or_else(|| {
        Error::new(
            "TypeError",
            "unbound method date.isocalendar() needs an argument",
        )
    })?;
    let date = calendar(ctx, this, "date").ok_or_else(|| {
        Error::new("TypeError", format!("descriptor 'isocalendar' for 'datetime.date' objects doesn't apply to a '{}' object", super::type_name(ctx, this)))
    })?;
    no_arguments("date.isocalendar", rest, kwargs)?;
    let owner = current_class(ctx)?;
    Ok(record(ctx, owner, date.iso_week_fields()))
}

fn no_arguments(qualified: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{qualified}() takes no keyword arguments"),
        ));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{qualified}() takes no arguments ({} given)", args.len()),
        ));
    }
    Ok(())
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&receiver, args) = args.split_first().ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("unbound method {NAME}.{name}() needs an argument"),
        )
    })?;
    if name == "__new__" {
        let (&class, args) = args.split_first().ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("{CLASS}.__new__(): not enough arguments"),
            )
        })?;
        if class != owner {
            return Err(ctx.invalid_new_subtype(CLASS, class));
        }
        let [year, week, day] = builtins::bind_builtin_arguments(NAME, args, kwargs, FIELDS, 3)?;
        let fields = [
            super::c_int(ctx, year.unwrap())?,
            super::c_int(ctx, week.unwrap())?,
            super::c_int(ctx, day.unwrap())?,
        ];
        return Ok(record(ctx, owner, fields));
    }
    ctx.check_heap_method_receiver(owner, name, receiver)?;
    no_arguments(&format!("{NAME}.{name}"), args, kwargs)?;
    let Value::Tuple(parts) = ctx.heap.get(ctx.scalar_value(receiver)) else {
        unreachable!()
    };
    if let Some(i) = FIELDS.iter().position(|field| *field == name) {
        return Ok(parts[i]);
    }
    if name == "__repr__" {
        let [year, week, weekday] =
            [parts[0], parts[1], parts[2]].map(|part| ctx.as_int(part).expect("C integer"));
        return Ok(ctx.text(format!(
            "{CLASS}(year={year}, week={week}, weekday={weekday})"
        )));
    }
    debug_assert_eq!(name, "__reduce__");
    let values = parts.clone();
    let tuple = ctx.alloc(Value::Tuple(values));
    let args = ctx.alloc(Value::Tuple(vec![tuple]));
    let constructor = ctx.builtin("tuple");
    Ok(ctx.alloc(Value::Tuple(vec![constructor, args])))
}
