//! Shallow sizes in Imre's boxed heap, excluding objects referenced by Python handles.
use super::value::Heap;
use super::{Context, Error, Id, Result, Value};

pub(super) fn descriptor(owner: &str) -> Option<&'static str> {
    Some(match owner {
        "object" => "object.__sizeof__",
        "dict" => "dict.__sizeof__",
        "list" => "list.__sizeof__",
        "str" => "str.__sizeof__",
        "bytearray" => "bytearray.__sizeof__",
        "set" => "set.__sizeof__",
        "frozenset" => "frozenset.__sizeof__",
        "int" => "int.__sizeof__",
        _ => return None,
    })
}
impl Context {
    /// A native constructor can store slots in the attribute map even when a subclass
    /// shadows their descriptors, and its capacity can survive deletion.
    fn has_declared_storage_slots(&self, class: Id) -> bool {
        self.find_in_mro(class, |base| {
            let Value::Class { scope, .. } = self.heap.get(base) else {
                return None;
            };
            let layout = self.scope_data(*scope).class_namespace.as_ref()?;
            let slots = layout.slot_names.as_ref()?;
            slots
                .iter()
                .any(|name| !matches!(name.as_str(), "__dict__" | "__weakref__"))
                .then_some(())
        })
        .is_some()
    }

    /// An instance's addressable attribute dictionary is a separate Python object.
    fn object_body_size(&self, obj: Id) -> Result<usize> {
        let extra = match self.heap.get(obj) {
            Value::Instance {
                class,
                fields,
                member_values,
                weak_reference,
                ..
            } => {
                // Some native layouts store slots in the same opaque IndexMap as ordinary
                // attributes. Its two independently reserved buffers are unavailable.
                if fields.capacity() != 0
                    && (!self.class_instances_have_dict(*class)
                        || self.has_declared_storage_slots(*class))
                {
                    return Err(self.unsupported_size(obj));
                }
                member_values
                    .as_ref()
                    .map_or(0, |slots| slots.allocation_size())
                    + weak_reference.as_deref().map_or(0, size_of_val)
            }
            _ => 0,
        };
        Ok(size_of::<Value>() + extra)
    }
    fn unsupported_size(&self, obj: Id) -> Error {
        Error::new(
            "TypeError",
            format!(
                "size of the native '{}' layout is unavailable",
                self.type_name(obj)
            ),
        )
    }

    pub(super) fn dictionary_size(&self, obj: Id, storage: Id, ordered: bool) -> Result<usize> {
        let entries = match self.heap.get(storage) {
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => entries,
            _ => unreachable!("checked dictionary storage"),
        };
        // new_dict gives a subtype its own intrinsic table, unlike a shared scalar payload.
        let backing = if storage == obj {
            0
        } else {
            size_of::<Value>() + Heap::size_overhead()
        };
        Ok(self.object_body_size(obj)?
            + backing
            + entries.table_bytes()
            + if ordered { entries.order_bytes() } else { 0 })
    }
    pub(super) fn native_sizeof(
        &mut self,
        owner: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some((&obj, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("unbound method {owner}.__sizeof__() needs an argument"),
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{owner}.__sizeof__() takes no keyword arguments"),
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{owner}.__sizeof__() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let payload = self.scalar_value(obj);
        let unsupported = || self.unsupported_size(obj);
        let wrong = || {
            Error::new(
                "TypeError",
                format!(
                    "descriptor '__sizeof__' for '{owner}' objects doesn't apply to a '{}' object",
                    self.type_name(obj)
                ),
            )
        };
        // A scalar constructor may retain another externally owned Python object.
        if payload != obj {
            return Err(unsupported());
        }
        let body = self.object_body_size(obj)?;
        let size = match (owner, self.heap.get(obj)) {
            ("dict", _) => {
                self.dictionary_size(obj, self.dict_storage(obj).ok_or_else(wrong)?, false)?
            }
            ("list", Value::List(values)) => body + values.capacity() * size_of::<Id>(),
            ("str", Value::Str(text)) => body + text.capacity(),
            ("str", Value::SurrogateStr(text)) => body + text.capacity() * size_of::<u32>(),
            ("bytearray", Value::ByteArray(bytes)) => body + bytes.capacity(),
            ("set" | "frozenset", Value::Set { entries, frozen })
                if *frozen == (owner == "frozenset") =>
            {
                body + entries.table_bytes()
            }
            ("int", Value::Int(number)) if number.as_small().is_none() => return Err(unsupported()),
            ("int", Value::Int(_) | Value::Bool(_)) => body,
            ("object", Value::Bytes(bytes)) => body + bytes.capacity(),
            ("object", Value::Tuple(items)) => body + items.capacity() * size_of::<Id>(),
            (
                "object",
                Value::Instance {
                    class,
                    property: None,
                    cycle: None,
                    memory_stream: None,
                    ..
                },
            ) if self.namedtuple_fields(*class).is_none() => body,
            (
                "object",
                Value::None
                | Value::Bool(_)
                | Value::Float(_)
                | Value::Int(_)
                | Value::Complex(_)
                | Value::Slice(_)
                | Value::Ellipsis
                | Value::NotImplemented
                | Value::Weak(_)
                | Value::MappingProxy(_)
                | Value::DictView { .. }
                | Value::Dict(_)
                | Value::DefaultDict { .. }
                | Value::Set { .. }
                | Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::List(_)
                | Value::ByteArray(_),
            ) => size_of::<Value>(),
            ("object", _) => return Err(unsupported()),
            _ => return Err(wrong()),
        };
        Ok(self.alloc(Value::Int(crate::Int::from_usize(size))))
    }
}
