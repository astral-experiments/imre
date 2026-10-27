//! Accessors and descriptor slots shared by the builtin property and its Python subclasses.

use super::{Context, Error, Id, NONE, Result, Value};

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__get__",
    "__set__",
    "__delete__",
    "__set_name__",
    "setter",
    "deleter",
];

#[derive(Clone, Copy, Default)]
pub(super) struct Property {
    get: Id,
    set: Id,
    delete: Id,
    pub(super) doc: Id,
    pub(super) name: Option<Id>,
    getter_doc: bool,
}

impl Value {
    pub(super) fn property(&self) -> Option<&Property> {
        match self {
            Self::Property(property) => Some(property),
            Self::Instance { property, .. } => property.as_deref(),
            _ => None,
        }
    }

    pub(super) fn property_mut(&mut self) -> Option<&mut Property> {
        match self {
            Self::Property(property) => Some(property),
            Self::Instance { property, .. } => property.as_deref_mut(),
            _ => None,
        }
    }
}

impl Property {
    pub(super) fn children(self, visit: &mut super::value::MarkQueue<'_>) {
        visit.extend([self.get, self.set, self.delete, self.doc]);
        visit.extend(self.name);
    }

    fn missing(&self, ctx: &Context, receiver: Id, accessor: &str) -> Error {
        let name = self
            .name
            .map_or_else(String::new, |name| format!(" {}", ctx.repr(name)));
        let class = match ctx.heap.get(receiver) {
            Value::Instance { class, .. } => Some(*class),
            Value::Raised(error) => error.class,
            _ => None,
        };
        let owner = if let Some(class) = class
            && let Value::Class { scope, .. } = ctx.heap.get(class)
            && let Some(&name) = ctx.scope_data(*scope).names.get("__qualname__")
            && let Value::Str(name) = ctx.heap.get(name)
        {
            name
        } else {
            ctx.type_name(receiver)
        };
        Error::new(
            "AttributeError",
            format!("property{name} of '{owner}' object has no {accessor}"),
        )
    }

    pub(super) fn read(self, ctx: &mut Context, object: Id) -> Result<Id> {
        if self.get == NONE {
            return Err(self.missing(ctx, object, "getter"));
        }
        ctx.call(self.get, vec![object], Vec::new())
    }

    pub(super) fn write(self, ctx: &mut Context, object: Id, value: Id) -> Result<()> {
        if self.set == NONE {
            return Err(self.missing(ctx, object, "setter"));
        }
        ctx.call(self.set, vec![object, value], Vec::new())?;
        Ok(())
    }

    pub(super) fn remove(self, ctx: &mut Context, object: Id) -> Result<()> {
        if self.delete == NONE {
            return Err(self.missing(ctx, object, "deleter"));
        }
        ctx.call(self.delete, vec![object], Vec::new())?;
        Ok(())
    }

    pub(super) fn attribute(self, ctx: &mut Context, name: &str) -> Result<Option<Id>> {
        Ok(Some(match name {
            "fget" => self.get,
            "fset" => self.set,
            "fdel" => self.delete,
            "__doc__" => self.doc,
            "__name__" => match self.name {
                Some(value) => value,
                None => match ctx.lookup_attr(self.get, "__name__") {
                    Err(error) if error.kind == "AttributeError" => {
                        return Err(Error::new(
                            "AttributeError",
                            "'property' object has no attribute '__name__'",
                        ));
                    }
                    result => result?,
                },
            },
            _ => return Ok(None),
        }))
    }
}

fn getter_doc(ctx: &mut Context, getter: Id) -> Result<Id> {
    match ctx.lookup_attr(getter, "__doc__") {
        Err(error) if error.kind == "AttributeError" => Ok(NONE),
        result => result,
    }
}

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let receiver = ctx.alloc(Value::Property(Property::default()));
    initialize(ctx, receiver, args, kwargs)?;
    Ok(receiver)
}

fn initialize(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let fields = super::builtins::bind_method_arguments(
        "property",
        args,
        kwargs,
        ["fget", "fset", "fdel", "doc"],
        4,
    )?;
    let [get, set, delete, doc] = fields.map(|value| value.unwrap_or(NONE));
    *ctx.heap.get_mut(receiver).property_mut().unwrap() = Property {
        get,
        set,
        delete,
        ..Property::default()
    };
    let auto_doc = doc == NONE && get != NONE;
    let doc = if auto_doc { getter_doc(ctx, get)? } else { doc };
    let property_doc_from_getter = auto_doc && doc != NONE;
    let exact = matches!(ctx.heap.get(receiver), Value::Property(_));
    let property = ctx.heap.get_mut(receiver).property_mut().unwrap();
    property.getter_doc = property_doc_from_getter;
    if exact {
        property.doc = doc;
    } else if let Err(error) = ctx.set_attr(receiver, "__doc__", doc)
        && (property_doc_from_getter || error.kind != "AttributeError")
    {
        return Err(error);
    }
    Ok(NONE)
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let wrapper = !matches!(name, "setter" | "deleter" | "__set_name__");
    let Some((&receiver, args)) = args.split_first() else {
        let msg = if wrapper {
            format!("descriptor '{name}' of 'property' object needs an argument")
        } else {
            format!("unbound method property.{name}() needs an argument")
        };
        return Err(Error::new("TypeError", msg));
    };
    let Some(property) = ctx.heap.get(receiver).property().copied() else {
        let actual = ctx.type_name(receiver);
        let msg = if wrapper {
            format!("descriptor '{name}' requires a 'property' object but received a '{actual}'")
        } else {
            format!(
                "descriptor '{name}' for 'property' objects doesn't apply to a '{actual}' object"
            )
        };
        return Err(Error::new("TypeError", msg));
    };
    if name == "__init__" {
        return initialize(ctx, receiver, args, kwargs);
    }

    if !kwargs.is_empty() {
        let owner = if wrapper { "wrapper " } else { "property." };
        return Err(Error::new(
            "TypeError",
            format!("{owner}{name}() takes no keyword arguments"),
        ));
    }
    let count = args.len();
    match (name, args) {
        ("setter" | "deleter", [accessor]) => {
            copy_accessor(ctx, receiver, property, name, *accessor)
        }
        ("setter" | "deleter", _) => Err(Error::new(
            "TypeError",
            format!("property.{name}() takes exactly one argument ({count} given)"),
        )),
        ("__get__", [object] | [object, _]) => {
            if *object != NONE {
                property.read(ctx, *object)
            } else if args.get(1).is_some_and(|owner| *owner != NONE) {
                Ok(receiver)
            } else {
                Err(Error::new("TypeError", "__get__(None, None) is invalid"))
            }
        }
        ("__set__", [object, value]) => {
            property.write(ctx, *object, *value)?;
            Ok(NONE)
        }
        ("__delete__", [object]) => {
            property.remove(ctx, *object)?;
            Ok(NONE)
        }
        ("__set_name__", [_, value]) => {
            ctx.heap.get_mut(receiver).property_mut().unwrap().name = Some(*value);
            Ok(NONE)
        }
        ("__get__", _) if count == 0 => Err(Error::new(
            "TypeError",
            "__get__ expected at least 1 argument, got 0",
        )),
        ("__get__", _) => Err(Error::new(
            "TypeError",
            format!("__get__ expected at most 2 arguments, got {count}"),
        )),
        ("__set__", _) => Err(Error::new(
            "TypeError",
            format!("__set__ expected 2 arguments, got {count}"),
        )),
        ("__delete__", _) => Err(Error::new(
            "TypeError",
            format!("expected 1 argument, got {count}"),
        )),
        ("__set_name__", _) => Err(Error::new(
            "TypeError",
            format!("__set_name__() takes 2 positional arguments but {count} were given"),
        )),
        _ => unreachable!(),
    }
}

/// Reconstruct the originating property subtype with the replaced accessor.
fn copy_accessor(
    ctx: &mut Context,
    receiver: Id,
    mut property: Property,
    name: &str,
    accessor: Id,
) -> Result<Id> {
    if accessor != NONE {
        match name {
            "setter" => property.set = accessor,
            "deleter" => property.delete = accessor,
            _ => unreachable!(),
        }
    }
    let class = ctx.exact_type(receiver)?;
    let doc = if property.getter_doc {
        NONE
    } else {
        property.doc
    };
    let new = ctx.call(
        class,
        vec![property.get, property.set, property.delete, doc],
        Vec::new(),
    )?;
    if let Some(copy) = ctx.heap.get_mut(new).property_mut() {
        copy.name = property.name;
    }
    Ok(new)
}
