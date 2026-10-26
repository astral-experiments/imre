//! Reuses final function bodies across edits to explicitly typed scalar defaults.
//! Every other source edit changes the structural key and starts a fresh lowering.

use std::path::PathBuf;

use super::*;
use crate::types::ShapePrefix;

pub(super) type DefaultId = (usize, usize);

#[derive(PartialEq)]
struct Structure {
    root: PathBuf,
    entry: ModuleId,
    // Logical and canonical file identities, qualified module name and source with defaults masked.
    modules: Vec<(PathBuf, PathBuf, String, String)>,
    missing: Vec<PathBuf>,
}

struct Revision {
    structure: Structure,
}

impl Revision {
    fn from_graph(graph: &SourceGraph) -> Option<Self> {
        if graph.reachable_only()
            || graph.command_startup.is_some()
            || graph.session.has_extension_roots()
        {
            return None;
        }
        let mut has_defaults = false;
        let mut masked = HashMap::new();
        for (module_id, module) in graph.modules.iter().enumerate() {
            let mut hash = blake3::Hasher::new();
            let mut previous = 0;
            for node in &module.parsed.syntax().body {
                let ast::Stmt::FunctionDef(function) = node else {
                    continue;
                };
                if function.returns.is_none()
                    || !function.decorator_list.is_empty()
                    || !function
                        .parameters
                        .iter_non_variadic_params()
                        .all(|p| p.annotation().is_some())
                {
                    continue;
                }
                for parameter in function.parameters.iter_non_variadic_params() {
                    let (Some(ast::Expr::Name(ty)), Some(default)) =
                        (parameter.annotation(), parameter.default.as_deref())
                    else {
                        continue;
                    };
                    let tag = match (ty.id.as_str(), default) {
                        ("bool", ast::Expr::BooleanLiteral(_)) => 0,
                        ("int", ast::Expr::NumberLiteral(n))
                            if matches!(n.value, ast::Number::Int(_)) =>
                        {
                            1
                        }
                        _ => continue,
                    };
                    let start = usize::from(default.start());
                    let end = usize::from(default.end());
                    // Length framing keeps literal holes distinct from source bytes and from one another.
                    hash.update(&(start - previous).to_le_bytes());
                    hash.update(&module.source.as_bytes()[previous..start]);
                    hash.update(&[tag]);
                    has_defaults = true;
                    previous = end;
                }
            }
            if previous != 0 {
                hash.update(&(module.source.len() - previous).to_le_bytes());
                hash.update(&module.source.as_bytes()[previous..]);
                masked.insert(module_id, hash.finalize().to_hex().to_string());
            }
        }
        if !has_defaults {
            return None;
        }
        // Files without holes already have a byte digest from the fresh loader.
        let modules = graph
            .inputs
            .files
            .iter()
            .zip(&graph.modules)
            .enumerate()
            .map(|(id, ((path, identity, digest), module))| {
                (
                    path.clone(),
                    identity.clone(),
                    module.name.clone(),
                    masked.remove(&id).unwrap_or_else(|| digest.clone()),
                )
            })
            .collect();
        Some(Self {
            structure: Structure {
                root: graph.session.root.clone(),
                entry: graph.entry,
                modules,
                missing: graph.inputs.missing.clone(),
            },
        })
    }
}

// A cached body may refer to existing compiler IDs. It must not allocate types, helpers, or
// regex descriptors that later functions rely on, or rely on an unrecorded semantic query.
#[derive(Clone, Copy, PartialEq)]
struct Context {
    shapes: usize,
    helpers: usize,
    regexes: usize,
    globals: usize,
    class_bindings: usize,
}

impl Context {
    fn new(checker: &Checker<'_>) -> Self {
        Self {
            shapes: checker.types.shape_count(),
            helpers: checker.helpers.len(),
            regexes: checker.globals.regex_literals.borrow().len(),
            globals: checker.globals.types().len(),
            class_bindings: checker.globals.class_data.borrow().binding_count(),
        }
    }
}

#[derive(PartialEq)]
enum ScalarDefault {
    Int(String),
    Bool(bool),
}

#[derive(Default)]
pub(crate) struct CallerCache {
    revision: Option<Revision>,
    changed: HashSet<DefaultId>,
    defaults: HashMap<DefaultId, ScalarDefault>,
    shapes: ShapePrefix,
    abi: Vec<Type>,
    bodies: HashMap<usize, (Context, LoweredBody)>,
}

impl CallerCache {
    /// Compares all current inputs before inspecting a single saved body.
    pub(crate) fn prepare(&mut self, graph: &SourceGraph) -> Option<&mut Self> {
        let Some(current) = Revision::from_graph(graph) else {
            *self = Self::default();
            return None;
        };
        if self
            .revision
            .as_ref()
            .is_none_or(|previous| previous.structure != current.structure)
        {
            self.bodies.clear();
        }
        self.revision = Some(current);
        Some(self)
    }

    /// Nominal IDs follow source order. Check all inferred ABI slots and module-global types
    /// as well, since a saved function can address them by their numeric index.
    pub(super) fn validate_abi(
        &mut self,
        definitions: &[(ModuleId, ast::StmtFunctionDef, Signature)],
        globals: &ModuleValues,
    ) {
        let mut defaults = HashMap::new();
        for (_, _, signature) in definitions {
            for (index, param) in signature.params.iter().enumerate() {
                let value = match param.default.as_ref().map(|value| &value.kind) {
                    Some(ExprKind::Int(value)) => ScalarDefault::Int(value.clone()),
                    Some(ExprKind::Bool(value)) => ScalarDefault::Bool(*value),
                    _ => continue,
                };
                defaults.insert((signature.id, index), value);
            }
        }
        self.changed.clear();
        self.changed.extend(
            self.defaults
                .iter()
                .filter_map(|(id, old)| (defaults.get(id) != Some(old)).then_some(*id)),
        );
        self.defaults = defaults;
        let global_types = globals.types();
        let slots = global_types
            .iter()
            .copied()
            .chain(definitions.iter().flat_map(|(_, _, signature)| {
                signature
                    .params
                    .iter()
                    .map(|p| p.ty)
                    .chain(std::iter::once(signature.returns))
            }));
        if !self.abi.iter().copied().eq(slots.clone()) {
            self.bodies.clear();
            self.abi = slots.collect();
        }
    }

    pub(super) fn body(
        &mut self,
        mut checker: Checker<'_>,
        definition: &ast::StmtFunctionDef,
        signature: &Signature,
    ) -> BodyResult {
        let before = Context::new(&checker);
        if let Some((previous, body)) = self.bodies.get(&signature.id)
            && before == *previous
            && checker.types.matches_prefix(&self.shapes)
            && body
                .default_reads
                .as_ref()
                .is_some_and(|reads| reads.iter().all(|id| !self.changed.contains(id)))
        {
            return Ok((body.clone(), HashMap::new()));
        }
        let (types, helpers, globals, session) = (
            checker.types,
            checker.helpers,
            checker.globals,
            &checker.graph.session,
        );
        let graph = checker.graph;
        let module = checker.module;
        let diagnostics = graph.checked_diagnostic_count(module);
        let hints = session.hint_reads();
        checker.default_reads = Some(Vec::new());
        let (body, sums) = lower_body(checker, definition, signature)?;
        if body.default_reads.is_some()
            && !body.calls.is_empty()
            && sums.is_empty()
            && before.shapes == types.shape_count()
            && before.helpers == helpers.len()
            && before.regexes == globals.regex_literals.borrow().len()
            && before.globals == globals.types().len()
            && before.class_bindings == globals.class_data.borrow().binding_count()
            && diagnostics == graph.checked_diagnostic_count(module)
            && hints == session.hint_reads()
        {
            self.bodies.insert(signature.id, (before, body.clone()));
        } else {
            self.bodies.remove(&signature.id);
        }
        Ok((body, sums))
    }

    pub(super) fn finish(&mut self, types: &Types) {
        if !self.bodies.is_empty() {
            types.save_prefix(&mut self.shapes);
        }
    }
}

#[cfg(test)]
thread_local! {
    static LOWERED: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn record_lowering(name: &str) {
    LOWERED.with_borrow_mut(|names| {
        if let Some(names) = names {
            names.push(name.to_owned());
        }
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    use super::LOWERED;
    use crate::{CompileOptions, IncrementalCompiler, UnitCompilation, compile_file_units};

    fn observe<T>(run: impl FnOnce() -> T) -> (T, Vec<String>) {
        LOWERED.with_borrow_mut(|names| *names = Some(Vec::new()));
        let result = run();
        (
            result,
            LOWERED.with_borrow_mut(|names| names.take().unwrap()),
        )
    }

    fn llvm(compilation: UnitCompilation) -> Vec<(String, String)> {
        compilation
            .units
            .into_iter()
            .map(|u| (u.name, u.llvm_ir))
            .collect()
    }

    fn compile(compiler: &mut IncrementalCompiler, entry: &Path, root: &Path) -> Vec<String> {
        let options = CompileOptions::default();
        let (retained, ran) = observe(|| compiler.compile_file_units(entry, Some(root), &options));
        let fresh = compile_file_units(entry, Some(root), &options);
        assert_eq!(retained.map(llvm).unwrap(), fresh.map(llvm).unwrap());
        ran
    }

    #[test]
    fn handler_class_slots_survive_a_cached_default_edit() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let entry = root.join("main.py");
        let library = root.join("marker.py");
        let echo = include_str!("../../../../tests/fixtures/socketserver/echo.py");
        fs::write(
            &entry,
            format!("from marker import marker\n{echo}\nprint(marker())\n"),
        )
        .unwrap();
        let default = "def marker(value: int = 1) -> int:\n    return value\n";
        let mut compiler = IncrementalCompiler::default();
        fs::write(&library, default).unwrap();
        compile(&mut compiler, &entry, root);
        fs::write(&library, default.replace("= 1", "= 19")).unwrap();
        compile(&mut compiler, &entry, root);
        compile(&mut compiler, &entry, root);
    }

    #[test]
    fn records_only_defaults_consumed_by_the_argument_binder() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let entry = root.join("main.py");
        let library = root.join("library.py");
        fs::write(&entry, "from library import score\ndef omitted() -> int:\n    return score()\ndef explicit() -> int:\n    return score(flag=False, value=7)\ndef positional() -> int:\n    return score(7)\ndef keyword() -> int:\n    return score(flag=False)\nprint(omitted() + explicit() + positional() + keyword())\n").unwrap();
        let base = "def score(value: int = 7, *, flag: bool = False) -> int:\n    return value if flag else 0\n";
        let mut compiler = IncrementalCompiler::default();
        fs::write(&library, base).unwrap();
        assert_eq!(
            compile(&mut compiler, &entry, root),
            ["omitted", "explicit", "positional", "keyword", "score"]
        );
        fs::write(&library, base.replace("7", "70_001")).unwrap();
        assert_eq!(
            compile(&mut compiler, &entry, root),
            ["omitted", "keyword", "score"]
        );
        fs::write(
            &library,
            base.replace("7", "70_001").replace("False", "True"),
        )
        .unwrap();
        assert_eq!(
            compile(&mut compiler, &entry, root),
            ["omitted", "positional", "score"]
        );
        assert_eq!(compile(&mut compiler, &entry, root), ["score"]);
    }

    #[test]
    fn a_starred_operand_tracks_only_the_default_actually_copied() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let entry = root.join("main.py");
        let source = "def combine(a: int = 2, b: int = 7) -> int:\n    return a + b\ndef both(pair: tuple[int, int]) -> int:\n    return combine(*pair)\ndef one(item: tuple[int]) -> int:\n    return combine(*item)\nprint(both((3, 5)) + one((4,)))\n";
        fs::write(&entry, source).unwrap();
        let mut compiler = IncrementalCompiler::default();
        compile(&mut compiler, &entry, root);
        fs::write(&entry, source.replace("= 7", "= 11")).unwrap();
        let ran = compile(&mut compiler, &entry, root);
        assert!(!ran.iter().any(|name| name == "both"), "{ran:?}");
        assert!(ran.iter().any(|name| name == "one"), "{ran:?}");
    }

    #[test]
    fn an_effect_or_binding_edit_lowers_callers_again_and_errors_use_current_ranges() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let entry = root.join("main.py");
        let library = root.join("library.py");
        let caller = "from library import score\ndef wrapper() -> int:\n    return score(7)\ndef outer() -> int:\n    try:\n        return wrapper()\n    except ValueError:\n        return 0\nprint(outer())\n";
        fs::write(&entry, caller).unwrap();
        let base = "def score(value: int = 7) -> int:\n    return value\n";
        fs::write(&library, base).unwrap();
        let mut compiler = IncrementalCompiler::default();
        compile(&mut compiler, &entry, root);
        fs::write(&library, base.replace("= 7", "= 77")).unwrap();
        assert_eq!(compile(&mut compiler, &entry, root), ["score"]);
        fs::write(
            &library,
            base.replace(
                "    return value",
                "    if value == 7:\n        raise ValueError(\"seven\")\n    return value",
            ),
        )
        .unwrap();
        assert_eq!(
            compile(&mut compiler, &entry, root),
            ["wrapper", "outer", "score"]
        );
        fs::write(
            &library,
            format!("{base}\ndef replacement(value: int = 7) -> int:\n    return 2\n"),
        )
        .unwrap();
        fs::write(
            &entry,
            caller.replace(
                "from library import score",
                "from library import replacement as score",
            ),
        )
        .unwrap();
        assert_eq!(
            compile(&mut compiler, &entry, root),
            ["wrapper", "outer", "score", "replacement"]
        );
        fs::write(
            &library,
            format!(
                "{}\n\ndef unused() -> int:\n    return \"bad\"\n",
                base.replace("= 7", "= 123456789")
            ),
        )
        .unwrap();
        let options = CompileOptions::default();
        fs::write(&entry, caller).unwrap();
        let retained = compiler
            .compile_file_units(&entry, Some(root), &options)
            .err()
            .unwrap();
        assert_eq!(
            retained,
            compile_file_units(&entry, Some(root), &options)
                .err()
                .unwrap()
        );
        assert!(retained.contains("library.py:6:"), "{retained}");
        fs::write(&library, base).unwrap();
        assert_eq!(
            compile(&mut compiler, &entry, root),
            ["wrapper", "outer", "score"]
        );
    }

    #[test]
    fn bodies_creating_helpers_or_types_run_again() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let entry = root.join("main.py");
        let source = "from typing import Callable\ndef score(value: int = 7) -> int:\n    return value\ndef apply(f: Callable[[int], int]) -> int:\n    return f(3)\ndef indirect() -> int:\n    return apply(score)\ndef fresh_type() -> int:\n    item: tuple[bool, int] = (True, score(7))\n    return item[1]\ndef helper() -> list[int]:\n    return [score() for _ in range(2)]\nprint(indirect() + fresh_type() + helper()[0])\n";
        fs::write(&entry, source).unwrap();
        let mut compiler = IncrementalCompiler::default();
        compile(&mut compiler, &entry, root);
        fs::write(&entry, source.replace("= 7", "= 77")).unwrap();
        let ran = compile(&mut compiler, &entry, root);
        assert!(!ran.iter().any(|n| n == "indirect"), "{ran:?}");
        assert!(ran.iter().any(|n| n == "helper"), "{ran:?}");
        assert!(ran.iter().any(|n| n == "fresh_type"), "{ran:?}");
        fs::write(
            &entry,
            source.replace(" = 7", "").replace("score()", "score(0)"),
        )
        .unwrap();
        compile(&mut compiler, &entry, root);
        assert!(compiler.callers.revision.is_none() && compiler.callers.bodies.is_empty());
    }

    #[test]
    fn ruff_preserves_the_caller_passing_compute() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let project = root.join("conformance_report");
        fs::create_dir(&project).unwrap();
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/conformance_report");
        for module in ["__init__", "compute", "app", "model", "parse", "render"] {
            let name = format!("{module}.py");
            fs::copy(fixtures.join(&name), project.join(&name)).unwrap();
        }
        let entry = project.join("compute.py");
        let app = project.join("app.py");
        let original = fs::read_to_string(&app).unwrap();
        let mut compiler = IncrementalCompiler::default();
        let initial = compile(&mut compiler, &entry, root);
        assert!(initial.iter().any(|n| n == "main") && initial.iter().any(|n| n == "compute_main"));
        fs::write(
            &app,
            original.replace("compute: bool = False", "compute: bool = True"),
        )
        .unwrap();
        let edited = compile(&mut compiler, &entry, root);
        assert!(edited.iter().any(|n| n == "main"), "{edited:?}");
        assert!(!edited.iter().any(|n| n == "compute_main"), "{edited:?}");
    }
}
