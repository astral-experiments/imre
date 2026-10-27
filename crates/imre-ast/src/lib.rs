//! Apply Python's lexical class-name mangling before lowering boxed modules.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use ruff_python_ast::name::Name;
use ruff_python_ast::visitor::transformer::{self, Transformer};
use ruff_python_ast::{self as ast, Identifier};
use ruff_python_stdlib::identifiers::is_mangled_private;
use ruff_text_size::{Ranged, TextSize};

#[derive(Default)]
pub struct Names {
    pub string_annotations: bool,
    definitions: HashMap<TextSize, Name>,
    owners: HashMap<TextSize, Name>,
    import_roots: HashSet<TextSize>,
}

impl Names {
    /// Return the class owning the lexical names in a deferred generator suite.
    pub fn suite_owner(&self, start: TextSize) -> Option<&str> {
        self.owners.get(&start).map(Name::as_str)
    }

    /// A private definition keeps its source name for introspection.
    pub fn definition<'a>(&'a self, name: &'a Identifier) -> &'a str {
        self.definitions
            .get(&name.start())
            .map_or(name.as_str(), Name::as_str)
    }

    /// A dotted import binds the top-level module under a possibly private name.
    pub fn imports_root(&self, name: &Identifier) -> bool {
        self.import_roots.contains(&name.start())
    }
}

pub fn lower(body: &mut [ast::Stmt]) -> Names {
    let string_annotations = body.iter().any(|s| {
        matches!(s, ast::Stmt::ImportFrom(i) if i.level == 0
            && i.module.as_ref().is_some_and(|m| m.as_str() == "__future__")
            && i.names.iter().any(|n| n.name.as_str() == "annotations"))
    });
    lower_in(body, "", string_annotations)
}

/// Apply the same lexical names when reparsing a generator's original suite.
pub fn lower_in(body: &mut [ast::Stmt], owner: &str, string_annotations: bool) -> Names {
    let names = RefCell::new(Names {
        string_annotations,
        ..Default::default()
    });
    Mangler {
        owner,
        names: &names,
        string_annotations,
    }
    .visit_body(body);
    names.into_inner()
}

struct Mangler<'a> {
    owner: &'a str,
    names: &'a RefCell<Names>,
    string_annotations: bool,
}

impl Mangler<'_> {
    fn replacement(&self, name: &str) -> Option<Name> {
        let owner = self.owner.trim_start_matches('_');
        (is_mangled_private(name) && !name.contains('.') && !owner.is_empty())
            .then(|| Name::new(format!("_{owner}{name}")))
    }

    fn rename(&self, name: &mut Name) {
        if let Some(replacement) = self.replacement(name.as_str()) {
            *name = replacement;
        }
    }

    fn definition(&self, name: &mut Identifier) {
        if let Some(replacement) = self.replacement(name.as_str()) {
            let original = std::mem::replace(&mut name.id, replacement);
            self.names
                .borrow_mut()
                .definitions
                .insert(name.start(), original);
        }
    }
}

impl Transformer for Mangler<'_> {
    fn visit_stmt(&self, stmt: &mut ast::Stmt) {
        match stmt {
            ast::Stmt::ClassDef(class) => {
                let owner = class.name.id.clone();
                self.definition(&mut class.name);
                for decorator in &mut class.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(params) = &mut class.type_params {
                    self.visit_type_params(params);
                }
                if let Some(arguments) = &mut class.arguments {
                    self.visit_arguments(arguments);
                }
                Mangler {
                    owner: owner.as_str(),
                    ..*self
                }
                .visit_body(&mut class.body);
                return;
            }
            ast::Stmt::FunctionDef(function) => {
                if !self.owner.is_empty()
                    && let Some(first) = function.body.first()
                {
                    self.names
                        .borrow_mut()
                        .owners
                        .insert(first.start(), Name::new(self.owner));
                }
                self.definition(&mut function.name);
            }

            ast::Stmt::Global(global) => {
                for name in &mut global.names {
                    self.rename(&mut name.id);
                }
            }
            ast::Stmt::Nonlocal(nonlocal) => {
                for name in &mut nonlocal.names {
                    self.rename(&mut name.id);
                }
            }
            ast::Stmt::ImportFrom(import) => {
                if let Some(module) = &mut import.module {
                    self.rename(&mut module.id);
                }
            }
            _ => {}
        }
        transformer::walk_stmt(self, stmt);
    }

    fn visit_expr(&self, expr: &mut ast::Expr) {
        match expr {
            ast::Expr::Name(name) => self.rename(&mut name.id),
            ast::Expr::Attribute(attr) => self.rename(&mut attr.attr.id),
            _ => {}
        }
        transformer::walk_expr(self, expr);
    }

    fn visit_annotation(&self, expression: &mut ast::Expr) {
        if !self.string_annotations {
            transformer::walk_annotation(self, expression);
        }
    }

    fn visit_parameter(&self, parameter: &mut ast::Parameter) {
        self.rename(&mut parameter.name.id);
        transformer::walk_parameter(self, parameter);
    }

    fn visit_except_handler(&self, handler: &mut ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(data) = handler;
        if let Some(name) = &mut data.name {
            self.rename(&mut name.id);
        }
        transformer::walk_except_handler(self, handler);
    }

    fn visit_alias(&self, alias: &mut ast::Alias) {
        if let Some(binding) = &mut alias.asname {
            self.rename(&mut binding.id);
        } else if let Some((root, _)) = alias.name.as_str().split_once('.')
            && let Some(binding) = self.replacement(root)
        {
            alias.asname = Some(Identifier::new(binding, alias.name.range()));
            self.names
                .borrow_mut()
                .import_roots
                .insert(alias.name.start());
        }
        self.rename(&mut alias.name.id);
    }
}
