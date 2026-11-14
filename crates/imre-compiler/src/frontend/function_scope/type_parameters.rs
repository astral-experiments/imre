//! Validate nonlocal declarations against the lexical scope of class type parameters.

use std::collections::BTreeMap;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

use super::ClosureNames;

#[derive(Clone, Copy)]
enum Binding {
    Parameter,
    Global,
}

/// Return a source-level error before any effects from the module can execute.
pub(crate) fn check_nonlocals(source: &str, body: &[ast::Stmt]) -> Result<(), String> {
    struct Check<'s> {
        source: &'s str,
        bindings: BTreeMap<String, Binding>,
        error: Option<String>,
    }

    impl<'a> Visitor<'a> for Check<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.error.is_some() {
                return;
            }
            match stmt {
                ast::Stmt::ClassDef(class) => {
                    let outer = self.bindings.clone();
                    if let Some(params) = &class.type_params {
                        self.bindings.extend(
                            params
                                .iter()
                                .map(|p| (p.name().to_string(), Binding::Parameter)),
                        );
                    }
                    if !self.bindings.is_empty() {
                        // CPython removes ordinary class-local bindings from its type-parameter
                        // set for nested scopes, but a class still doesn't bind a closure cell.
                        let locals = ClosureNames::class_declarations(&class.body).locals;
                        self.bindings.retain(|name, binding| {
                            !matches!(binding, Binding::Parameter) || !locals.contains(name)
                        });
                    }
                    self.visit_body(&class.body);
                    self.bindings = outer;
                }
                ast::Stmt::FunctionDef(function) if !self.bindings.is_empty() => {
                    // An intervening ordinary local hides the type parameter. A
                    // nonlocal declaration forwards the lookup to its outer scope.
                    let outer = self.bindings.clone();
                    let mut names = ClosureNames::default();
                    names.visit_body(&function.body);
                    names.locals.extend(
                        function
                            .parameters
                            .iter()
                            .map(|p| p.as_parameter().name.to_string()),
                    );
                    self.bindings.retain(|name, binding| {
                        if names.globals.contains(name) {
                            *binding = Binding::Global;
                            true
                        } else {
                            names.nonlocals.contains(name) || !names.locals.contains(name)
                        }
                    });
                    self.visit_body(&function.body);
                    self.bindings = outer;
                }
                ast::Stmt::Nonlocal(statement) => {
                    for name in &statement.names {
                        let message = match self.bindings.get(name.as_str()) {
                            Some(Binding::Parameter) => format!(
                                "SyntaxError: nonlocal binding not allowed for type parameter '{name}'"
                            ),
                            Some(Binding::Global) => {
                                format!("SyntaxError: no binding for nonlocal '{name}' found")
                            }
                            None => continue,
                        };
                        self.error = Some(crate::frontend::diagnostic_message(
                            self.source,
                            statement,
                            &message,
                        ));
                        break;
                    }
                }
                _ => visitor::walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }

    let mut check = Check {
        source,
        bindings: BTreeMap::new(),
        error: None,
    };
    check.visit_body(body);
    check.error.map_or(Ok(()), Err)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeParameterScope {
    Function,
    MethodClosure,
}

/// A synchronous boxed function with one body scope can capture its unbounded type parameters.
pub(crate) fn boxed_function_type_params<'a>(
    function: &'a ast::StmtFunctionDef,
    string_annotations: bool,
    nested: bool,
    private_names: Option<&imre_ast::Names>,
) -> Option<&'a ast::TypeParams> {
    function.type_params.as_deref().filter(|_| {
        type_parameter_scope(function, string_annotations, nested, private_names)
            == Some(TypeParameterScope::Function)
    })
}

/// Parameters whose names and kinds can be installed by the registered function frame.
pub(crate) fn function_parameter_header<'a>(
    function: &'a ast::StmtFunctionDef,
    string_annotations: bool,
    nested: bool,
    private_names: Option<&imre_ast::Names>,
) -> Option<&'a ast::TypeParams> {
    let params = function.type_params.as_deref()?;
    let mut names = std::collections::HashSet::new();
    if function.is_async
        || !params.iter().all(|p| {
            let name = private_names.map_or(p.name().as_str(), |names| names.definition(p.name()));
            p.default().is_none()
                && match p {
                    ast::TypeParam::TypeVar(v) => v.bound.is_none(),
                    ast::TypeParam::TypeVarTuple(_) | ast::TypeParam::ParamSpec(_) => {
                        string_annotations
                    }
                }
                && !matches!(name, "__classdict__" | "__debug__")
                && (!nested || !name.starts_with("__"))
                && names.insert(p.name().as_str())
        })
    {
        return None;
    }

    Some(params)
}

/// Determine whether a generic function can select object lowering during class layout or boxing.
pub(crate) fn type_parameter_scope(
    function: &ast::StmtFunctionDef,
    string_annotations: bool,
    nested: bool,
    private_names: Option<&imre_ast::Names>,
) -> Option<TypeParameterScope> {
    use ast::AnyNodeRef::*;
    use ast::visitor::source_order::{SourceOrderVisitor, TraversalSignal};

    function_parameter_header(function, string_annotations, nested, private_names)?;

    struct BodyScope {
        nested: bool,
        unsupported: bool,
        has_lexical_function: bool,
        needs_separate_site: bool,
    }
    impl<'a> SourceOrderVisitor<'a> for BodyScope {
        fn visit_annotation(&mut self, annotation: &'a ast::Expr) {
            // The annotation provider owns its operands. Keep the lexical
            // exclusions (including class cells) but not its executable sites.
            let enclosing = self.needs_separate_site;
            self.visit_expr(annotation);
            self.needs_separate_site = enclosing;
        }

        fn enter_node(&mut self, node: ast::AnyNodeRef<'a>) -> TraversalSignal {
            if let StmtFunctionDef(def) = node {
                self.has_lexical_function = true;
                self.unsupported |= def.type_params.is_some() || def.is_async;
            }
            // Imports and template fields have separate site owners. A non-name
            // annotation can use the ordinary attribute store only with a value.
            self.needs_separate_site |=
                matches!(node, StmtImport(_) | StmtImportFrom(_) | ExprTString(_))
                    || matches!(node, StmtAnnAssign(a) if !a.target.is_name_expr()
                    && !(a.target.is_attribute_expr() && a.value.is_some()));
            self.unsupported |= matches!(
                node,
                StmtClassDef(_)
                    | StmtTypeAlias(_)
                    | StmtNonlocal(_)
                    | ExprLambda(_)
                    | ExprGenerator(_)
                    | ExprListComp(_)
                    | ExprSetComp(_)
                    | ExprDictComp(_)
                    | ExprYield(_)
                    | ExprYieldFrom(_)
            );
            // Nested declarations with a class cell need a different capture path.
            self.unsupported |= self.nested
                && matches!(node, ExprName(n) if matches!(n.id.as_str(), "__class__" | "super"));
            if self.unsupported {
                TraversalSignal::Skip
            } else {
                TraversalSignal::Traverse
            }
        }
    }
    let mut body = BodyScope {
        nested,
        unsupported: false,
        has_lexical_function: false,
        needs_separate_site: false,
    };
    body.visit_body(&function.body);
    if body.unsupported {
        return None;
    }
    if !body.has_lexical_function {
        return Some(TypeParameterScope::Function);
    }
    (nested
        && !body.needs_separate_site
        && function.decorator_list.is_empty()
        && function
            .parameters
            .iter_non_variadic_params()
            .all(|p| p.default.is_none()))
    .then_some(TypeParameterScope::MethodClosure)
}
