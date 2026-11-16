//! Metadata owned by native source functions and copied by decorators.

use super::Context;
use super::value::{Error, Id, NONE, Result, Value};

#[derive(Clone, Copy)]
pub(super) struct FunctionFields {
    pub name: Id,
    pub qualname: Id,
    pub module: Id,
    pub doc: Id,
    pub annotate: Id,
    pub(super) annotations: Option<Id>,
    pub(super) type_params: Id,
    /// The writable namespace, or `NONE` until it is first exposed.
    pub(super) dict: Id,
}

impl FunctionFields {
    pub fn new(
        ctx: &mut Context,
        scope: Id,
        module: Id,
        name: Id,
        qualname: Id,
        doc: Id,
        annotate: Id,
    ) -> Self {
        Self {
            name,
            qualname,
            doc,
            annotate,
            module,
            annotations: None,
            type_params: ctx.alloc(Value::Tuple(Vec::new())),
            dict: if ctx.scope_data(scope).parent.is_none()
                || matches!(ctx.heap.get(name), Value::Str(name) if name == "<genexpr>")
            {
                NONE
            } else {
                ctx.dict(Vec::new())
            },
        }
    }

    pub fn children(self, visit: &mut impl super::value::VisitReferences) {
        visit.push(self.name);
        visit.push(self.qualname);
        visit.push(self.module);
        visit.push(self.doc);
        visit.push(self.annotate);
        visit.push(self.type_params);
        if self.dict != NONE {
            visit.push(self.dict);
        }
        visit.extend(self.annotations);
    }
}

impl Context {
    pub(super) fn root_module_name(&self, root: Id) -> Id {
        self.scope_name(root, "__name__").unwrap_or(NONE)
    }

    /// Materialize the dictionary only when Python exposes or writes it.
    fn function_dict(&mut self, fun: Id, dict: Id) -> Id {
        if dict != NONE {
            return dict;
        }
        let dict = self.dict(Vec::new());
        let Value::Function { fields, .. } = self.heap.get_mut(fun) else {
            unreachable!()
        };
        fields.dict = dict;
        dict
    }

    pub(super) fn function_attr(
        &mut self,
        fun: Id,
        fields: FunctionFields,
        name: &str,
    ) -> Result<Option<Id>> {
        let value = match name {
            "__name__" => fields.name,
            "__qualname__" => fields.qualname,
            "__module__" => fields.module,
            "__doc__" => fields.doc,
            "__annotate__" => fields.annotate,
            "__type_params__" => fields.type_params,
            "__dict__" => self.function_dict(fun, fields.dict),
            "__defaults__" | "__kwdefaults__" => {
                let Value::Function {
                    defaults,
                    kwdefaults,
                    ..
                } = *self.heap.get(fun)
                else {
                    unreachable!()
                };
                if name == "__defaults__" {
                    defaults
                } else {
                    kwdefaults
                }
            }
            "__annotations__" => {
                if let Some(value) = fields.annotations {
                    return Ok(Some(value));
                }
                let annotations = if fields.annotate == NONE {
                    self.dict(Vec::new())
                } else {
                    let format = self.int(1);
                    let value = self.call(fields.annotate, vec![format], Vec::new())?;
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
                };
                let Value::Function { fields, .. } = self.heap.get_mut(fun) else {
                    unreachable!()
                };
                fields.annotations = Some(annotations);
                annotations
            }
            _ => {
                let entries = self.dict_entries(fields.dict).unwrap_or_default();
                return Ok(entries.iter().find_map(|&(key, value)| {
                    matches!(self.heap.get(key), Value::Str(s) if s == name).then_some(value)
                }));
            }
        };
        Ok(Some(value))
    }

    pub(super) fn delete_function_attr(
        &mut self,
        fun: Id,
        fields: FunctionFields,
        name: &str,
    ) -> Result<()> {
        match name {
            "__globals__" | "__builtins__" | "__closure__" => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
            "__dict__" => Err(Error::new("TypeError", "cannot delete __dict__")),
            "__annotate__" => Err(Error::new("TypeError", "__annotate__ cannot be deleted")),
            "__code__" => Err(Error::new(
                "TypeError",
                "__code__ must be set to a code object",
            )),
            "__module__" | "__doc__" | "__annotations__" | "__name__" | "__qualname__"
            | "__type_params__" | "__defaults__" | "__kwdefaults__" => {
                self.set_function_attr(fun, name, NONE)
            }
            _ => {
                let Some(storage) = self.dict_storage(fields.dict) else {
                    return Err(self.missing_attribute(fun, name));
                };
                self.delete_dict_attribute(fun, storage, name)
            }
        }
    }

    pub(super) fn set_function_attr(&mut self, fun: Id, name: &str, value: Id) -> Result<()> {
        if matches!(name, "__globals__" | "__builtins__" | "__closure__") {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        let object = self.heap.get(value);
        if name == "__code__" {
            let Value::Code(new) = object else {
                return Err(Error::new(
                    "TypeError",
                    "__code__ must be set to a code object",
                ));
            };
            let Value::Function { code, .. } = *self.heap.get(fun) else {
                unreachable!()
            };
            if value != code
                && !(new.definition.module_definition
                    && self.code_object(code).definition.module_definition)
            {
                return Err(Error::new(
                    "NotImplementedError",
                    "replacing nested function code requires closure layout",
                ));
            }
            let Value::Function { code, .. } = self.heap.get_mut(fun) else {
                unreachable!()
            };
            *code = value;
            return Ok(());
        }
        let invalid = match name {
            "__defaults__" if value != NONE && !matches!(object, Value::Tuple(_)) => {
                Some("__defaults__ must be set to a tuple object".into())
            }
            "__kwdefaults__" if value != NONE && self.dict_storage(value).is_none() => {
                Some("__kwdefaults__ must be set to a dict object".into())
            }
            "__name__" | "__qualname__" if !matches!(object, Value::Str(_)) => {
                Some(format!("{name} must be set to a string object"))
            }
            "__dict__" if self.dict_storage(value).is_none() => Some(format!(
                "__dict__ must be set to a dictionary, not a '{}'",
                self.type_name(value)
            )),
            "__annotations__" if value != NONE && self.dict_storage(value).is_none() => {
                Some("__annotations__ must be set to a dict object".into())
            }
            "__annotate__" if value != NONE && !self.callable(value) => {
                Some("__annotate__ must be callable or None".into())
            }
            "__type_params__" if !matches!(object, Value::Tuple(_)) => {
                Some("__type_params__ must be set to a tuple".into())
            }
            _ => None,
        };
        if let Some(reason) = invalid {
            return Err(Error::new("TypeError", reason));
        }
        if name == "__defaults__" || name == "__kwdefaults__" {
            let Value::Function {
                defaults,
                kwdefaults,
                ..
            } = self.heap.get_mut(fun)
            else {
                unreachable!()
            };
            if name == "__defaults__" {
                *defaults = value;
            } else {
                *kwdefaults = value;
            }
            return Ok(());
        }
        let Value::Function { fields, .. } = self.heap.get_mut(fun) else {
            unreachable!()
        };
        match name {
            "__name__" => fields.name = value,
            "__qualname__" => fields.qualname = value,
            "__module__" => fields.module = value,
            "__doc__" => fields.doc = value,
            "__type_params__" => fields.type_params = value,
            "__dict__" => fields.dict = value,
            "__annotate__" => {
                fields.annotate = value;
                if value != NONE {
                    fields.annotations = None;
                }
            }
            "__annotations__" => {
                fields.annotate = NONE;
                fields.annotations = (value != NONE).then_some(value);
            }
            _ => {
                let current = fields.dict;
                let dict = self.function_dict(fun, current);
                let storage = self.dict_storage(dict).expect("function namespace");
                let key = self.text(name);
                self.put_item(storage, key, value)?;
            }
        }
        Ok(())
    }
}
