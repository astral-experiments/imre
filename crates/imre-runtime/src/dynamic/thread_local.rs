//! One Python attribute dictionary per local object and native thread.

use std::borrow::Cow;
use std::sync::{Arc, Weak};

use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const CLASS: &str = "_thread._local";

// Names in the two builtin dictionaries, in _thread._local and object MRO order.
const TYPE_NAMES: &[&str] = &[
    "__new__",
    "__getattribute__",
    "__setattr__",
    "__delattr__",
    "__doc__",
    "__module__",
    "__new__",
    "__repr__",
    "__hash__",
    "__str__",
    "__getattribute__",
    "__setattr__",
    "__delattr__",
    "__lt__",
    "__le__",
    "__eq__",
    "__ne__",
    "__gt__",
    "__ge__",
    "__init__",
    "__reduce_ex__",
    "__reduce__",
    "__getstate__",
    "__subclasshook__",
    "__init_subclass__",
    "__format__",
    "__sizeof__",
    "__dir__",
    "__class__",
    "__doc__",
];

pub(super) fn is_method(name: &str) -> bool {
    matches!(name, "__getattribute__" | "__setattr__" | "__delattr__")
}

/// Recognize only the three slot descriptors installed by the local type.
pub(super) fn wrapper_name(symbol: &str) -> Option<&'static str> {
    match symbol {
        "_thread._local.__getattribute__" => Some("__getattribute__"),
        "_thread._local.__setattr__" => Some("__setattr__"),
        "_thread._local.__delattr__" => Some("__delattr__"),
        _ => None,
    }
}

/// Bind the slot to a local instance or return the descriptor on class access.
pub(super) fn bind_wrapper(
    ctx: &mut Context,
    descriptor: Id,
    slot: &str,
    receiver: Id,
) -> Result<Id> {
    if receiver == NONE {
        return Ok(descriptor);
    }
    if !matches!(ctx.heap.get(receiver), Value::ThreadLocal(_)) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{slot}' for '{CLASS}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    }
    Ok(ctx.alloc(Value::Method {
        callable: descriptor,
        receiver,
        explicit: false,
    }))
}

/// Return attributes shared by an unbound slot and its bound method wrapper.
pub(super) fn wrapper_attribute(ctx: &mut Context, slot: &str, attr: &str) -> Option<Id> {
    Some(match attr {
        "__name__" => ctx.text(slot),
        "__qualname__" => ctx.text(format!("_local.{slot}")),
        "__objclass__" => ctx.builtin(CLASS),
        _ => return None,
    })
}

/// Direct slot calls can carry any Python object, including a key in the local dictionary.
#[derive(Clone, Copy)]
pub(super) enum Name<'a> {
    Text(&'a str),
    Key(Id),
}

impl<'a> Name<'a> {
    pub(super) fn from_lookup(text: &'a str, original: Option<Id>) -> Self {
        original.map_or(Self::Text(text), Self::Key)
    }

    fn key(self, ctx: &mut Context) -> Id {
        match self {
            Self::Text(name) => ctx.text(name),
            Self::Key(key) => key,
        }
    }

    fn text(self, ctx: &Context) -> Result<Cow<'a, str>> {
        match self {
            Self::Text(name) => Ok(Cow::Borrowed(name)),
            Self::Key(key) => builtins::attribute_name(ctx, key).map(Cow::Owned),
        }
    }

    fn is_dict(self, ctx: &mut Context) -> Result<bool> {
        match self {
            Self::Text(name) => Ok(name == "__dict__"),
            Self::Key(key) => match ctx.heap.get(key) {
                Value::Str(name) => Ok(name == "__dict__"),
                _ => {
                    let marker = ctx.text("__dict__");
                    let equal = ctx.binary("==", key, marker)?;
                    ctx.truth(equal)
                }
            },
        }
    }
}

/// Implement the descriptor's explicit __get__ with Python's receiver and owner rules.
fn descriptor_get(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&descriptor, arguments)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "descriptor '__get__' of 'wrapper_descriptor' object needs an argument",
        ));
    };
    let slot = match ctx.heap.get(descriptor) {
        Value::Native(symbol) => wrapper_name(symbol),
        _ => None,
    }
    .ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "descriptor '__get__' requires a 'wrapper_descriptor' object but received a '{}'",
                ctx.type_name(descriptor)
            ),
        )
    })?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "wrapper __get__() takes no keyword arguments",
        ));
    }
    match *arguments {
        [] => Err(Error::new(
            "TypeError",
            "__get__ expected at least 1 argument, got 0",
        )),
        [NONE] | [NONE, NONE] => Err(Error::new("TypeError", "__get__(None, None) is invalid")),
        [receiver] | [receiver, _] => bind_wrapper(ctx, descriptor, slot, receiver),
        _ => Err(Error::new(
            "TypeError",
            format!(
                "__get__ expected at most 2 arguments, got {}",
                arguments.len()
            ),
        )),
    }
}

/// Apply one of the type's attribute wrappers after validating its receiver and arguments.
pub(super) fn method(
    ctx: &mut Context,
    slot: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if slot == "wrapper_get" {
        return descriptor_get(ctx, args, kwargs);
    }
    let Some((&obj, arguments)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{slot}' of '{CLASS}' object needs an argument"),
        ));
    };
    if !matches!(ctx.heap.get(obj), Value::ThreadLocal(_)) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{slot}' requires a '{CLASS}' object but received a '{}'",
                ctx.type_name(obj)
            ),
        ));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("wrapper {slot}() takes no keyword arguments"),
        ));
    }
    match (slot, arguments) {
        ("__getattribute__", [key]) => ctx.local_attribute(obj, Name::Key(*key)),
        ("__setattr__", [key, value]) => {
            ctx.update_local_attribute(obj, Name::Key(*key), Some(*value))?;
            Ok(NONE)
        }
        ("__delattr__", [key]) => {
            ctx.update_local_attribute(obj, Name::Key(*key), None)?;
            Ok(NONE)
        }
        _ => {
            let prefix = if slot == "__setattr__" {
                "__setattr__ expected 2 arguments"
            } else {
                "expected 1 argument"
            };
            Err(Error::new(
                "TypeError",
                format!("{prefix}, got {}", arguments.len()),
            ))
        }
    }
}

thread_local! { static THREAD: Arc<()> = Arc::new(()); }

/// A thread owns its token. Once it exits, its dictionary may be collected.
#[derive(Clone, Default)]
pub(super) struct Dictionaries(Vec<(Weak<()>, Id)>);

impl Dictionaries {
    fn current(&self) -> Option<Id> {
        THREAD.with(|thread| {
            self.0
                .iter()
                .find(|(owner, _)| owner.as_ptr() == Arc::as_ptr(thread))
                .map(|(_, dict)| *dict)
        })
    }

    fn insert(&mut self, dict: Id) {
        self.0.retain(|(owner, _)| owner.strong_count() != 0);
        self.0.push((THREAD.with(Arc::downgrade), dict));
    }

    pub(super) fn live(&self) -> impl Iterator<Item = Id> {
        self.0
            .iter()
            .filter(|(owner, _)| owner.strong_count() != 0)
            .map(|(_, dict)| *dict)
    }
}

impl Context {
    fn local_dict(&mut self, obj: Id) -> Id {
        let Value::ThreadLocal(local) = self.heap.get(obj) else {
            unreachable!()
        };
        if let Some(dict) = local.current() {
            return dict;
        }
        let dict = self.dict(Vec::new());
        let Value::ThreadLocal(local) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        local.insert(dict);
        dict
    }

    /// Preserve an exception from the lookup separately from an absent key.
    fn local_dict_value(&mut self, dict: Id, key: Id) -> Result<Option<Id>> {
        Ok(self
            .find_dictionary_key(dict, key, None)?
            .1
            .map(|index| self.dict_entries(dict).expect("thread-local dictionary")[index].1))
    }

    pub(super) fn local_attribute(&mut self, obj: Id, name: Name<'_>) -> Result<Id> {
        let dict = self.local_dict(obj);
        if name.is_dict(self)? {
            return Ok(dict);
        }
        let key = name.key(self);
        if let Some(value) = self.local_dict_value(dict, key)? {
            return Ok(value);
        }
        let text = name.text(self)?;
        let descriptor = self.lookup_builtin_type_name(key, TYPE_NAMES);
        if descriptor == Some("__class__") {
            return Ok(self.builtin(CLASS));
        }
        // A descriptor lookup can run Python and populate the thread dictionary.
        if let Some(value) = self.local_dict_value(dict, key)? {
            return Ok(value);
        }
        match descriptor {
            Some(slot) => self.default_get_attr(obj, slot),
            None => Err(self.missing_attribute(obj, &text)),
        }
    }

    /// The base object wrappers see the class descriptor, but no instance dictionary.
    pub(super) fn update_base_local_attribute(
        &self,
        obj: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        if name != "__class__" {
            return Err(self.missing_attribute(obj, name));
        }
        let message = match value {
            None => "can't delete __class__ attribute".to_owned(),
            Some(value) if self.builtin_type_name(value) != Some("type") => {
                format!(
                    "__class__ must be set to a class, not '{}' object",
                    self.type_name(value)
                )
            }
            Some(_) => {
                "__class__ assignment only supported for mutable types or ModuleType subclasses"
                    .to_owned()
            }
        };
        Err(Error::new("TypeError", message))
    }

    pub(super) fn update_local_attribute(
        &mut self,
        obj: Id,
        name: Name<'_>,
        value: Option<Id>,
    ) -> Result<()> {
        let dict = self.local_dict(obj);
        if name.is_dict(self)? {
            let key = name.key(self);
            let repr = if let Some(hook) = self.bound_slot(key, "__repr__")? {
                let value = self.call(hook, Vec::new(), Vec::new())?;
                self.str_value(value)
                    .map_err(|_| {
                        Error::new(
                            "TypeError",
                            format!(
                                "__repr__ returned non-string (type {})",
                                self.type_name(value)
                            ),
                        )
                    })?
                    .to_owned()
            } else {
                self.repr(self.scalar_value(key))
            };
            return Err(Error::new(
                "AttributeError",
                format!("'_thread._local' object attribute {repr} is read-only"),
            ));
        }
        let text = name.text(self)?;
        let key = name.key(self);
        if self.lookup_builtin_type_name(key, TYPE_NAMES) == Some("__class__") {
            return self.update_base_local_attribute(obj, "__class__", value);
        }
        let result = match value {
            Some(value) => self.put_item(dict, key, value),
            None => self.delete_item(dict, key),
        };
        match result {
            Err(error) if self.is_builtin_error(&error, "KeyError") => {
                Err(self.missing_attribute(obj, &text))
            }
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dictionaries;
    use crate::dynamic::value::{Heap, Location, NONE, Value};
    use std::rc::Rc;
    use std::sync::{Barrier, Mutex};

    #[test]
    fn dictionaries_are_private_and_unrooted_after_thread_exit() {
        let mut heap = Heap::new();
        let mut dictionary = || {
            let payload: Rc<[Vec<String>]> = Rc::from([]);
            let probe = Rc::downgrade(&payload);
            let value = heap.alloc(Value::AttrGetter(payload));
            (heap.alloc(Value::Dict(vec![(NONE, value)].into())), probe)
        };
        let (parent, parent_probe) = dictionary();
        let (worker, worker_probe) = dictionary();
        let local = Mutex::new(Dictionaries::default());
        let ready = Barrier::new(2);
        local.lock().unwrap().insert(parent);
        std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                {
                    let mut local = local.lock().unwrap();
                    assert_eq!(local.current(), None);
                    local.insert(worker);
                }
                ready.wait();
                assert_eq!(local.lock().unwrap().current(), Some(worker));
                ready.wait();
            });
            ready.wait();
            {
                let local = local.lock().unwrap();
                assert_eq!(local.current(), Some(parent));
                assert_eq!(local.live().collect::<Vec<_>>(), [parent, worker]);
            }
            ready.wait();
            handle.join().unwrap();
        });
        let object = heap.alloc(Value::ThreadLocal(local.into_inner().unwrap()));
        // Cross the ordinary statement collection threshold with only the local rooted.
        for _ in 0..4096 {
            heap.alloc(Value::None);
        }
        let location = Location {
            file: "local.py".into(),
            line: 1,
            span: None,
        };
        heap.statement(std::iter::once(object), &[], location, NONE);
        assert!(parent_probe.upgrade().is_some());
        assert!(worker_probe.upgrade().is_none());
    }
}
