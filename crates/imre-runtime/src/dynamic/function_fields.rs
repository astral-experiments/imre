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
    annotations: Option<Id>,
    type_params: Id,
    dict: Id,
}

impl FunctionFields {
    pub fn new(
        ctx: &mut Context,
        scope: Id,
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
            module: ctx.text(ctx.scope_data(scope).module.clone()),
            annotations: None,
            type_params: ctx.alloc(Value::Tuple(Vec::new())),
            dict: ctx.dict(Vec::new()),
        }
    }

    pub fn children(self, visit: &mut Vec<Id>) {
        visit.extend([
            self.name,
            self.qualname,
            self.module,
            self.doc,
            self.annotate,
            self.type_params,
            self.dict,
        ]);
        visit.extend(self.annotations);
    }
}

impl Context {
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
            "__dict__" => fields.dict,
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
                let entries = self.dict_entries(fields.dict).expect("function namespace");
                return Ok(entries.iter().find_map(|&(key, value)| {
                    matches!(self.heap.get(key), Value::Str(s) if s == name).then_some(value)
                }));
            }
        };
        Ok(Some(value))
    }

    pub(super) fn set_function_attr(&mut self, fun: Id, name: &str, value: Id) -> Result<()> {
        if name == "__globals__" {
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
            if value != code && !(new.module_definition && self.code_object(code).module_definition)
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
                let dict = fields.dict;
                let storage = self.dict_storage(dict).expect("function namespace");
                let key = self.text(name);
                self.put_item(storage, key, value)?;
            }
        }
        Ok(())
    }
}
