//! Lexical reads for lazy function and class annotation providers.

use std::collections::{BTreeMap, BTreeSet};

use ruff_python_ast::{self as ast, visitor::Visitor};

use super::{ClassBindings, ClosureNames, FunctionScope, class_metadata::Uses};

#[derive(Clone, Copy, Default)]
pub(super) struct Symbol {
    pub(super) local: bool,
    // A captured target keeps its cell while its display is executing.
    pub(super) display_cell: bool,
}

#[derive(Clone, Default)]
pub(in crate::frontend) struct ParameterReads {
    pub(super) symbols: BTreeMap<String, Symbol>,
    pub(super) nested: BTreeSet<String>,
}

impl ParameterReads {
    pub(super) fn read(&mut self, name: &str) {
        self.symbols.entry(name.to_owned()).or_default().local = false;
    }

    pub(super) fn direct_reads(&self) -> impl Iterator<Item = &String> {
        self.symbols
            .iter()
            .filter_map(|(name, symbol)| (!symbol.local).then_some(name))
    }

    pub(super) fn nested_reads(&self) -> impl Iterator<Item = &String> {
        self.nested
            .iter()
            .filter(|name| !self.symbols.get(*name).is_some_and(|symbol| symbol.local))
    }

    pub(super) fn remove_locals(&mut self) {
        self.nested
            .retain(|name| !self.symbols.get(name).is_some_and(|symbol| symbol.local));
    }

    pub(super) fn enclosing(mut self, class: Option<&ClassBindings>) -> BTreeSet<String> {
        if let Some(class) = class {
            self.symbols
                .retain(|name, _| !class.locals.contains(name) && !class.globals.contains(name));
        }
        self.nested.extend(
            self.symbols
                .into_iter()
                .filter_map(|(name, symbol)| (!symbol.local).then_some(name)),
        );
        self.nested
    }

    pub(in crate::frontend) fn provider(
        mut self,
        class: Option<&ClassBindings>,
        bound: impl Fn(&str) -> bool,
    ) -> FunctionScope {
        let mut names = ClosureNames::default();
        for (name, symbol) in &self.symbols {
            if symbol.display_cell {
                names.cell_reads.insert(name.clone());
            }
            if symbol.local {
                let nested = self.nested.remove(name);
                // Children resolved before inlining still bypass this slot when global.
                if bound(name) {
                    names.locals.insert(name.clone());
                    if nested {
                        names.cell_reads.insert(name.clone());
                    }
                }
            }
        }
        names.reads = self.resolve(class);
        names.finish_scope();
        FunctionScope {
            names,
            captures_class: false,
        }
    }

    pub(in crate::frontend) fn resolve(
        mut self,
        class: Option<&ClassBindings>,
    ) -> BTreeSet<String> {
        if class.is_some()
            && (self
                .symbols
                .get("super")
                .is_some_and(|symbol| !symbol.local)
                || self.nested.contains("super"))
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
