//! Native property descriptors copy their accessor state when a setter is replaced.

use super::{Context, Error, Id, NONE, Result, Value};

#[derive(Clone, Copy)]
pub(super) struct Property {
    get: Id,
    set: Id,
    delete: Id,
    pub(super) doc: Id,
    pub(super) name: Option<Id>,
    getter_doc: bool,
}

impl Property {
    pub(super) fn children(self, visit: &mut Vec<Id>) {
        visit.extend([self.get, self.set, self.delete, self.doc]);
        visit.extend(self.name);
    }

    fn missing(&self, ctx: &Context, receiver: Id, accessor: &str) -> Error {
        let name = self
            .name
            .map_or_else(String::new, |name| format!(" {}", ctx.repr(name)));
        let owner = ctx.type_name(receiver);
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

pub(super) fn construct(ctx: &mut Context, fields: [Option<Id>; 4]) -> Result<Id> {
    let [get, set, delete, doc] = fields.map(|field| field.unwrap_or(NONE));
    let auto_doc = doc == NONE && get != NONE;
    let doc = if auto_doc { getter_doc(ctx, get)? } else { doc };
    Ok(ctx.alloc(Value::Property(Property {
        get,
        set,
        delete,
        doc,
        name: None,
        getter_doc: auto_doc,
    })))
}

pub(super) fn setter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "unbound method property.setter() needs an argument",
        ));
    };
    let Value::Property(mut property) = *ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor 'setter' for 'property' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "property.setter() takes no keyword arguments",
        ));
    }
    let [set] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "property.setter() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    if *set != NONE {
        property.set = *set;
    }
    if property.getter_doc {
        property.doc = getter_doc(ctx, property.get)?;
    }
    Ok(ctx.alloc(Value::Property(property)))
}
