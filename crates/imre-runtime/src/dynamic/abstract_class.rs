//! Abstract method declarations on the bundled ABC and its subclasses.

use std::collections::BTreeSet;

use super::Context;
use super::value::{Error, FALSE, Id, NONE, NOT_IMPLEMENTED, Result, TRUE, Value};

impl Context {
    /// Look up a value in an already loaded module without invoking its attribute hooks.
    pub(super) fn module_value(&self, module: &str, name: &str) -> Option<Id> {
        let Value::Module { scope, .. } = self.heap.get(*self.modules.get(module)?) else {
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
        for &method in methods {
            let name = self.str_value(method)?;
            let declared = self.find_in_mro(class, |base| {
                if let Some(slot) = self.own_class_attr(base, name) {
                    return Some(slot != NONE);
                }
                let Value::Native(kind) = self.heap.get(base) else {
                    return None;
                };
                let present = match kind.as_str() {
                    "str" | "bytes" | "bytearray" | "list" | "tuple" | "range" | "dict" | "set"
                    | "frozenset" | "dict_keys" | "dict_items" | "mappingproxy" => {
                        matches!(name, "__iter__" | "__len__" | "__contains__")
                    }
                    "memoryview" | "dict_values" => matches!(name, "__iter__" | "__len__"),
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

    /// Recognize the bundled ABC and its nominal subclasses, which inherit its metaclass.
    pub(super) fn is_bundled_abc(&self, class: Id) -> bool {
        self.module_value("abc", "ABC")
            .is_some_and(|abc| self.direct_subclass(class, abc))
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

    pub(super) fn init_abstract_class(&mut self, class: Id, name: &str, scope: Id) -> Result<()> {
        if !super::mapping_abc::is_abc_class(self, class)
            && !(name == "ABC" && self.scope_data(scope).module == "abc")
            && !self.is_bundled_abc(class)
            && !self
                .classes
                .get("typing.Protocol")
                .is_some_and(|&protocol| self.subclass(class, protocol))
        {
            return Ok(());
        }
        let mut names: BTreeSet<String> = self.scope_data(scope).names.keys().cloned().collect();
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
            match self.lookup_attr(value, "__isabstractmethod__") {
                Ok(value) if self.truth(value)? => {
                    let key = self.text(name);
                    super::builtins::set_add(self, methods, key)?;
                }
                Ok(_) => {}
                Err(err) if err.kind == "AttributeError" => {}
                Err(err) => return Err(err),
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
