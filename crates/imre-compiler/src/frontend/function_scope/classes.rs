//! Class namespaces hide their own bindings from direct reads, while methods capture outer cells.
use std::collections::BTreeSet;

use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextSize};
use serde_json::{Value, json};

use super::{ClassBindings, ClosureNames, FunctionScope};

pub(crate) struct ClassScope {
    pub(crate) enclosing_class: Option<ruff_text_size::TextSize>,
    pub(crate) bindings: ClassBindings,
    pub(crate) namespace: FunctionScope,
    pub(crate) cells: imre_ast::class_cell::ClassCells,
    pub(crate) static_attributes: BTreeSet<String>,
}

pub(super) fn site(class: &ast::StmtClassDef) -> TextSize {
    class
        .decorator_list
        .first()
        .map_or(class.start(), Ranged::start)
}

impl ClassScope {
    pub(crate) fn write_signature(&self, signature: &mut Value, method_scope: bool) {
        signature["needs_class_cell"] = json!(self.cells.class);
        signature["needs_dict_cell"] = json!(self.cells.dict);
        signature["static_attributes"] = json!(self.static_attributes);
        if method_scope {
            // A nested class body can close over its defining class's cell.
            signature["method_class_cell"] =
                json!(self.namespace.names.reads.contains("__class__"));
        }
        if !self.bindings.globals.is_empty() {
            signature["class_globals"] = json!(self.bindings.globals);
        }
        self.namespace.write_signature(signature);
    }

    pub(crate) fn prepare(
        class: &ast::StmtClassDef,
        names: &imre_ast::Names,
        enclosing_class: Option<ruff_text_size::TextSize>,
    ) -> Self {
        let body = &class.body;
        let string_annotations = names.string_annotations;
        let declarations = ClosureNames::class_declarations(body);
        Self {
            enclosing_class,
            bindings: ClassBindings {
                locals: declarations.locals,
                globals: declarations.globals,
            },
            namespace: FunctionScope {
                names: ClosureNames {
                    nonlocals: declarations.nonlocals,
                    ..ClosureNames::default()
                },
                captures_class: false,
            },
            cells: imre_ast::class_cell::ClassCells::of(body, string_annotations),
            static_attributes: BTreeSet::new(),
        }
    }
}
