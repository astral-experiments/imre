//! Lexical reads for lazy function and class annotation providers.

use std::collections::BTreeSet;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::Visitor;

use super::{ClassBindings, ClosureNames};

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

#[derive(Default)]
pub(crate) struct ClassAnnotations {
    pub(crate) conditional_count: usize,
    pub(crate) body: Option<Box<crate::frontend::AnnotationProvider>>,
    pub(crate) reads: BTreeSet<String>,
    pub(crate) incomplete: bool,
    pub(crate) postponed: bool,
}

impl ClassAnnotations {
    pub(in crate::frontend) fn read(&mut self, expr: &ast::Expr) {
        let mut names = ClosureNames {
            reads: std::mem::take(&mut self.reads),
            ..ClosureNames::default()
        };
        names.visit_expr(expr);
        self.reads = names.reads;
        self.incomplete |= names.incomplete;
    }

    pub(in crate::frontend) fn resolve(&mut self, bindings: &ClassBindings) {
        if self.body.is_some() {
            self.reads
                .retain(|name| !bindings.locals.contains(name) && !bindings.globals.contains(name));
            self.reads.insert("__classdict__".into());
            if self.conditional_count != 0 {
                self.reads.insert("__conditional_annotations__".into());
            }
        }
    }
}
