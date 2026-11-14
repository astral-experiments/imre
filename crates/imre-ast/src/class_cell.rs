//! Identify the implicit cells needed before a class body executes.
use crate::comprehension::{Output, Plan};
use ast::visitor::{self, Visitor};
use ruff_python_ast as ast;

const CLASS: u8 = 1;
const DICT: u8 = 2;
const CONDITIONAL: u8 = 4;

fn cell(name: &str) -> u8 {
    match name {
        "__class__" => CLASS,
        "__classdict__" => DICT,
        "__conditional_annotations__" => CONDITIONAL,
        _ => 0,
    }
}

fn cells<'a>(names: impl Iterator<Item = &'a str>) -> u8 {
    names.fold(0, |found, name| found | cell(name))
}

/// Return the class and namespace cells required by the class body, in that order.
pub fn required(body: &[ast::Stmt], string_annotations: bool) -> (bool, bool) {
    let cells = ClassCells::of(body, string_annotations);
    (cells.class, cells.dict)
}

/// Return the class, namespace and conditional annotation cells needed by lexical reads.
pub fn required_with_conditional(
    body: &[ast::Stmt],
    string_annotations: bool,
) -> (bool, bool, bool) {
    let cells = ClassCells::of(body, string_annotations);
    (cells.class, cells.dict, cells.conditional)
}

/// The implicit cells supplied by a class and the ones its own code reads from an outer scope.
pub struct ClassCells {
    pub class: bool,
    pub dict: bool,
    pub conditional: bool,
    enclosing: u8,
}

impl ClassCells {
    pub fn of(body: &[ast::Stmt], string_annotations: bool) -> Self {
        let mut scope = Scope {
            class: true,
            string_annotations,
            ..Scope::default()
        };
        scope.visit_body(body);
        let needed = scope.nested | (scope.annotation & (!scope.shadows | scope.nonlocal));
        Self {
            class: needed & CLASS != 0,
            dict: needed & DICT != 0,
            conditional: needed & CONDITIONAL != 0,
            enclosing: scope.free(),
        }
    }

    /// A class can supply a method's cell and also read an outer cell with the same name.
    pub fn supplied_only(&self) -> impl Iterator<Item = &'static str> {
        [
            ("__class__", self.class, CLASS),
            ("__classdict__", self.dict, DICT),
            ("__conditional_annotations__", self.conditional, CONDITIONAL),
        ]
        .into_iter()
        .filter_map(|(name, supplied, bit)| (supplied && self.enclosing & bit == 0).then_some(name))
    }
}

/// Whether the function body reads a free __class__, including through zero-argument super.
pub fn function_needed(
    body: &[ast::Stmt],
    parameters: Option<&ast::Parameters>,
    string_annotations: bool,
) -> bool {
    let mut scope = function_scope(parameters, string_annotations);
    scope.visit_body(body);
    scope.free() & CLASS != 0
}

fn function_scope(parameters: Option<&ast::Parameters>, string_annotations: bool) -> Scope {
    Scope {
        string_annotations,
        shadows: parameters.map_or(0, |params| {
            cells(params.iter().map(|arg| arg.name().as_str()))
        }),
        ..Scope::default()
    }
}

#[derive(Default)]
struct Scope {
    class: bool,
    string_annotations: bool,
    shadows: u8,
    direct: u8,
    nonlocal: u8,
    walrus: u8,
    nested: u8,
    // Lazy annotations consult the class namespace before taking a free variable.
    annotation: u8,
}

impl Scope {
    fn child(&self) -> Self {
        Self {
            string_annotations: self.string_annotations,
            ..Self::default()
        }
    }
    fn free(&self) -> u8 {
        let reads = if self.class {
            self.direct
        } else {
            self.direct | self.nested | self.annotation
        };
        self.nonlocal | (reads & !self.shadows)
    }
    fn add_annotation(&mut self, child: Self) {
        self.annotation |= (child.direct | child.annotation) & !child.shadows;
        self.nested |= child.nested & !child.shadows;
    }
    fn type_params(&self, params: &ast::TypeParams) -> Self {
        let mut child = self.child();
        child.shadows = cells(params.type_params.iter().map(|p| p.name().as_str()));
        child.visit_type_params(params);
        child
    }
    fn comprehension(&mut self, plan: Plan<'_>) {
        self.visit_expr(plan.outer);
        let mut child = self.child();
        plan.visit_body(|expr| child.visit_expr(expr));
        self.walrus |= child.walrus;
        self.shadows |= child.walrus;
        // CPython drops a free __class__ when inlining eager comprehensions into classes.
        if matches!(plan.output, Output::Generator(_)) || !self.class {
            self.nested |= child.free();
        }
    }
}
impl<'a> Visitor<'a> for Scope {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if self.class {
            // Even an unannotated method gets an annotation scope in Python 3.14.
            if match stmt {
                ast::Stmt::FunctionDef(f) => !self.string_annotations || f.type_params.is_some(),
                ast::Stmt::AnnAssign(_) => !self.string_annotations,
                ast::Stmt::ClassDef(c) => c.type_params.is_some(),
                ast::Stmt::TypeAlias(_) => true,
                _ => false,
            } {
                self.annotation |= DICT;
            }
        }
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                self.shadows |= cell(f.name.as_str());
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                }
                let mut method = self.child();
                method.shadows = cells(f.parameters.iter().map(|arg| arg.name().as_str()));
                method.visit_body(&f.body);
                for default in f
                    .parameters
                    .iter_non_variadic_params()
                    .filter_map(|p| p.default.as_deref())
                {
                    self.visit_expr(default);
                }
                let mut generic = f.type_params.as_deref().map(|p| self.type_params(p));
                let parent = generic.as_mut().unwrap_or(self);
                for arg in f.parameters.iter() {
                    parent.visit_parameter(arg.as_parameter());
                }
                if let Some(result) = &f.returns {
                    parent.visit_annotation(result);
                }
                parent.nested |= method.free();
                if let Some(generic) = generic {
                    self.add_annotation(generic);
                }
            }
            ast::Stmt::ClassDef(c) => {
                self.shadows |= cell(c.name.as_str());
                for decorator in &c.decorator_list {
                    self.visit_decorator(decorator);
                }
                let mut inner = self.child();
                inner.class = true;
                inner.visit_body(&c.body);
                let mut generic = c.type_params.as_deref().map(|p| self.type_params(p));
                let parent = generic.as_mut().unwrap_or(self);
                if let Some(args) = &c.arguments {
                    parent.visit_arguments(args);
                }
                // The inner class's methods capture it. Its body can capture the outer class.
                parent.nested |= inner.free();
                if let Some(generic) = generic {
                    self.add_annotation(generic);
                }
            }
            ast::Stmt::AnnAssign(a) => {
                if crate::annotation_visits_target(a) {
                    self.visit_expr(&a.target);
                }
                if let Some(value) = &a.value {
                    self.visit_expr(value);
                }
                if !self.string_annotations {
                    let mut annotation = self.child();
                    annotation.visit_expr(&a.annotation);
                    // The symbol table still analyzes nested scopes within an unevaluated
                    // function-local annotation, but ignores its own name references.
                    if !self.class {
                        annotation.direct = 0;
                    }
                    self.add_annotation(annotation);
                }
            }
            ast::Stmt::TypeAlias(a) => {
                self.visit_expr(&a.name);
                let mut alias = a
                    .type_params
                    .as_deref()
                    .map_or_else(|| self.child(), |p| self.type_params(p));
                alias.visit_expr(&a.value);
                self.add_annotation(alias);
            }
            ast::Stmt::Global(g) => self.shadows |= cells(g.names.iter().map(|n| n.as_str())),
            ast::Stmt::Nonlocal(n) => self.nonlocal |= cells(n.names.iter().map(|n| n.as_str())),
            _ => visitor::walk_stmt(self, stmt),
        }
    }
    fn visit_alias(&mut self, a: &'a ast::Alias) {
        self.shadows |= a.asname.as_ref().map_or_else(
            || a.name.as_str().split('.').next().map_or(0, cell),
            |n| cell(n.as_str()),
        );
    }
    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(h) = handler;
        self.shadows |= h.name.as_ref().map_or(0, |n| cell(n.as_str()));
        visitor::walk_except_handler(self, handler);
    }
    fn visit_pattern(&mut self, p: &'a ast::Pattern) {
        let name = match p {
            ast::Pattern::MatchAs(p) => p.name.as_ref(),
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        self.shadows |= name.map_or(0, |n| cell(n.as_str()));
        visitor::walk_pattern(self, p);
    }
    fn visit_annotation(&mut self, expr: &'a ast::Expr) {
        if !self.string_annotations {
            let mut child = self.child();
            child.visit_expr(expr);
            self.add_annotation(child);
        }
    }
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        match expr {
            ast::Expr::Name(n) if n.ctx == ast::ExprContext::Load => {
                self.direct |= cell(n.id.as_str());
                if !self.class && n.id.as_str() == "super" {
                    self.direct |= CLASS;
                }
            }
            ast::Expr::Name(n) => self.shadows |= cell(n.id.as_str()),
            ast::Expr::Named(n) => {
                if let ast::Expr::Name(target) = &*n.target {
                    self.walrus |= cell(target.id.as_str());
                }
                visitor::walk_expr(self, expr);
            }
            ast::Expr::Lambda(f) => {
                let mut child = self.child();
                if let Some(params) = &f.parameters {
                    self.visit_parameters(params);
                    child.shadows = cells(params.iter().map(|a| a.name().as_str()));
                }
                child.visit_expr(&f.body);
                self.nested |= child.free();
            }
            expr if let Some(plan) = Plan::from_expr(expr) => self.comprehension(plan),
            _ => visitor::walk_expr(self, expr),
        }
    }
}
