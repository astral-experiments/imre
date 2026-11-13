use super::{Expr, Visitor, ast, visitor};
use crate::modules::{Binding, Intrinsic, IntrinsicModule, SourceGraph, bundled_source};

/// Borrow the shipped Python module selected by this target and entry point.
pub(crate) fn bundled_module_source(
    name: &str,
    standalone: bool,
) -> Option<(&'static str, &'static str)> {
    if let Some(path) = crate::modules::charmap_provider_path(name, true) {
        return Some((path, include_str!("../../stdlib/encodings/charmap.py")));
    }
    if let Some((_, path)) = super::FIXED_UTF_PROVIDERS
        .iter()
        .find(|(id, _)| *id == name)
    {
        return Some((path, include_str!("../../stdlib/encodings/utf_fixed.py")));
    }
    match name {
        // Standalone executables can run without the launcher's installed stdlib ZIP.
        "_pydecimal" if standalone => bundled_source!("_pydecimal.py"),
        "types" => bundled_source!("types.py"),
        "reprlib" => bundled_source!("reprlib.py"),
        // The scalar Decimal stays available for checked serializers.
        "decimal" => Some(("decimal.py", include_str!("../../stdlib/decimal_boxed.py"))),
        "datetime" => Some((
            "datetime.py",
            include_str!(
                "../../../../benchmarks/cpython_stdlib/full/datetime/upstream/Lib/datetime.py"
            ),
        )),
        "enum" => bundled_source!("enum.py"),
        "json" => bundled_source!("json/__init__.py"),
        "argparse" => bundled_source!("argparse.py"),
        "_json" => bundled_source!("_json.py"),
        "json.encoder" => bundled_source!("json/encoder.py"),
        "json.decoder" => bundled_source!("json/decoder.py"),
        "json.__main__" => bundled_source!("json/__main__.py"),
        "json.tool" => bundled_source!("json/tool.py"),
        "json.scanner" => bundled_source!("json/scanner.py"),
        // A fresh _collections module exports the same accelerator type. Keep its
        // Python wrappers around native storage in their own module namespace.
        "_collections" => Some((
            "_collections.py",
            "from _imre_ordered_dict import OrderedDict\n",
        )),
        "_imre_ordered_dict" => bundled_source!("_imre_ordered_dict.py"),
        "runpy" => bundled_source!("runpy.py"),
        // CPython 3.14.5 Lib/trace.py. See licenses/CPython.txt.
        "trace" => bundled_source!("trace.py"),
        // CPython 3.14.5 Lib/traceback.py with native traceback positions.
        "traceback" => bundled_source!("traceback.py"),
        "subprocess" if cfg!(unix) => {
            bundled_source!("subprocess.py")
        }
        // CPython 3.14.5 Lib/linecache.py. See licenses/CPython.txt.
        "linecache" => bundled_source!("linecache.py"),
        "urllib.error" => bundled_source!("urllib/error.py"),
        "urllib.request" => bundled_source!("urllib/request.py"),
        // CPython 3.14.5 Lib/urllib/robotparser.py. See licenses/CPython.txt.
        "urllib.robotparser" => bundled_source!("urllib/robotparser.py"),
        "urllib.response" => bundled_source!("urllib/response.py"),
        "urllib.parse" => bundled_source!("urllib/parse.py"),
        "annotationlib" => bundled_source!("annotationlib.py"),
        "functools" => bundled_source!("functools.py"),
        "sysconfig" => Some((
            "sysconfig.py",
            if cfg!(target_os = "linux") {
                concat!(
                    include_str!("../../stdlib/sysconfig.py"),
                    "\n",
                    include_str!("../../stdlib/sysconfig_vars.py")
                )
            } else {
                include_str!("../../stdlib/sysconfig_vars.py")
            },
        )),
        "hashlib" => bundled_source!("hashlib.py"),
        "hmac" => bundled_source!("hmac.py"),
        // CPython 3.14.5 pathlib and its glob and NT path dependencies. See stdlib/docstrings/LICENSE.
        "pathlib" => bundled_source!("pathlib/__init__.py"),
        "pathlib._local" => bundled_source!("pathlib/_local.py"),
        "pathlib._os" => bundled_source!("pathlib/_os.py"),
        "pathlib.types" => bundled_source!("pathlib/types.py"),
        "glob" => bundled_source!("glob.py"),
        "bz2" => bundled_source!("bz2.py"),
        "gzip" => bundled_source!("gzip.py"),
        "ntpath" => bundled_source!("ntpath.py"),
        "shlex" => bundled_source!("shlex.py", from "shlex"),
        // CPython 3.14.5 Lib/shutil.py. See stdlib/docstrings/LICENSE.
        "shutil" => bundled_source!("shutil.py"),
        "socket" => bundled_source!("socket.py"),
        "ssl" => bundled_source!("ssl.py"),
        "tempfile" => bundled_source!("tempfile.py"),
        "doctest" => Some((
            "doctest.py",
            concat!(
                include_str!("../../stdlib/doctest.py"),
                "\n",
                include_str!("../../stdlib/doctest_boxed.py"),
            ),
        )),
        "_imre_doctest" => bundled_source!("_imre_doctest.py"),
        "unittest" => Some((
            "unittest/__init__.py",
            concat!(
                include_str!("../../stdlib/unittest/__init__.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_equal.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_raises.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_cleanup.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_suite.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_loader.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_skip.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_runner.py"),
            ),
        )),
        "unittest.__main__" => bundled_source!("unittest/__main__.py"),
        "unittest.mock" => Some((
            "unittest/mock.py",
            concat!(
                include_str!("../../stdlib/unittest/mock.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_mock.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_patch.py"),
                "\n",
                include_str!("../../stdlib/unittest/threading_mock.py"),
            ),
        )),
        "unittest.loader" => Some((
            "unittest/loader.py",
            include_str!("../../stdlib/unittest/loader_boxed.py"),
        )),
        "threading" => bundled_source!("threading.py"),
        "textwrap" => Some((
            "textwrap.py",
            // The public functions forward the caller's **kwargs to the module's TextWrapper.
            concat!(
                include_str!("../../stdlib/textwrap/wrapper.py"),
                "\n\n",
                include_str!("../../stdlib/textwrap/functions_boxed.py"),
                "\n",
            ),
        )),
        "importlib.resources._adapters" => bundled_source!("importlib/resources/_adapters.py"),
        "importlib.resources" => bundled_source!("importlib/resources/__init__.py"),
        "importlib.readers" => bundled_source!("importlib/readers.py"),
        "importlib.resources._itertools" => bundled_source!("importlib/resources/_itertools.py"),
        "importlib.resources.readers" => bundled_source!("importlib/resources/readers.py"),
        "importlib.resources.abc" => bundled_source!("importlib/resources/abc.py"),
        "importlib.resources._as_file" => bundled_source!("importlib/resources/_as_file.py"),
        "importlib.resources._filesystem" => bundled_source!("importlib/resources/_filesystem.py"),
        "importlib.util" => bundled_source!("importlib/util.py"),
        "importlib._abc" => bundled_source!("importlib/_abc.py"),
        "importlib.abc" => bundled_source!("importlib/abc.py"),
        "importlib.machinery" => bundled_source!("importlib/machinery.py"),
        "_frozen_importlib" => Some((
            "_frozen_importlib.py",
            include_str!("../../stdlib/importlib/_bootstrap.py"),
        )),
        "importlib._bootstrap" => bundled_source!("importlib/_bootstrap.py"),
        "_frozen_importlib_external" | "importlib._bootstrap_external" => {
            bundled_source!("importlib/_bootstrap_external.py")
        }
        // CPython 3.14.5 Lib/uuid.py with a public Enum declaration for SafeUUID.
        "uuid" => Some(("uuid.py", include_str!("../../stdlib/uuid_boxed.py"))),
        "asyncio" => bundled_source!("asyncio/__init__.py"),
        "asyncio.__main__" => bundled_source!("asyncio/__main__.py"),
        "asyncio.base_events" => bundled_source!("asyncio/base_events.py"),
        "asyncio.base_futures" => bundled_source!("asyncio/base_futures.py"),
        "asyncio.base_subprocess" => bundled_source!("asyncio/base_subprocess.py"),
        "asyncio.base_tasks" => bundled_source!("asyncio/base_tasks.py"),
        "asyncio.constants" => bundled_source!("asyncio/constants.py"),
        "asyncio.coroutines" => bundled_source!("asyncio/coroutines.py"),
        "asyncio.events" => bundled_source!("asyncio/events.py"),
        "asyncio.exceptions" => bundled_source!("asyncio/exceptions.py"),
        "asyncio.format_helpers" => bundled_source!("asyncio/format_helpers.py"),
        "asyncio.futures" => bundled_source!("asyncio/futures.py"),
        "asyncio.graph" => bundled_source!("asyncio/graph.py"),
        "asyncio.locks" => bundled_source!("asyncio/locks.py"),
        "asyncio.log" => bundled_source!("asyncio/log.py"),
        "asyncio.mixins" => bundled_source!("asyncio/mixins.py"),
        "asyncio.proactor_events" => bundled_source!("asyncio/proactor_events.py"),
        "asyncio.protocols" => bundled_source!("asyncio/protocols.py"),
        "asyncio.queues" => bundled_source!("asyncio/queues.py"),
        "asyncio.runners" => bundled_source!("asyncio/runners.py"),
        "asyncio.selector_events" => bundled_source!("asyncio/selector_events.py"),
        "asyncio.sslproto" => bundled_source!("asyncio/sslproto.py"),
        "asyncio.staggered" => bundled_source!("asyncio/staggered.py"),
        "asyncio.streams" => bundled_source!("asyncio/streams.py"),
        "asyncio.subprocess" => bundled_source!("asyncio/subprocess.py"),
        "asyncio.taskgroups" => bundled_source!("asyncio/taskgroups.py"),
        "asyncio.tasks" => bundled_source!("asyncio/tasks.py"),
        "asyncio.threads" => bundled_source!("asyncio/threads.py"),
        "asyncio.timeouts" => bundled_source!("asyncio/timeouts.py"),
        "asyncio.tools" => bundled_source!("asyncio/tools.py"),
        "asyncio.transports" => bundled_source!("asyncio/transports.py"),
        "asyncio.trsock" => bundled_source!("asyncio/trsock.py"),
        "asyncio.unix_events" => bundled_source!("asyncio/unix_events.py"),
        "asyncio.windows_events" => bundled_source!("asyncio/windows_events.py"),
        "asyncio.windows_utils" => bundled_source!("asyncio/windows_utils.py"),
        "concurrent" => bundled_source!("concurrent/__init__.py"),
        "concurrent.futures" => bundled_source!("concurrent/futures/__init__.py"),
        "concurrent.futures._base" => bundled_source!("concurrent/futures/_base.py"),
        "concurrent.futures.interpreter" => bundled_source!("concurrent/futures/interpreter.py"),
        "concurrent.futures.process" => bundled_source!("concurrent/futures/process.py"),
        "concurrent.futures.thread" => bundled_source!("concurrent/futures/thread.py"),
        "multiprocessing" => bundled_source!("multiprocessing/__init__.py"),
        "multiprocessing.connection" => bundled_source!("multiprocessing/connection.py"),
        "multiprocessing.context" => bundled_source!("multiprocessing/context.py"),
        "multiprocessing.dummy" => bundled_source!("multiprocessing/dummy/__init__.py"),
        "multiprocessing.dummy.connection" => {
            bundled_source!("multiprocessing/dummy/connection.py")
        }
        "multiprocessing.forkserver" => bundled_source!("multiprocessing/forkserver.py"),
        "multiprocessing.heap" => bundled_source!("multiprocessing/heap.py"),
        "multiprocessing.managers" => bundled_source!("multiprocessing/managers.py"),
        "multiprocessing.pool" => bundled_source!("multiprocessing/pool.py"),
        "multiprocessing.popen_fork" => bundled_source!("multiprocessing/popen_fork.py"),
        "multiprocessing.popen_forkserver" => {
            bundled_source!("multiprocessing/popen_forkserver.py")
        }
        "multiprocessing.popen_spawn_posix" => {
            bundled_source!("multiprocessing/popen_spawn_posix.py")
        }
        "multiprocessing.popen_spawn_win32" => {
            bundled_source!("multiprocessing/popen_spawn_win32.py")
        }
        "multiprocessing.process" => bundled_source!("multiprocessing/process.py"),
        "multiprocessing.queues" => bundled_source!("multiprocessing/queues.py"),
        "multiprocessing.reduction" => bundled_source!("multiprocessing/reduction.py"),
        "multiprocessing.resource_sharer" => bundled_source!("multiprocessing/resource_sharer.py"),
        "multiprocessing.resource_tracker" => {
            bundled_source!("multiprocessing/resource_tracker.py")
        }
        "multiprocessing.shared_memory" => bundled_source!("multiprocessing/shared_memory.py"),
        "multiprocessing.sharedctypes" => bundled_source!("multiprocessing/sharedctypes.py"),
        "multiprocessing.spawn" => bundled_source!("multiprocessing/spawn.py"),
        "multiprocessing.synchronize" => bundled_source!("multiprocessing/synchronize.py"),
        "multiprocessing.util" => bundled_source!("multiprocessing/util.py"),
        "tomllib" => bundled_source!("tomllib/__init__.py"),
        "tomllib._parser" => bundled_source!("tomllib/_parser.py"),
        "tomllib._re" => bundled_source!("tomllib/_re.py"),
        "tomllib._types" => bundled_source!("tomllib/_types.py"),
        "zipfile" => bundled_source!("zipfile/__init__.py"),
        "zipfile.__main__" => bundled_source!("zipfile/__main__.py"),
        "zipfile._path" => bundled_source!("zipfile/_path/__init__.py"),
        "zipfile._path.glob" => bundled_source!("zipfile/_path/glob.py"),
        _ => crate::modules::bundled_runtime_source(name),
    }
}

#[derive(Default)]
pub(crate) struct RegisteredImports {
    pub(super) called: bool,
    pub(super) computed: bool,
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
                } else if !self.graph.modules[self.module].bundled {
                    self.imports.computed = true;
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
    calls
        .imports
        .targets
        .extend_from_slice(&graph.doctest_imports);
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
        crate::modules::resolve_import(literal("package", 1)?.value.to_str(), level, rest)?
    };
    absolute
        .split('.')
        .all(ruff_python_stdlib::identifiers::is_identifier)
        .then_some(absolute)
}

#[cfg(test)]
mod tests {
    use super::bundled_module_source;

    #[test]
    fn inspect_compiles_the_tracked_python_verbatim() {
        let (filename, source) = bundled_module_source("inspect", false).unwrap();
        assert_eq!(filename, "inspect.py");
        assert!(source == include_str!("../../stdlib/inspect.py"));
    }
}
