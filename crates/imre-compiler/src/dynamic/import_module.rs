use super::{Expr, Visitor, ast, resolve, visitor};
use crate::modules::{Binding, Intrinsic, IntrinsicModule, SourceGraph};

/// Prefer boxed module implementations over the common bundled sources.
pub(super) fn boxed_source(name: &str) -> Option<(&'static str, &'static str)> {
    if let Some(source) = stdlib_package_source(name) {
        return Some(source);
    }
    match name {
        "types" => {
            // PEP 3115/560 class construction uses only builtins; native types remain module exports.
            const UPSTREAM: &str = include_str!(
                "../../../../benchmarks/cpython_stdlib/full/types/upstream/Lib/types.py"
            );
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some((
                "types.py",
                SOURCE.get_or_init(|| {
                    let start = UPSTREAM.find("def new_class(").unwrap();
                    let (helpers, _) = UPSTREAM[start..]
                        .split_once("def get_original_bases(")
                        .unwrap();
                    format!(
                        "{helpers}{}",
                        include_str!("../../stdlib/types_original_bases.py")
                    )
                }),
            ))
        }
        "reprlib" => Some(("reprlib.py", include_str!("../../stdlib/reprlib.py"))),
        "datetime" => Some(("datetime.py", include_str!("../../stdlib/datetime.py"))),
        "enum" => Some(("enum.py", include_str!("../../stdlib/enum.py"))),
        "json" => Some((
            "json/__init__.py",
            concat!(
                "r\"\"\"",
                include_str!("../../stdlib/docstrings/json.txt"),
                "\"\"\"\n",
                include_str!("../../stdlib/json/boxed_init.py"),
            ),
        )),
        "argparse" => Some(("argparse.py", include_str!("../../stdlib/argparse.py"))),
        "_json" => Some((
            "_json.py",
            concat!(
                include_str!("../../stdlib/json/accelerator_scanner.py"),
                "\n",
                include_str!("../../stdlib/json/accelerator_encoder.py"),
            ),
        )),
        "json.encoder" => Some((
            "json/encoder.py",
            include_str!("../../stdlib/json/encoder.py"),
        )),
        "json.decoder" => Some((
            "json/decoder.py",
            include_str!("../../stdlib/json/decoder.py"),
        )),
        "json.scanner" => Some((
            "json/scanner.py",
            include_str!("../../stdlib/json/scanner.py"),
        )),
        // A fresh _collections module exports the same accelerator type. Keep its
        // Python wrappers around native storage in their own module namespace.
        "_collections" => Some((
            "_collections.py",
            "from _imre_ordered_dict import OrderedDict\n",
        )),
        "_imre_ordered_dict" => Some((
            "_imre_ordered_dict.py",
            include_str!("../../stdlib/_imre_ordered_dict.py"),
        )),
        "runpy" => Some(("runpy.py", include_str!("../../stdlib/runpy.py"))),
        "traceback" => Some((
            "traceback.py",
            include_str!("../../stdlib/traceback_stack.py"),
        )),
        "subprocess" if cfg!(unix) => {
            Some(("subprocess.py", include_str!("../../stdlib/subprocess.py")))
        }
        "linecache" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            const UPSTREAM: &str = include_str!(
                "../../../../benchmarks/cpython_stdlib/full/linecache/upstream/Lib/linecache.py"
            );
            // Keep CPython's cache, loader fallback and invalidation. The native source
            // decoder implements the disk path ordinarily provided by tokenize.open.
            Some(("linecache.py", SOURCE.get_or_init(|| {
                UPSTREAM.replace("import tokenize\n", "from _imre_linecache import source_lines\n")
                    .replace("with tokenize.open(fullname) as fp:\n            lines = fp.readlines()",
                             "with open(fullname, 'rb') as fp:\n            lines = source_lines(fp.read())")
            })))
        }
        "inspect" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some((
                "inspect.py",
                SOURCE.get_or_init(|| {
                    // Keep the full CPython Parameter, BoundArguments and Signature implementations.
                    // The runtime supplies read-only mappings and Unicode identifier validation.
                    const UPSTREAM: &str = include_str!(
                        "../../../../benchmarks/cpython_stdlib/full/inspect/upstream/Lib/inspect.py"
                    );
                    let section = |start, end| {
                        let begin = UPSTREAM.find(start).unwrap();
                        UPSTREAM[begin..].split_once(end).unwrap().0
                    };
                    let classes = section("_POSITIONAL_ONLY         = ", "class BufferFlags(")
                        .replace("name.isidentifier()", "_signature_identifier(name)")
                        .replace(
                            "types.MappingProxyType(params)",
                            "_signature_mapping(params)",
                        );
                    [
                        include_str!("../../stdlib/inspect.py"),
                        include_str!("../../stdlib/inspect_signature.py"),
                        "import functools\nimport importlib.machinery\nimport linecache\nimport os\nimport sys",
                        section("def ismodule(object):", "def ispackage(object):"),
                        section("def ismethoddescriptor(object):", "def isdatadescriptor(object):"),
                        section("def isfunction(object):", "def _has_code_flag("),
                        section("def istraceback(object):", "def isabstract(object):"),
                        section("def getfile(object):", "class ClassFoundException("),
                        section("def unwrap(func, *, stop=None):", "def indentsize(line):"),
                        section("def _signature_bound_method(", "def _signature_is_builtin("),
                        section("class _void:", "class _ParameterKind("),
                        &classes,
                    ]
                    .join("\n")
                }),
            ))
        }
        "bisect" => Some(("bisect.py", include_str!("../../stdlib/bisect.py"))),
        "email.errors" => Some((
            "email/errors.py",
            include_str!(
                "../../../../benchmarks/cpython_stdlib/full/email/upstream/Lib/email/errors.py"
            ),
        )),
        "email._encoded_words" => Some((
            "email/_encoded_words.py",
            include_str!(
                "../../../../benchmarks/cpython_stdlib/full/email/upstream/Lib/email/_encoded_words.py"
            ),
        )),
        "email._header_value_parser" => Some((
            "email/_header_value_parser.py",
            include_str!(
                "../../../../benchmarks/cpython_stdlib/full/email/upstream/Lib/email/_header_value_parser.py"
            ),
        )),
        "urllib.error" => Some((
            "urllib/error.py",
            include_str!("../../stdlib/urllib/error.py"),
        )),
        "urllib.request" => Some((
            "urllib/request.py",
            include_str!("../../stdlib/urllib/request.py"),
        )),
        "urllib.response" => Some((
            "urllib/response.py",
            include_str!("../../stdlib/urllib/response.py"),
        )),
        "urllib.parse" => Some((
            "urllib/parse.py",
            include_str!("../../stdlib/urllib/parse.py"),
        )),
        "annotationlib" => Some((
            "annotationlib.py",
            include_str!("../../stdlib/annotationlib.py"),
        )),
        "functools" => Some(("functools.py", include_str!("../../stdlib/functools.py"))),
        "sysconfig" if cfg!(target_os = "linux") => {
            Some(("sysconfig.py", include_str!("../../stdlib/sysconfig.py")))
        }
        "hashlib" => Some(("hashlib.py", include_str!("../../stdlib/hashlib.py"))),
        // CPython 3.14.5 pathlib and its glob and NT path dependencies. See stdlib/docstrings/LICENSE.
        "pathlib" => Some((
            "pathlib/__init__.py",
            include_str!("../../stdlib/pathlib/__init__.py"),
        )),
        "pathlib._local" => Some((
            "pathlib/_local.py",
            include_str!("../../stdlib/pathlib/_local.py"),
        )),
        "pathlib._os" => Some((
            "pathlib/_os.py",
            include_str!("../../stdlib/pathlib/_os.py"),
        )),
        "pathlib.types" => Some((
            "pathlib/types.py",
            include_str!("../../stdlib/pathlib/types.py"),
        )),
        "glob" => Some(("glob.py", include_str!("../../stdlib/glob.py"))),
        "ntpath" => Some(("ntpath.py", include_str!("../../stdlib/ntpath.py"))),
        // CPython 3.14.5 Lib/shutil.py. See stdlib/docstrings/LICENSE.
        "shutil" => Some(("shutil.py", include_str!("../../stdlib/shutil.py"))),
        "socket" => Some(("socket.py", include_str!("../../stdlib/socket.py"))),
        "tempfile" => Some((
            "tempfile.py",
            concat!(
                include_str!("../../stdlib/tempfile/names.py"),
                "\n",
                include_str!("../../stdlib/tempfile/boxed_spool.py"),
                "\n",
                include_str!("../../stdlib/tempfile/directory.py"),
            ),
        )),
        "doctest" => Some((
            "doctest.py",
            concat!(
                include_str!("../../stdlib/doctest.py"),
                "\n",
                include_str!("../../stdlib/doctest_boxed.py"),
            ),
        )),
        "_imre_doctest" => Some((
            "_imre_doctest.py",
            include_str!("../../stdlib/_imre_doctest.py"),
        )),
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
        "unittest.mock" => Some((
            "unittest/mock.py",
            concat!(
                include_str!("../../stdlib/unittest/mock.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_mock.py"),
                "\n",
                include_str!("../../stdlib/unittest/boxed_patch.py"),
            ),
        )),
        "unittest.loader" => Some((
            "unittest/loader.py",
            include_str!("../../stdlib/unittest/loader_boxed.py"),
        )),
        "threading" => Some(("threading.py", include_str!("../../stdlib/threading.py"))),
        "textwrap" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some(("textwrap.py", SOURCE.get_or_init(|| {
                // The boxed ABI can forward the original **kwargs, preserving omitted
                // options and dispatching construction through the module's TextWrapper.
                const WRAPPER: &str = include_str!("../../stdlib/textwrap/wrapper.py");
                const UPSTREAM: &str = include_str!(
                    "../../../../benchmarks/cpython_stdlib/full/textwrap/upstream/Lib/textwrap.py"
                );
                let (implementation, _) = WRAPPER.split_once("\ndef wrap(").unwrap();
                let first = UPSTREAM.find("\ndef wrap(").unwrap();
                let (functions, _) = UPSTREAM[first..].split_once("\nif __name__").unwrap();
                format!("{implementation}{functions}")
            })))
        }
        "importlib.resources._adapters" => Some((
            "importlib/resources/_adapters.py",
            include_str!("../../stdlib/importlib/resources/_adapters.py"),
        )),
        "importlib.resources" => Some((
            "importlib/resources/__init__.py",
            include_str!("../../stdlib/importlib/resources/__init__.py"),
        )),
        "importlib.readers" => Some((
            "importlib/readers.py",
            include_str!("../../stdlib/importlib/readers.py"),
        )),
        "importlib.resources._itertools" => Some((
            "importlib/resources/_itertools.py",
            include_str!("../../stdlib/importlib/resources/_itertools.py"),
        )),
        "importlib.resources.readers" => Some((
            "importlib/resources/readers.py",
            include_str!("../../stdlib/importlib/resources/readers.py"),
        )),
        "importlib.resources.abc" => Some((
            "importlib/resources/abc.py",
            include_str!("../../stdlib/importlib/resources/abc.py"),
        )),
        "importlib.resources._as_file" => Some((
            "importlib/resources/_as_file.py",
            include_str!("../../stdlib/importlib/resources/_as_file.py"),
        )),
        "importlib.resources._filesystem" => Some((
            "importlib/resources/_filesystem.py",
            include_str!("../../stdlib/importlib/resources/_filesystem.py"),
        )),
        "importlib.util" => Some((
            "importlib/util.py",
            include_str!("../../stdlib/importlib/util.py"),
        )),
        "importlib._abc" => Some((
            "importlib/_abc.py",
            include_str!("../../stdlib/importlib/_abc.py"),
        )),
        "importlib.abc" => Some((
            "importlib/abc.py",
            include_str!("../../stdlib/importlib/abc.py"),
        )),
        "importlib.machinery" => Some((
            "importlib/machinery.py",
            include_str!("../../stdlib/importlib/machinery.py"),
        )),
        "_frozen_importlib" | "importlib._bootstrap" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let file = if name == "_frozen_importlib" {
                "_frozen_importlib.py"
            } else {
                "importlib/_bootstrap.py"
            };
            Some((file, SOURCE.get_or_init(|| {
                const UPSTREAM: &str = include_str!(
                    "../../../../benchmarks/cpython_stdlib/full/importlib/upstream/Lib/importlib/_bootstrap.py"
                );
                let section = |start, end| {
                    let begin = UPSTREAM.find(start).unwrap();
                    UPSTREAM[begin..].split_once(end).unwrap().0
                };
                // The native warning backend is the public warnings module; load it only on emission.
                let imports = section("def _gcd_import(", "def _builtin_from_name(")
                    .replace("_warnings.warn(", "_warning_module().warn(");
                let find = section("def _find_spec(", "def _sanity_check(")
                    .replace("for finder in meta_path:", "for index, finder in enumerate(meta_path):")
                    .replace("                spec = find_spec(name, path, target)", concat!(
                        "                spec = find_spec(name, path, target)\n",
                        "                if index + 1 < len(meta_path) and meta_path[index + 1] is _BundledImporter:\n",
                        "                    spec = _prefer_bundled_spec(finder, spec, name, path, target)",
                    ))
                    .replace("with _ImportLockContext():", "with _import_lock:")
                    .replace("_warnings.warn('sys.meta_path is empty', ImportWarning)",
                             "_import_warning('sys.meta_path is empty', 1254)");
                let exec = section("def _exec(spec, module):", "def _load_backward_compatible(")
                    .replace("_warnings.warn(msg, ImportWarning)", "_import_warning(msg, 866)");
                let load = section("def _load_backward_compatible(", "# Loaders")
                    .replace("_warnings.warn(msg, ImportWarning)", "_import_warning(msg, 921)");
                // Preserve CPython's module creation hooks and its fresh/reload attribute rules.
                [
                    include_str!("../../stdlib/importlib/legacy_source_loader.py"),
                    section("class ModuleSpec:", "def _spec_from_module("),
                    section("def _init_module_attrs(", "def _module_repr_from_spec("),
                    section("def _call_with_frames_removed(", "def _requires_builtin("),
                    section("def _resolve_name(", "def _find_spec("),
                    section("def _sanity_check(", "_ERR_MSG_PREFIX ="),
                    section("def _object_name(", "# Bootstrap-related"),
                    &find,
                    &exec,
                    &load,
                    &imports,
                ].join("\n")
            })))
        }
        "_frozen_importlib_external" | "importlib._bootstrap_external" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some(("importlib/_bootstrap_external.py", SOURCE.get_or_init(|| {
                // A create_module hook can convert a file package into a namespace spec.
                const UPSTREAM: &str = include_str!(
                    "../../../../benchmarks/cpython_stdlib/full/importlib/upstream/Lib/importlib/_bootstrap_external.py"
                );
                let section = |start, end| {
                    let begin = UPSTREAM.find(start).unwrap();
                    UPSTREAM[begin..].split_once(end).unwrap().0
                };
                // Preserve FileFinder’s package, loader and cache ordering for filesystem entries.
                let spec = section("_POPULATE = object()", "def _bless_my_loader(")
                    .replace("_get_supported_file_loaders()", "_file_loader_defaults(location)");
                [
                    include_str!("../../stdlib/importlib/source_loader.py"),
                    section("_CASE_INSENSITIVE_PLATFORMS_STR_KEY =", "def _pack_uint32("),
                    section("def _unpack_uint32(", "def _unpack_uint16("),
                    section("if _MS_WINDOWS:\n    def _path_join(", "def _write_atomic("),
                    section("def _get_cached(", "def _calc_mode("),
                    section("def _classify_pyc(", "def _validate_timestamp_pyc("),
                    section("def _compile_bytecode(", "def _code_to_timestamp_pyc("),
                    section("class SourcelessFileLoader(", "class ExtensionFileLoader("),
                    &spec,
                    &section("class _NamespacePath:", "class FileFinder:")
                        .replace("_warnings.warn('sys.path_hooks is empty', ImportWarning)",
                                 "_warn_import(globals(), 'sys.path_hooks is empty', 1224, _is_frozen)"),
                    section("class FileFinder:", "class AppleFrameworkLoader("),
                    include_str!("../../stdlib/importlib/registered_finder.py"),
                    section("def _set_bootstrap_module(", "def _install("),
                ].join("\n")
            })))
        }
        _ => crate::modules::bundled_runtime_source(name),
    }
}

/// Pinned CPython 3.14.5 packages for runtime imports.
fn stdlib_package_source(name: &str) -> Option<(&'static str, &'static str)> {
    macro_rules! source {
        ($file:literal) => {
            Some(($file, include_str!(concat!("../../stdlib/", $file))))
        };
    }
    match name {
        "asyncio" => source!("asyncio/__init__.py"),
        "asyncio.__main__" => source!("asyncio/__main__.py"),
        "asyncio.base_events" => source!("asyncio/base_events.py"),
        "asyncio.base_futures" => source!("asyncio/base_futures.py"),
        "asyncio.base_subprocess" => source!("asyncio/base_subprocess.py"),
        "asyncio.base_tasks" => source!("asyncio/base_tasks.py"),
        "asyncio.constants" => source!("asyncio/constants.py"),
        "asyncio.coroutines" => source!("asyncio/coroutines.py"),
        "asyncio.events" => source!("asyncio/events.py"),
        "asyncio.exceptions" => source!("asyncio/exceptions.py"),
        "asyncio.format_helpers" => source!("asyncio/format_helpers.py"),
        "asyncio.futures" => source!("asyncio/futures.py"),
        "asyncio.graph" => source!("asyncio/graph.py"),
        "asyncio.locks" => source!("asyncio/locks.py"),
        "asyncio.log" => source!("asyncio/log.py"),
        "asyncio.mixins" => source!("asyncio/mixins.py"),
        "asyncio.proactor_events" => source!("asyncio/proactor_events.py"),
        "asyncio.protocols" => source!("asyncio/protocols.py"),
        "asyncio.queues" => source!("asyncio/queues.py"),
        "asyncio.runners" => source!("asyncio/runners.py"),
        "asyncio.selector_events" => source!("asyncio/selector_events.py"),
        "asyncio.sslproto" => source!("asyncio/sslproto.py"),
        "asyncio.staggered" => source!("asyncio/staggered.py"),
        "asyncio.streams" => source!("asyncio/streams.py"),
        "asyncio.subprocess" => source!("asyncio/subprocess.py"),
        "asyncio.taskgroups" => source!("asyncio/taskgroups.py"),
        "asyncio.tasks" => source!("asyncio/tasks.py"),
        "asyncio.threads" => source!("asyncio/threads.py"),
        "asyncio.timeouts" => source!("asyncio/timeouts.py"),
        "asyncio.tools" => source!("asyncio/tools.py"),
        "asyncio.transports" => source!("asyncio/transports.py"),
        "asyncio.trsock" => source!("asyncio/trsock.py"),
        "asyncio.unix_events" => source!("asyncio/unix_events.py"),
        "asyncio.windows_events" => source!("asyncio/windows_events.py"),
        "asyncio.windows_utils" => source!("asyncio/windows_utils.py"),
        "concurrent" => source!("concurrent/__init__.py"),
        "concurrent.futures" => source!("concurrent/futures/__init__.py"),
        "concurrent.futures._base" => source!("concurrent/futures/_base.py"),
        "concurrent.futures.interpreter" => source!("concurrent/futures/interpreter.py"),
        "concurrent.futures.process" => source!("concurrent/futures/process.py"),
        "concurrent.futures.thread" => source!("concurrent/futures/thread.py"),
        "multiprocessing" => source!("multiprocessing/__init__.py"),
        "multiprocessing.connection" => source!("multiprocessing/connection.py"),
        "multiprocessing.context" => source!("multiprocessing/context.py"),
        "multiprocessing.dummy" => source!("multiprocessing/dummy/__init__.py"),
        "multiprocessing.dummy.connection" => source!("multiprocessing/dummy/connection.py"),
        "multiprocessing.forkserver" => source!("multiprocessing/forkserver.py"),
        "multiprocessing.heap" => source!("multiprocessing/heap.py"),
        "multiprocessing.managers" => source!("multiprocessing/managers.py"),
        "multiprocessing.pool" => source!("multiprocessing/pool.py"),
        "multiprocessing.popen_fork" => source!("multiprocessing/popen_fork.py"),
        "multiprocessing.popen_forkserver" => source!("multiprocessing/popen_forkserver.py"),
        "multiprocessing.popen_spawn_posix" => source!("multiprocessing/popen_spawn_posix.py"),
        "multiprocessing.popen_spawn_win32" => source!("multiprocessing/popen_spawn_win32.py"),
        "multiprocessing.process" => source!("multiprocessing/process.py"),
        "multiprocessing.queues" => source!("multiprocessing/queues.py"),
        "multiprocessing.reduction" => source!("multiprocessing/reduction.py"),
        "multiprocessing.resource_sharer" => source!("multiprocessing/resource_sharer.py"),
        "multiprocessing.resource_tracker" => source!("multiprocessing/resource_tracker.py"),
        "multiprocessing.shared_memory" => source!("multiprocessing/shared_memory.py"),
        "multiprocessing.sharedctypes" => source!("multiprocessing/sharedctypes.py"),
        "multiprocessing.spawn" => source!("multiprocessing/spawn.py"),
        "multiprocessing.synchronize" => source!("multiprocessing/synchronize.py"),
        "multiprocessing.util" => source!("multiprocessing/util.py"),
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
        resolve(literal("package", 1)?.value.to_str(), true, level, rest)?
    };
    absolute
        .split('.')
        .all(ruff_python_stdlib::identifiers::is_identifier)
        .then_some(absolute)
}
