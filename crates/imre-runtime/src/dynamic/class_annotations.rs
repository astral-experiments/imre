//! Class-wide annotation providers, following CPython's type descriptors.
use super::{Context, Error, Id, NONE, Result, Value};

impl Context {
    /// Keep declaration inputs alive across type creation, but remove the
    /// scratch field map before __set_name__ can inspect the class namespace.
    pub(super) fn prepare_typeddict_namespace(
        &mut self,
        scope: Id,
        scratch: Id,
    ) -> Result<(Id, Id)> {
        let fields = self.scope_name(scope, "__annotations__").unwrap_or(NONE);
        let thunks = self.enable_class_annotation_provider(scope);
        self.heap.keep(fields);
        self.heap.keep(thunks);
        let key = self.heap.code_name("__annotations__");
        self.remove_binding(scope, "__annotations__", key)?;
        if thunks != NONE {
            let factory = self
                .module_value("typing", "_typeddict_body_annotator")
                .ok_or_else(|| Error::new("RuntimeError", "missing TypedDict body annotator"))?;
            let annotate = self.call(factory, vec![thunks], Vec::new())?;
            self.bind_local(scope, "__annotate_func__", annotate);
        }
        if fields == NONE {
            return Ok((self.dict(Vec::new()), NONE));
        }
        Ok((fields, if fields == scratch { thunks } else { NONE }))
    }

    pub(super) fn enable_class_annotation_provider(&mut self, scope: Id) -> Id {
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        let namespace = frame.class_namespace.get_or_insert_with(Default::default);
        namespace.provider_annotations = true;
        namespace.annotation_thunks.take().unwrap_or(NONE)
    }

    pub(super) fn has_class_annotation_provider(&self, scope: Id) -> bool {
        self.scope_data(scope)
            .class_namespace
            .as_ref()
            .is_some_and(|ns| ns.provider_annotations)
    }

    pub(super) fn class_annotation_attr(&mut self, class: Id, scope: Id, name: &str) -> Result<Id> {
        let key = if name == "__annotate__" {
            "__annotate_func__"
        } else {
            "__annotations_cache__"
        };
        if let Some(value) = self
            .own_class_attr(class, name)
            .or_else(|| self.own_class_attr(class, key))
        {
            return self.bind_attribute(value, NONE, class);
        }
        let value = if name == "__annotate__" {
            NONE
        } else {
            let provider = self.lookup_attr(class, "__annotate__")?;
            if self.callable(provider) {
                let format = self.int(1);
                let value = self.call(provider, vec![format], Vec::new())?;
                if self.dict_storage(value).is_none() {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "__annotate__ returned non-dict of type '{}'",
                            self.type_name(value)
                        ),
                    ));
                }
                value
            } else {
                self.dict(Vec::new())
            }
        };
        self.bind_local(scope, key, value);
        Ok(value)
    }

    pub(super) fn update_class_annotation_attr(
        &mut self,
        scope: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        if name == "__annotate__" {
            let provider = value
                .ok_or_else(|| Error::new("TypeError", "cannot delete __annotate__ attribute"))?;
            if provider != NONE && !self.callable(provider) {
                return Err(Error::new(
                    "TypeError",
                    "__annotate__ must be callable or None",
                ));
            }
            self.bind_local(scope, "__annotate_func__", provider);
            if provider != NONE {
                let key = self.heap.code_name("__annotations_cache__");
                self.remove_binding(scope, "__annotations_cache__", key)?;
            }
            return Ok(());
        }
        let explicit = self.scope_name(scope, name).is_some();
        let storage = if explicit {
            name
        } else {
            "__annotations_cache__"
        };
        let key = self.text(storage);
        if let Some(value) = value {
            self.bind_local(scope, storage, value);
        } else if !self.remove_binding(scope, storage, key)? {
            return Err(Error::new("AttributeError", "__annotations__"));
        }
        if explicit {
            let key = self.text("__annotations_cache__");
            self.remove_binding(scope, "__annotations_cache__", key)?;
        }
        for name in ["__annotate__", "__annotate_func__"] {
            let key = self.heap.code_name(name);
            self.remove_binding(scope, name, key)?;
        }
        Ok(())
    }
}
