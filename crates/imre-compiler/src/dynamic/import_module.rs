use super::{Expr, Visitor, ast, resolve, visitor};
use crate::modules::{Binding, Intrinsic, IntrinsicModule, SourceGraph};

/// Include literal import_module targets in the same source registry as import statements.
/// Runtime expressions still select their target when the call executes.
pub(crate) fn literal_targets(graph: &SourceGraph) -> Vec<String> {
    struct Calls<'a> {
        graph: &'a SourceGraph,
        module: usize,
        names: Vec<String>,
    }
    impl<'ast> Visitor<'ast> for Calls<'_> {
        fn visit_expr(&mut self, expr: &'ast Expr) {
            if let Expr::Call(call) = expr
                && self.graph.resolve(self.module, &call.func)
                    == Some(Binding::Intrinsic(Intrinsic::ImportModule))
                && let Some(name) = target(call)
                && self.graph.session.is_import_module(self.module, &call.func)
            {
                self.names.push(name);
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut calls = Calls {
        graph,
        module: 0,
        names: Vec::new(),
    };
    for (id, module) in graph.modules.iter().enumerate() {
        if module.bindings.values().any(|binding| {
            matches!(
                binding,
                Binding::Intrinsic(Intrinsic::ImportModule)
                    | Binding::IntrinsicModule(IntrinsicModule::Importlib)
            )
        }) {
            calls.module = id;
            calls.visit_body(&module.parsed.syntax().body);
        }
    }
    calls.names
}

fn target(call: &ast::ExprCall) -> Option<String> {
    let literal = |name, position| {
        call.arguments
            .find_argument_value(name, position)?
            .as_string_literal_expr()
    };
    let name = literal("name", 0)?.value.to_str();
    let rest = name.trim_start_matches('.');
    let level = u32::try_from(name.len() - rest.len()).ok()?;
    let absolute = if level == 0 {
        name.to_owned()
    } else {
        resolve(literal("package", 1)?.value.to_str(), true, level, rest)?
    };
    absolute
        .split('.')
        .all(ruff_python_stdlib::identifiers::is_identifier)
        .then_some(absolute)
}
