//! Python local slots shared by code-object introspection and frame locals.

use std::collections::BTreeSet;

use indexmap::IndexSet;
use serde_json::Value as Json;

use super::{Context, Error, Id, NONE, Result, Value};

pub(super) fn argument_slots(signature: &Json) -> impl Iterator<Item = &str> {
    ["posonly", "positional", "kwonly"]
        .into_iter()
        .flat_map(|group| signature[group].as_array().into_iter().flatten())
        .filter_map(Json::as_str)
        .chain(
            ["varargs", "kwargs"]
                .into_iter()
                .filter_map(|key| signature[key].as_str()),
        )
}

impl Context {
    /// Register the code attributes whose values the runtime can supply.
    pub(super) fn install_code_members(&mut self, class: Id) {
        let mut entries = Vec::new();
        for field in [
            "co_argcount",
            "co_posonlyargcount",
            "co_kwonlyargcount",
            "co_nlocals",
            "co_flags",
            "co_name",
            "co_qualname",
            "co_filename",
            "co_firstlineno",
        ] {
            let descriptor = self.readonly_member("code", field);
            entries.push((self.heap.code_name(field), descriptor));
        }
        for field in ["co_varnames", "co_cellvars", "co_freevars"] {
            let get = self.builtin(&format!("_code_local.{field}"));
            let descriptor = self.alloc(Value::GetSetDescriptor {
                owner: "code",
                name: field,
                get,
                set: None,
            });
            entries.push((self.heap.code_name(field), descriptor));
        }
        let dictionary = self.dict(entries);
        self.native_type_dicts.insert(class, dictionary);
    }

    pub(super) fn code_local_field(&mut self, object: Id, field: &str) -> Result<Id> {
        if field == "co_freevars" {
            let code = self.code_object(object);
            if !code.closure_supported && !code.module_definition {
                return Err(Error::new(
                    "NotImplementedError",
                    "code lacks closure metadata",
                ));
            }
            if code.freevar_names != NONE {
                return Ok(code.freevar_names);
            }
            let names = code.freevars.clone();
            let names = names.iter().map(|name| self.heap.code_name(name)).collect();
            let tuple = self.alloc(Value::Tuple(names));
            let Value::Code(code) = self.heap.get_mut(object) else {
                unreachable!()
            };
            code.freevar_names = tuple;
            return Ok(tuple);
        }
        let (locals, cells) = self.code_slot_tuples(object, field)?;
        match field {
            "co_varnames" => Ok(locals),
            "co_cellvars" => Ok(cells),
            "co_nlocals" => {
                let Value::Tuple(items) = self.heap.get(locals) else {
                    unreachable!()
                };
                Ok(self.int(items.len() as i64))
            }
            _ => unreachable!("unknown code layout field"),
        }
    }

    fn code_slot_tuples(&mut self, object: Id, field: &str) -> Result<(Id, Id)> {
        let code = self.code_object(object);
        if code.varnames != NONE {
            return Ok((code.varnames, code.cellvars));
        }
        if code.parameters == NONE {
            return Err(self.missing_attribute(object, field));
        }
        let signature: Json = serde_json::from_str(self.str_value(code.parameters)?)
            .map_err(|error| Error::new("SystemError", error.to_string()))?;
        if signature["scope_locals_complete"] != true
            || signature.get("scope_cellvars") == Some(&Json::Null)
        {
            return Err(self.missing_attribute(object, field));
        }
        // Captured arguments keep their fast-local slots. Cell-only locals
        // follow the argument cells in lexical name order.
        let mut cells: BTreeSet<_> = signature["scope_cellvars"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .collect();
        let mut locals = IndexSet::new();
        let mut cell_names = Vec::new();
        for arg in argument_slots(&signature) {
            locals.insert(arg);
            if cells.remove(arg) {
                cell_names.push(self.heap.code_name(arg));
            }
        }
        for local in signature["scope_locals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            if !cells.contains(local) {
                locals.insert(local);
            }
        }
        cell_names.extend(cells.into_iter().map(|name| self.heap.code_name(name)));
        let local_names = locals
            .into_iter()
            .map(|name| self.heap.code_name(name))
            .collect();
        let varnames = self.alloc(Value::Tuple(local_names));
        let cellvars = self.alloc(Value::Tuple(cell_names));
        let Value::Code(code) = self.heap.get_mut(object) else {
            unreachable!()
        };
        code.varnames = varnames;
        code.cellvars = cellvars;
        Ok((varnames, cellvars))
    }
}
