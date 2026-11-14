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
    pub(crate) frame_reads: BTreeSet<String>,
    pub(crate) needs_class_cell: bool,
    pub(crate) needs_dict_cell: bool,
    pub(crate) needs_conditional_cell: bool,
    pub(crate) static_attributes: BTreeSet<String>,
}

pub(super) fn site(class: &ast::StmtClassDef) -> TextSize {
    class
        .decorator_list
        .first()
        .map_or(class.start(), Ranged::start)
}

impl ClassScope {
    pub(crate) fn write_signature(&self, signature: &mut Value) {
        signature["needs_class_cell"] = json!(self.needs_class_cell);
        signature["needs_dict_cell"] = json!(self.needs_dict_cell);
        signature["static_attributes"] = json!(self.static_attributes);
        signature["class_frame_reads"] = json!(self.frame_reads);
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
        let (needs_class_cell, needs_dict_cell, needs_conditional_cell) =
            imre_ast::class_cell::required_with_conditional(body, string_annotations);
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
            frame_reads: BTreeSet::new(),
            needs_class_cell,
            needs_dict_cell,
            needs_conditional_cell,
            static_attributes: imre_ast::class_attributes::static_attributes(body, names),
        }
    }
}
