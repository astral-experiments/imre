//! One Python attribute dictionary per local object and native thread.

use std::borrow::Cow;
use std::sync::{Arc, Weak};

use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const CLASS: &str = "_thread._local";

// Names in the two builtin dictionaries, in _thread._local and object MRO order.
const TYPE_NAMES: &[&[&str]] = &[
    &[
        "__new__",
        "__getattribute__",
        "__setattr__",
        "__delattr__",
        "__doc__",
        "__module__",
    ],
    crate::objects::OBJECT_METHODS,
    &["__class__", "__doc__"],
];

pub(super) fn is_method(name: &str) -> bool {
    matches!(name, "__getattribute__" | "__setattr__" | "__delattr__")
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

/// Allocate a local with its real subclass and a distinct dictionary for the creating thread.
pub(super) fn allocate(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&class, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "_thread._local.__new__(): not enough arguments",
        ));
    };
    let base = ctx.builtin(CLASS);
    if !ctx.subclass(class, base) {
        return Err(ctx.invalid_new_subtype(CLASS, class));
    }
    if (!args.is_empty() || !kwargs.is_empty()) && ctx.local_initializer(class).is_none() {
        return Err(Error::new(
            "TypeError",
            "Initialization arguments are not supported",
        ));
    }
    // The normal class call runs the creator's __init__ after __new__ returns.
    let mut local = Dictionaries {
        args: args.to_vec(),
        kwargs: kwargs.to_vec(),
        ..Default::default()
    };
    local.insert(ctx.dict(Vec::new()));
    let storage = ctx.alloc(Value::ThreadLocal(local));
    if class == base {
        return Ok(storage);
    }
    let object = ctx.instance(class, None);
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(object) else {
        unreachable!()
    };
    *scalar = Some(storage);
    Ok(object)
}

/// Apply one of the type's attribute wrappers after validating its receiver and arguments.
pub(super) fn method(
    ctx: &mut Context,
    slot: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&obj, arguments)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{slot}' of '{CLASS}' object needs an argument"),
        ));
    };
    if !ctx.builtin_slot_accepts(obj, CLASS) {
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
pub(super) struct Dictionaries {
    threads: Vec<(Weak<()>, Id)>,
    args: Vec<Id>,
    kwargs: Vec<(String, Id)>,
}

impl Dictionaries {
    fn current(&self) -> Option<Id> {
        THREAD.with(|thread| {
            self.threads
                .iter()
                .find(|(owner, _)| owner.as_ptr() == Arc::as_ptr(thread))
                .map(|(_, dict)| *dict)
        })
    }

    fn insert(&mut self, dict: Id) {
        self.threads.retain(|(owner, _)| owner.strong_count() != 0);
        self.threads.push((THREAD.with(Arc::downgrade), dict));
    }

    pub(super) fn live(&self) -> impl Iterator<Item = Id> {
        self.threads
            .iter()
            .filter(|(owner, _)| owner.strong_count() != 0)
            .map(|(_, dict)| *dict)
            .chain(self.args.iter().copied())
            .chain(self.kwargs.iter().map(|(_, value)| *value))
    }
}

impl Context {
    fn local_initializer(&self, class: Id) -> Option<Id> {
        self.class_attr(class, "__init__").filter(
            |&init| !matches!(self.heap.get(init), Value::Native(name) if name == "object.__init__"),
        )
    }

    fn local_dict(&mut self, obj: Id) -> Result<Id> {
        let storage = self.scalar_value(obj);
        let Value::ThreadLocal(local) = self.heap.get(storage) else {
            unreachable!()
        };
        if let Some(dict) = local.current() {
            return Ok(dict);
        }
        let dict = self.dict(Vec::new());
        let Value::ThreadLocal(local) = self.heap.get_mut(storage) else {
            unreachable!()
        };
        // Recursive access in __init__ must see this thread's in-progress dictionary.
        local.insert(dict);
        if let Some(class) = self.heap.get(obj).instance_class()
            && let Some(init) = self.local_initializer(class)
        {
            let Value::ThreadLocal(local) = self.heap.get(storage) else {
                unreachable!()
            };
            let args = local.args.clone();
            let kwargs = local.kwargs.clone();
            if let Err(error) = self.call_initializer(init, obj, class, args, kwargs) {
                let Value::ThreadLocal(local) = self.heap.get_mut(storage) else {
                    unreachable!()
                };
                local.threads.retain(|&(_, id)| id != dict);
                return Err(error);
            }
        }
        Ok(dict)
    }

    /// Preserve an exception from the lookup separately from an absent key.
    fn local_dict_value(&mut self, dict: Id, key: Id) -> Result<Option<Id>> {
        Ok(self
            .find_dictionary_key(dict, key, None)?
            .1
            .map(|index| self.dict_entries(dict).expect("thread-local dictionary")[index].1))
    }

    pub(super) fn local_attribute(&mut self, obj: Id, name: Name<'_>) -> Result<Id> {
        let dict = self.local_dict(obj)?;
        if name.is_dict(self)? {
            return Ok(dict);
        }
        if let Some(class) = self.heap.get(obj).instance_class() {
            let text = name.text(self)?;
            if self.local_data_attribute(class, &text, false) {
                return self.default_get_attr(obj, &text);
            }
            let key = name.key(self);
            let field = self.local_dict_value(dict, key)?;
            return self.instance_attribute(obj, class, field, &text);
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
        let dict = self.local_dict(obj)?;
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
                format!(
                    "'{}' object attribute {repr} is read-only",
                    self.type_name(obj)
                ),
            ));
        }
        let text = name.text(self)?;
        let key = name.key(self);
        if let Some(class) = self.heap.get(obj).instance_class() {
            if self.local_data_attribute(class, &text, true) {
                return match value {
                    Some(value) => self.default_set_attr(obj, &text, value),
                    None => self.default_delete_attr(obj, &text),
                };
            }
            if text == "__class__" && self.class_attr(class, &text).is_none() {
                if value == Some(class) {
                    return Ok(());
                }
                if value.is_none_or(|value| self.builtin_type_name(value) != Some("type")) {
                    return self.update_base_local_attribute(obj, "__class__", value);
                }
                return Err(Error::new(
                    "NotImplementedError",
                    "reassigning a thread-local subclass is unsupported",
                ));
            }
        } else if self.lookup_builtin_type_name(key, TYPE_NAMES) == Some("__class__") {
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
mod initialization_tests;

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
