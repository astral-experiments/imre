//! Class namespaces hide their own bindings from direct reads, while methods capture outer cells.
use std::collections::BTreeSet;

use ruff_python_ast::{self as ast, visitor::Visitor};

use super::{ClassBindings, ClosureNames, FunctionScope, class_metadata};

impl ClassBindings {
    pub(super) fn from_names(names: &ClosureNames) -> Self {
        Self {
            locals: names
                .locals
                .iter()
                .filter(|name| !names.globals.contains(*name) && !names.nonlocals.contains(*name))
                .cloned()
                .collect(),
            globals: names.globals.clone(),
        }
    }
}

pub(crate) struct ClassScope {
    pub(crate) annotations: super::ClassAnnotations,
    pub(crate) bindings: ClassBindings,
    pub(crate) namespace: FunctionScope,
    pub(crate) frame_reads: BTreeSet<String>,
    pub(crate) postponed_annotations: bool,
    pub(crate) needs_class_cell: bool,
    pub(crate) needs_dict_cell: bool,
    pub(crate) static_attributes: BTreeSet<String>,
}

impl ClassScope {
    pub(crate) fn prepare(class: &ast::StmtClassDef, names: &imre_ast::Names) -> Self {
        let body = &class.body;
        let string_annotations = names.string_annotations;
        let mut declarations = ClosureNames::default();
        declarations.visit_body(body);
        let bindings = ClassBindings::from_names(&declarations);
        let (frame_reads, method_reads) =
            class_metadata::capture(&mut declarations, class, string_annotations);
        Self {
            annotations: Default::default(),
            bindings,
            namespace: FunctionScope::new(
                ClosureNames {
                    nonlocals: declarations.nonlocals,
                    incomplete: method_reads.is_none(),
                    reads: method_reads.unwrap_or_default(),
                    ..ClosureNames::default()
                },
                None,
                false,
            ),
            frame_reads,
            postponed_annotations: string_annotations
                && imre_ast::annotations::has_variable_annotation(body),
            needs_class_cell: imre_ast::class_cell::needed(body, string_annotations),
            needs_dict_cell: imre_ast::class_cell::dict_needed(body, string_annotations),
            static_attributes: imre_ast::class_attributes::static_attributes(body, names),
        }
    }
}
