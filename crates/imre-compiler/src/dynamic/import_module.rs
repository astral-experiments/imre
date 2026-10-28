use super::{Expr, Visitor, ast, resolve, visitor};
use crate::modules::{Binding, Intrinsic, IntrinsicModule, SourceGraph};

/// Python sources used only by the boxed importer.
pub(super) fn boxed_source(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "textwrap" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some(("textwrap.py", SOURCE.get_or_init(|| {
                // The indentation functions are independent of TextWrapper's regular expressions.
                const UPSTREAM: &str = include_str!(
                    "../../../../benchmarks/cpython_stdlib/full/textwrap/upstream/Lib/textwrap.py"
                );
                let first = UPSTREAM.find("\ndef dedent(").unwrap();
                let (functions, _) = UPSTREAM[first..].split_once("\nif __name__").unwrap();
                format!("{}{functions}", include_str!("../../stdlib/textwrap/wrapper.py"))
            })))
        }
        "importlib.util" => Some((
            "importlib/util.py",
            include_str!("../../stdlib/importlib/util.py"),
        )),
        "importlib.machinery" => Some((
            "importlib/machinery.py",
            include_str!("../../stdlib/importlib/machinery.py"),
        )),
        "_frozen_importlib" => {
            // Compile the pinned ModuleSpec definition with the boxed object protocol.
            let bootstrap = include_str!(
                "../../../../benchmarks/cpython_stdlib/full/importlib/upstream/Lib/importlib/_bootstrap.py"
            );
            let start = bootstrap.find("class ModuleSpec:").unwrap();
            let (spec, _) = bootstrap[start..]
                .split_once("\ndef spec_from_loader(")
                .unwrap();
            Some(("_frozen_importlib.py", spec))
        }
        "_frozen_importlib_external" => Some((
            "_frozen_importlib_external.py",
            include_str!("../../stdlib/importlib/source_loader.py"),
        )),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct RegisteredImports {
    pub(super) called: bool,
    pub(super) targets: Vec<String>,
}

/// Include literal import_module and find_spec targets in the same source registry as import statements.
/// Runtime expressions still select their target when the call executes.
pub(crate) fn registered_imports(graph: &SourceGraph) -> RegisteredImports {
    struct Calls<'a> {
        graph: &'a SourceGraph,
        module: usize,
        imports: RegisteredImports,
    }
    impl<'ast> Visitor<'ast> for Calls<'_> {
        fn visit_expr(&mut self, expr: &'ast Expr) {
            if let Expr::Call(call) = expr
                && let Some(Binding::Intrinsic(Intrinsic::RegisteredImport(function))) =
                    self.graph.resolve(self.module, &call.func)
                && let Some((owner, function)) = function.rsplit_once('.')
                && self
                    .graph
                    .session
                    .is_stdlib_function(self.module, &call.func, owner, function)
            {
                self.imports.called = true;
                if let Some(name) = target(call) {
                    self.imports.targets.push(name);
                }
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut calls = Calls {
        graph,
        module: 0,
        imports: RegisteredImports::default(),
    };
    for (id, module) in graph.modules.iter().enumerate() {
        if module.bindings.values().any(|binding| {
            matches!(
                binding,
                Binding::Intrinsic(Intrinsic::RegisteredImport(_))
                    | Binding::IntrinsicModule(
                        IntrinsicModule::Importlib | IntrinsicModule::ImportlibUtil
                    )
            )
        }) {
            calls.module = id;
            calls.visit_body(&module.parsed.syntax().body);
        }
    }
    calls.imports
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
