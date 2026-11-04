//! Identify the implicit class and class-namespace cells before the class body executes.
use ast::visitor::{self, Visitor};
use ruff_python_ast as ast;

const CLASS: &str = "__class__";
const DICT: &str = "__classdict__";

/// Only free references from a child scope create a cell in the declaring class.
pub fn needed(body: &[ast::Stmt], string_annotations: bool) -> bool {
    needed_for(body, string_annotations, CLASS)
}

/// Annotation scopes created by a class suite can also close over the namespace.
pub fn dict_needed(body: &[ast::Stmt], string_annotations: bool) -> bool {
    needed_for(body, string_annotations, DICT)
}

/// Whether the function body reads a free __class__, including through zero-argument super.
pub fn function_needed(
    body: &[ast::Stmt],
    parameters: Option<&ast::Parameters>,
    string_annotations: bool,
) -> bool {
    let mut scope = function_scope(parameters, string_annotations);
    scope.visit_body(body);
    scope.free()
}

/// Apply the same lexical rules to the body of a lambda.
pub fn expression_needed(
    body: &ast::Expr,
    parameters: Option<&ast::Parameters>,
    string_annotations: bool,
) -> bool {
    let mut scope = function_scope(parameters, string_annotations);
    scope.visit_expr(body);
    scope.free()
}

fn function_scope(parameters: Option<&ast::Parameters>, string_annotations: bool) -> Scope {
    Scope {
        name: CLASS,
        string_annotations,
        shadows: parameters.is_some_and(|p| p.iter().any(|arg| arg.name().as_str() == CLASS)),
        ..Scope::default()
    }
}

fn needed_for(body: &[ast::Stmt], string_annotations: bool, name: &'static str) -> bool {
    let mut s = Scope {
        name,
        class: true,
        string_annotations,
        ..Scope::default()
    };
    s.visit_body(body);
    s.nested || (s.annotation && (!s.shadows || s.nonlocal))
}

#[derive(Default)]
struct Scope {
    name: &'static str,
    class: bool,
    string_annotations: bool,
    shadows: bool,
    direct: bool,
    nonlocal: bool,
    walrus: bool,
    nested: bool,
    // Lazy annotations consult the class namespace before taking a free variable.
    annotation: bool,
}

impl Scope {
    fn child(&self) -> Self {
        Self {
            name: self.name,
            string_annotations: self.string_annotations,
            ..Self::default()
        }
    }
    fn free(&self) -> bool {
        self.nonlocal
            || (!self.shadows && (self.direct || (!self.class && (self.nested || self.annotation))))
    }
    fn add_annotation(&mut self, child: Self) {
        if !child.shadows {
            self.annotation |= child.direct || child.annotation;
            self.nested |= child.nested;
        }
    }
    fn type_params(&self, params: &ast::TypeParams) -> Self {
        let mut child = self.child();
        child.shadows = params
            .type_params
            .iter()
            .any(|p| p.name().as_str() == self.name);
        child.visit_type_params(params);
        child
    }
    fn comprehension<'a>(
        &mut self,
        generators: &'a [ast::Comprehension],
        terms: impl IntoIterator<Item = &'a ast::Expr>,
        generator_expression: bool,
    ) {
        let mut child = self.child();
        for (index, generator) in generators.iter().enumerate() {
            if index == 0 {
                self.visit_expr(&generator.iter);
            } else {
                child.visit_expr(&generator.iter);
            }
            child.visit_expr(&generator.target);
            for condition in &generator.ifs {
                child.visit_expr(condition);
            }
        }
        for term in terms {
            child.visit_expr(term);
        }
        self.walrus |= child.walrus;
        self.shadows |= child.walrus;
        // CPython drops a free __class__ when inlining eager comprehensions into classes.
        if generator_expression || !self.class {
            self.nested |= child.free();
        }
    }
}
impl<'a> Visitor<'a> for Scope {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if self.class && self.name == DICT {
            // Even an unannotated method gets an annotation scope in Python 3.14.
            self.annotation |= match stmt {
                ast::Stmt::FunctionDef(f) => !self.string_annotations || f.type_params.is_some(),
                ast::Stmt::AnnAssign(_) => !self.string_annotations,
                ast::Stmt::ClassDef(c) => c.type_params.is_some(),
                ast::Stmt::TypeAlias(_) => true,
                _ => false,
            };
        }
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                self.shadows |= f.name.as_str() == self.name;
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                }
                let mut method = self.child();
                method.shadows = f
                    .parameters
                    .iter()
                    .any(|arg| arg.name().as_str() == self.name);
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
                self.shadows |= c.name.as_str() == self.name;
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
                self.visit_expr(&a.target);
                if let Some(value) = &a.value {
                    self.visit_expr(value);
                }
                if !self.string_annotations {
                    let mut annotation = self.child();
                    annotation.visit_expr(&a.annotation);
                    // The symbol table still analyzes nested scopes within an unevaluated
                    // function-local annotation, but ignores its own name references.
                    annotation.direct &= self.class;
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
            ast::Stmt::Global(g) => self.shadows |= g.names.iter().any(|n| n.as_str() == self.name),
            ast::Stmt::Nonlocal(n) => {
                self.nonlocal |= n.names.iter().any(|n| n.as_str() == self.name)
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
    fn visit_alias(&mut self, a: &'a ast::Alias) {
        self.shadows |= a.asname.as_ref().map_or_else(
            || a.name.as_str().split('.').next() == Some(self.name),
            |n| n.as_str() == self.name,
        );
    }
    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(h) = handler;
        self.shadows |= h.name.as_ref().is_some_and(|n| n.as_str() == self.name);
        visitor::walk_except_handler(self, handler);
    }
    fn visit_pattern(&mut self, p: &'a ast::Pattern) {
        let name = match p {
            ast::Pattern::MatchAs(p) => p.name.as_ref(),
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        self.shadows |= name.is_some_and(|n| n.as_str() == self.name);
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
                self.direct |= n.id.as_str() == self.name
                    || (self.name == CLASS && !self.class && n.id.as_str() == "super");
            }
            ast::Expr::Name(n) => self.shadows |= n.id.as_str() == self.name,
            ast::Expr::Named(n) => {
                self.walrus |= matches!(&*n.target, ast::Expr::Name(target) if target.id.as_str() == self.name);
                visitor::walk_expr(self, expr);
            }
            ast::Expr::Lambda(f) => {
                let mut child = self.child();
                if let Some(params) = &f.parameters {
                    self.visit_parameters(params);
                    child.shadows = params.iter().any(|a| a.name().as_str() == self.name);
                }
                child.visit_expr(&f.body);
                self.nested |= child.free();
            }
            ast::Expr::Generator(g) => self.comprehension(&g.generators, [g.elt.as_ref()], true),
            ast::Expr::ListComp(g) => self.comprehension(&g.generators, [g.elt.as_ref()], false),
            ast::Expr::SetComp(g) => self.comprehension(&g.generators, [g.elt.as_ref()], false),
            ast::Expr::DictComp(g) => self.comprehension(
                &g.generators,
                g.key.as_deref().into_iter().chain([g.value.as_ref()]),
                false,
            ),
            _ => visitor::walk_expr(self, expr),
        }
    }
}
