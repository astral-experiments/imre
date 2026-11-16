//! Deferred ordinary class declarations and class-wide annotation providers.
use super::{Context, Error, Id, NONE, Result, Value, source_code::CONDITIONAL};

impl Context {
    /// Capture the live class dictionary and the conditional declarations that ran.
    pub(super) fn class_annotation_environment(&mut self, class_scope: Id) -> Id {
        let parent = self.scope_data(class_scope).parent.expect("class parent");
        let dict_cell = self.class_annotation_dict_cell(class_scope);
        let mut names = vec!["__classdict__".to_owned()];
        let mut cells = vec![dict_cell];
        if let Some(cell) = self.scope_data(class_scope).class_cell {
            names.push("__class__".to_owned());
            cells.push(cell);
        }
        if let Some(cell) = self.scope_data(class_scope).conditional_annotation_cell() {
            names.push(CONDITIONAL.to_owned());
            cells.push(cell);
        }
        self.closure_environment(parent, names, &cells)
    }

    /// Mark the declaration as executed and record its field name for TypedDict.
    pub(super) fn record_class_annotation(&mut self, scope: Id, name: Id, index: Id) -> Result<Id> {
        let Some((fields, conditional_cell)) = self
            .scope_data(scope)
            .class_namespace
            .as_ref()
            .and_then(|ns| ns.annotation_scratch)
        else {
            return Err(Error::new(
                "RuntimeError",
                "annotation outside an executing class body",
            ));
        };
        if index != NONE {
            let cell = conditional_cell.expect("conditional class annotation cell");
            let Value::Cell(Some(active)) = *self.heap.get(cell) else {
                return Err(Error::unbound_local(CONDITIONAL));
            };
            self.mark_annotation(active, index)?;
        }
        if fields != NONE {
            self.put_item(fields, name, NONE)?;
        }
        Ok(NONE)
    }

    /// Keep declaration inputs alive across type creation, but remove the
    /// scratch field map before __set_name__ can inspect the class namespace.
    pub(super) fn prepare_typeddict_namespace(
        &mut self,
        scope: Id,
        scratch: Id,
    ) -> Result<(Id, Id)> {
        let fields = self.scope_name(scope, "__annotations__").unwrap_or(NONE);
        let annotate = if fields == scratch || fields == NONE {
            self.scope_name(scope, "__annotate__")
                .or_else(|| self.scope_name(scope, "__annotate_func__"))
                .unwrap_or(NONE)
        } else {
            NONE
        };
        self.heap.keep(fields);
        self.heap.keep(annotate);
        let key = self.heap.code_name("__annotations__");
        self.remove_binding(scope, "__annotations__", key)?;
        self.enable_class_annotation_provider(scope);
        let fields = if fields == NONE {
            self.dict(Vec::new())
        } else {
            fields
        };
        Ok((fields, annotate))
    }

    pub(super) fn enable_class_annotation_provider(&mut self, scope: Id) {
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        let namespace = frame.class_namespace.get_or_insert_with(Default::default);
        namespace.provider_annotations = true;
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
