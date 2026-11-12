//! Lexical reads for lazy function and class annotation providers.

use std::collections::{BTreeSet, HashMap};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::Visitor;
use ruff_text_size::{Ranged, TextSize};

use super::{ClassBindings, ClosureNames};

#[derive(Default)]
pub(crate) struct AnnotationScopes {
    assignments: HashMap<TextSize, AssignmentReads>,
}

#[derive(Clone)]
pub(in crate::frontend) struct ParameterReads {
    pub(super) direct: BTreeSet<String>,
    pub(super) nested: BTreeSet<String>,
}

impl ParameterReads {
    pub(in crate::frontend) fn resolve(
        mut self,
        class: Option<&ClassBindings>,
    ) -> BTreeSet<String> {
        if let Some(class) = class {
            if self.direct.contains("super") || self.nested.contains("super") {
                self.nested.insert("__class__".into());
            }
            self.direct
                .retain(|name| !class.locals.contains(name) && !class.globals.contains(name));
        }
        self.direct.append(&mut self.nested);
        self.direct
    }
}

struct AssignmentReads {
    reads: BTreeSet<String>,
    incomplete: bool,
}

impl AnnotationScopes {
    pub(super) fn assignment(&mut self, assign: &ast::StmtAnnAssign) {
        if assign.simple {
            let mut names = ClosureNames::default();
            names.visit_expr(&assign.annotation);
            self.assignments.insert(
                assign.start(),
                AssignmentReads {
                    reads: names.reads,
                    incomplete: names.incomplete,
                },
            );
        }
    }

    pub(crate) fn class_reads<'a>(
        &self,
        sites: impl Iterator<Item = &'a TextSize>,
        class: &ClassBindings,
    ) -> (BTreeSet<String>, bool) {
        let mut reads = BTreeSet::new();
        let mut complete = true;
        for site in sites {
            let assignment = &self.assignments[site];
            reads.extend(
                assignment
                    .reads
                    .iter()
                    .filter(|name| !class.locals.contains(*name) && !class.globals.contains(*name))
                    .cloned(),
            );
            complete &= !assignment.incomplete;
        }
        (reads, complete)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClassAnnotationSite {
    pub(crate) site: TextSize,
    pub(crate) index: Option<usize>,
}

#[derive(Default)]
pub(crate) struct ClassAnnotations {
    pub(crate) sites: Vec<ClassAnnotationSite>,
    pub(crate) body: Option<Box<crate::frontend::AnnotationProvider>>,
    pub(crate) reads: BTreeSet<String>,
    pub(crate) incomplete: bool,
}

impl ClassAnnotations {
    /// Compile the lazy provider once its lexical sites and annotation operands are known.
    pub(crate) fn prepare_body(
        &mut self,
        fields: &mut HashMap<TextSize, crate::frontend::DeferredAnnotation>,
        bindings: crate::frontend::RegisteredBindings,
    ) {
        if !self.sites.is_empty() {
            let fields = self.sites.iter().map(|site| {
                (
                    fields.remove(&site.site).expect("checked class annotation"),
                    site.index,
                )
            });
            self.body = Some(Box::new(crate::frontend::DeferredAnnotation::prepare_body(
                fields, bindings,
            )));
        }
    }

    pub(super) fn observe(&mut self, assign: &ast::StmtAnnAssign) {
        if assign.simple && assign.target.is_name_expr() {
            self.sites.push(ClassAnnotationSite {
                site: assign.start(),
                index: None,
            });
        }
    }

    pub(super) fn finish(
        &mut self,
        class: &ast::StmtClassDef,
        scopes: &AnnotationScopes,
        bindings: &ClassBindings,
    ) {
        if self.sites.is_empty() {
            return;
        }
        let (reads, complete) =
            scopes.class_reads(self.sites.iter().map(|entry| &entry.site), bindings);
        self.reads = reads;
        self.incomplete = !complete;
        self.reads.insert("__classdict__".into());
        let mut conditional = 0;
        for entry in &mut self.sites {
            if class
                .body
                .binary_search_by_key(&entry.site, Ranged::start)
                .is_err()
            {
                self.reads.insert("__conditional_annotations__".into());
                entry.index = Some(conditional);
                conditional += 1;
            }
        }
    }
}
