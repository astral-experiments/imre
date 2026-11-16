//! Python local slots shared by code-object introspection and frame locals.

use std::collections::BTreeSet;

use super::{CellLayout, Context, Error, Id, NONE, Result, Value};

pub(super) fn visible_slot_name(slot: &str) -> &str {
    if slot == imre_ast::annotations::FORMAT_PARAMETER {
        "format"
    } else {
        slot
    }
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
            if !code.definition.closure_supported && !code.definition.module_definition {
                return Err(Error::new(
                    "NotImplementedError",
                    "code lacks closure metadata",
                ));
            }
            if code.freevar_names != NONE {
                return Ok(code.freevar_names);
            }
            let names = code.definition.freevars.clone();
            let names = names
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|name| self.heap.code_name(name))
                .collect();
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
        let parameters = self.parsed_parameters(object)?;
        if !parameters.locals_complete {
            return Err(self.missing_attribute(object, field));
        }
        let mut cells: BTreeSet<_> = match &parameters.cell_layout {
            CellLayout::Complete(names) => names.iter().map(String::as_str).collect(),
            CellLayout::Unspecified => BTreeSet::new(),
            CellLayout::Incomplete => return Err(self.missing_attribute(object, field)),
        };
        // Captured arguments keep their fast-local slots. Cell-only locals
        // follow the argument cells in lexical name order.
        let mut cell_names = Vec::new();
        for arg in parameters.argument_slots() {
            if cells.remove(arg) {
                cell_names.push(self.heap.code_name(arg));
            }
        }
        cell_names.extend(cells.iter().map(|name| self.heap.code_name(name)));
        let local_names = parameters
            .locals
            .iter()
            .filter(|name| !cells.contains(name.as_str()))
            .map(|name| self.heap.code_name(visible_slot_name(name)))
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
