//! Abstract method declarations on the bundled ABC and its subclasses.

use std::collections::BTreeSet;

use super::Context;
use super::value::{Error, Id, NONE, Result, Value};

impl Context {
    fn module_class(&self, module: &str, name: &str) -> Option<Id> {
        let Value::Module { scope, .. } = self.heap.get(*self.modules.get(module)?) else {
            return None;
        };
        self.scope_name(*scope, name)
    }

    pub(super) fn structural_context_manager(&self, class: Id, target: Id) -> bool {
        matches!(self.heap.get(target), Value::Class { name, .. } if name == "AbstractContextManager")
            && self.module_class("contextlib", "AbstractContextManager") == Some(target)
            && ["__enter__", "__exit__"].iter().all(|name| {
                self.class_attr(class, name)
                    .is_some_and(|value| value != NONE)
            })
    }

    pub(super) fn init_abstract_class(&mut self, class: Id, name: &str, scope: Id) -> Result<()> {
        if !super::mapping_abc::is_abc_class(self, class)
            && !(name == "ABC" && self.scope_data(scope).module == "abc")
            && !self
                .module_class("abc", "ABC")
                .is_some_and(|abc| self.subclass(class, abc))
        {
            return Ok(());
        }
        let mut names: BTreeSet<String> = self.scope_data(scope).names.keys().cloned().collect();
        let (bases, _) = self.scope_data(scope).class_lineage.expect("class bases");
        let Value::Tuple(bases) = self.heap.get(bases) else {
            unreachable!()
        };
        for &base in bases {
            if let Some(methods) = self.own_class_attr(base, "__abstractmethods__")
                && let Value::Set { items, .. } = self.heap.get(methods)
            {
                for &key in items {
                    names.insert(self.str_value(key)?.to_owned());
                }
            }
        }
        let mut items = Vec::new();
        for name in names {
            let Some(value) = self.class_attr(class, &name) else {
                continue;
            };
            match self.lookup_attr(value, "__isabstractmethod__") {
                Ok(value) if self.truth(value)? => items.push(self.text(name)),
                Ok(_) => {}
                Err(err) if err.kind == "AttributeError" => {}
                Err(err) => return Err(err),
            }
        }
        let methods = self.alloc(Value::Set {
            items,
            frozen: true,
        });
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
