//! Abstract method declarations on the bundled ABC and its subclasses.

use std::collections::BTreeSet;

use super::Context;
use super::value::{Error, FALSE, Id, NONE, NOT_IMPLEMENTED, Result, TRUE, Value};

impl Context {
    /// Look up a value in an already loaded module without invoking its attribute hooks.
    pub(super) fn module_value(&self, module: &str, name: &str) -> Option<Id> {
        let Value::Module { scope, .. } =
            self.heap.get(self.dict_named_item(self.modules, module)?)
        else {
            return None;
        };
        self.scope_name(*scope, name)
    }

    /// Look up raw method declarations in C3 order, without binding a descriptor.
    pub(super) fn abc_check_methods(&mut self, class: Id, methods: &[Id]) -> Result<Id> {
        if !matches!(
            self.builtin_type_name(class),
            Some("type" | "typing._AnyMeta")
        ) {
            self.lookup_attr(class, "__mro__")?;
        }
        let hierarchy = self.lineage(class);
        for &method in methods {
            let name = self.str_value(method)?;
            let declared = hierarchy.iter().find_map(|&base| {
                if let Some(slot) = self.own_class_attr(base, name) {
                    return Some(slot != NONE);
                }
                let Value::Native(kind) = self.heap.get(base) else {
                    return None;
                };
                if name == "__hash__" {
                    // Builtin classes may supply a hash slot directly or inherit it
                    // later in the MRO. Looking up the slot must never hash a value.
                    return match kind.as_str() {
                        "bytearray"
                        | "dict"
                        | "list"
                        | "set"
                        | "cell"
                        | "dict_keys"
                        | "dict_items"
                        | "odict_keys"
                        | "odict_items"
                        | "array.array"
                        | "collections.defaultdict"
                        | "collections.deque"
                        | "_contextvars.Context"
                        | "_contextvars.Token"
                        | "types.SimpleNamespace"
                        | "weakref.ProxyType"
                        | "weakref.CallableProxyType" => Some(false),
                        "object"
                        | "int"
                        | "float"
                        | "complex"
                        | "str"
                        | "bytes"
                        | "tuple"
                        | "frozenset"
                        | "memoryview"
                        | "range"
                        | "slice"
                        | "NoneType"
                        | "mappingproxy"
                        | "types.GenericAlias"
                        | "weakref.ReferenceType" => Some(true),
                        _ => None,
                    };
                }
                if name == "__buffer__" {
                    return matches!(
                        kind.as_str(),
                        "bytes" | "bytearray" | "memoryview" | "array.array"
                    )
                    .then_some(true);
                }
                let present = match kind.as_str() {
                    "list"
                    | "range"
                    | "dict"
                    | "collections.defaultdict"
                    | "collections.deque"
                    | "dict_keys"
                    | "dict_items"
                    | "odict_keys"
                    | "odict_items"
                    | "mappingproxy" => {
                        matches!(
                            name,
                            "__iter__" | "__len__" | "__contains__" | "__reversed__"
                        )
                    }
                    "str" | "bytes" | "bytearray" | "tuple" | "set" | "frozenset" => {
                        matches!(name, "__iter__" | "__len__" | "__contains__")
                    }
                    "dict_values" | "odict_values" => {
                        matches!(name, "__iter__" | "__len__" | "__reversed__")
                    }
                    "memoryview" => matches!(name, "__iter__" | "__len__"),
                    _ => {
                        super::builtins::is_iterator_type(kind)
                            && matches!(name, "__iter__" | "__next__")
                    }
                };
                present.then_some(true)
            });
            if declared != Some(true) {
                return Ok(NOT_IMPLEMENTED);
            }
        }
        Ok(TRUE)
    }

    /// Keep the standard metaclass rooted independently of the mutable module binding.
    pub(super) fn abc_meta(&mut self) -> Id {
        if let Some(&meta) = self.builtins.get("abc.ABCMeta") {
            return meta;
        }
        let scope = self.scope("abc", None);
        let module = self.text("abc");
        self.bind(scope, "__module__", module);
        let qualname = self.text("ABCMeta");
        self.bind(scope, "__qualname__", qualname);
        let callable = self.builtin("abc.ABCMeta.__new__");
        let dictionary = self.dict(Vec::new());
        let new = self.alloc(Value::StaticMethod {
            callable,
            dictionary,
        });
        self.bind(scope, "__new__", new);
        let register = self.builtin("abc.register");
        self.bind(scope, "register", register);
        for name in [
            "__instancecheck__",
            "__subclasscheck__",
            "_abc_registry_clear",
        ] {
            let check = self.builtin(&format!("abc.{name}"));
            self.bind(scope, name, check);
        }
        let base = self.builtin("type");
        let meta = self
            .allocate_class("ABCMeta".into(), scope, vec![base])
            .expect("type has a consistent MRO");
        self.builtins.insert("abc.ABCMeta".into(), meta);
        meta
    }

    pub(super) fn is_abc_metaclass(&self, class: Id) -> bool {
        self.builtins
            .get("abc.ABCMeta")
            .is_some_and(|&abc| self.direct_subclass(class, abc))
    }

    pub(super) fn is_bundled_abc(&self, class: Id) -> bool {
        let Value::Class {
            metaclass,
            ancestors,
            ..
        } = self.heap.get(class)
        else {
            return false;
        };
        metaclass.is_some_and(|meta| self.is_abc_metaclass(meta))
            // Protocol's synthetic layout still inherits ABCMeta's hooks and registry.
            || (metaclass.is_none()
                && self.classes.get("typing.Protocol").is_some_and(|p| ancestors.contains(p)))
    }

    /// Only ABC classes use a Python subclass hook for public instance and subclass checks.
    pub(super) fn has_abc_subclass_hook(&self, class: Id) -> bool {
        self.class_attr(class, "__subclasshook__").is_some()
            && (self.is_bundled_abc(class) || super::mapping_abc::is_abc_class(self, class))
    }

    pub(super) fn abc_hook_result(&mut self, child: Id, parent: Id) -> Result<Option<bool>> {
        let hook = self.lookup_attr(parent, "__subclasshook__")?;
        match self.call(hook, vec![child], Vec::new())? {
            TRUE => Ok(Some(true)),
            FALSE => Ok(Some(false)),
            NOT_IMPLEMENTED => Ok(None),
            _ => Err(Error::new(
                "AssertionError",
                "__subclasshook__ must return either False, True, or NotImplemented",
            )),
        }
    }

    pub(super) fn structural_context_manager(&self, class: Id, target: Id) -> bool {
        matches!(self.heap.get(target), Value::Class { name, .. } if name == "AbstractContextManager")
            && self.module_value("contextlib", "AbstractContextManager") == Some(target)
            && ["__enter__", "__exit__"].iter().all(|name| {
                self.class_attr(class, name)
                    .is_some_and(|value| value != NONE)
            })
    }

    /// Read the marker using Python lookup and truth testing, including descriptor errors.
    pub(super) fn is_abstract_method(&mut self, value: Id) -> Result<bool> {
        self.with_slot_depth("maximum recursion depth exceeded", |ctx| {
            match ctx.lookup_attr(value, "__isabstractmethod__") {
                Ok(marker) => ctx.truth(marker),
                Err(error) if error.kind == "AttributeError" => Ok(false),
                Err(error) => Err(error),
            }
        })
    }

    pub(super) fn init_abstract_class(&mut self, class: Id, scope: Id) -> Result<()> {
        if !super::mapping_abc::is_abc_class(self, class)
            && !self.is_bundled_abc(class)
            && !self
                .classes
                .get("typing.Protocol")
                .is_some_and(|&protocol| self.subclass(class, protocol))
        {
            return Ok(());
        }
        let mut names: BTreeSet<String> = self
            .scope_bindings(scope)
            .map(|(name, _)| name.to_owned())
            .collect();
        let bases = self
            .scope_data(scope)
            .class_namespace
            .as_ref()
            .expect("class bases")
            .bases;
        let Value::Tuple(bases) = self.heap.get(bases) else {
            unreachable!()
        };
        for &base in bases {
            if let Some(methods) = self.own_class_attr(base, "__abstractmethods__")
                && let Value::Set { entries, .. } = self.heap.get(methods)
            {
                for &(key, _) in entries.iter() {
                    names.insert(self.str_value(key)?.to_owned());
                }
            }
        }
        let methods = self.alloc(Value::Set {
            entries: Default::default(),
            frozen: true,
        });
        for name in names {
            let Some(value) = self.class_attr(class, &name) else {
                continue;
            };
            if self.is_abstract_method(value)? {
                let key = self.text(name);
                super::builtins::set_add(self, methods, key)?;
            }
        }
        self.set_abstract_methods(scope, methods)
    }

    pub(super) fn set_abstract_methods(&mut self, scope: Id, methods: Id) -> Result<()> {
        let abstract_class = self.truth(methods)?;
        let Value::Scope(data) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        data.is_abstract = abstract_class;
        self.bind(scope, "__abstractmethods__", methods);
        Ok(())
    }

    pub(super) fn abstract_class_error(&mut self, scope: Id, name: &str) -> Result<Error> {
        let methods = self
            .scope_name(scope, "__abstractmethods__")
            .expect("abstract class methods");
        let values = self.items(methods)?;
        let plural = if values.len() == 1 { "" } else { "s" };
        let mut methods: Vec<_> = values.iter().map(|&id| self.repr(id)).collect();
        methods.sort();
        Ok(Error::new(
            "TypeError",
            format!(
                "Can't instantiate abstract class {name} without an implementation for abstract method{plural} {}",
                methods.join(", ")
            ),
        ))
    }
}
