//! Lexical reads for lazy function and class annotation providers.

use std::collections::{BTreeMap, BTreeSet};

use ruff_python_ast::{self as ast, visitor::Visitor};

use super::{ClassBindings, class_metadata::Uses};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Symbol {
    Read,
    Local,
}

#[derive(Clone, Default)]
pub(in crate::frontend) struct ParameterReads {
    pub(super) symbols: BTreeMap<String, Symbol>,
    pub(super) nested: BTreeSet<String>,
}

impl ParameterReads {
    pub(super) fn remove_locals(&mut self) {
        self.nested
            .retain(|name| self.symbols.get(name) != Some(&Symbol::Local));
    }

    pub(super) fn enclosing(mut self, class: Option<&ClassBindings>) -> BTreeSet<String> {
        if let Some(class) = class {
            self.symbols
                .retain(|name, _| !class.locals.contains(name) && !class.globals.contains(name));
        }
        self.nested.extend(
            self.symbols
                .into_iter()
                .filter_map(|(name, symbol)| (symbol == Symbol::Read).then_some(name)),
        );
        self.nested
    }

    pub(in crate::frontend) fn resolve(
        mut self,
        class: Option<&ClassBindings>,
    ) -> BTreeSet<String> {
        if class.is_some()
            && (self.symbols.get("super") == Some(&Symbol::Read) || self.nested.contains("super"))
        {
            self.nested.insert("__class__".into());
        }
        self.enclosing(class)
    }
}

#[derive(Default)]
pub(crate) struct ClassAnnotations<R = BTreeSet<String>> {
    pub(crate) conditional_count: usize,
    pub(crate) body: Option<Box<crate::frontend::AnnotationProvider>>,
    pub(crate) reads: R,
    pub(crate) postponed: bool,
}

impl ClassAnnotations<Uses> {
    pub(in crate::frontend) fn read(&mut self, expr: &ast::Expr) {
        self.reads.visit_expr(expr);
    }

    pub(in crate::frontend) fn resolve(self, bindings: &ClassBindings) -> ClassAnnotations {
        let mut reads = self.reads.reads.resolve(Some(bindings));
        if self.body.is_some() {
            reads.insert("__classdict__".into());
            if self.conditional_count != 0 {
                reads.insert("__conditional_annotations__".into());
            }
        }
        ClassAnnotations {
            reads,
            conditional_count: self.conditional_count,
            body: self.body,
            postponed: self.postponed,
        }
    }
}
