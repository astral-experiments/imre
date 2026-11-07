use super::{Expr, Visitor, ast, visitor};
use crate::modules::{Binding, Intrinsic, IntrinsicModule, SourceGraph, bundled_source};

/// Prefer boxed module implementations over the common bundled sources.
pub(super) fn boxed_source(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "types" => {
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
                    let descriptor = UPSTREAM.find("class DynamicClassAttribute:").unwrap();
                    let coroutine = UPSTREAM.find("def coroutine(func):").unwrap();
                    format!(
                        "{helpers}{}\n{}\n{}",
                        include_str!("../../stdlib/types_original_bases.py"),
                        &UPSTREAM[descriptor..coroutine],
                        include_str!("../../stdlib/types_coroutine.py"),
                    )
                }),
            ))
        }
        "http.server" => {
            const UPSTREAM: &str = include_str!(
                "../../../../benchmarks/cpython_stdlib/full/http/upstream/Lib/http/server.py"
            );
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some(("http/server.py", SOURCE.get_or_init(|| {
                let start = UPSTREAM.find("class SimpleHTTPRequestHandler(").unwrap();
                let (handler, _) = UPSTREAM[start..]
                    .split_once("# Utilities for CGIHTTPRequestHandler")
                    .unwrap();
                // The existing server's _escape implements html.escape with quote=False.
                let handler = handler
                    .replace("html.escape(displaypath, quote=False)", "_escape(displaypath)")
                    .replace("html.escape(displayname, quote=False)", "_escape(displayname)");
                format!(
                    "{}\nimport datetime\nimport email.utils\nimport io\nimport mimetypes\nimport os\nimport posixpath\nimport shutil\nimport urllib.parse\n\n{handler}",
                    include_str!("../../stdlib/http/server.py")
                )
            })))
        }
        "encodings.iso8859_2" => Some((
            "encodings/iso8859_2.py",
            include_str!("../../stdlib/encodings/writer_charmap.py"),
        )),
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
        "json" => Some((
            "json/__init__.py",
            concat!(
                "r\"\"\"",
                include_str!("../../stdlib/docstrings/json.txt"),
                "\"\"\"\n",
                include_str!("../../stdlib/json/boxed_init.py"),
            ),
        )),
        "argparse" => bundled_source!("argparse.py"),
        "_json" => Some((
            "_json.py",
            concat!(
                include_str!("../../stdlib/json/accelerator_scanner.py"),
                "\n",
                include_str!("../../stdlib/json/accelerator_encoder.py"),
            ),
        )),
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
        "traceback" => Some((
            "traceback.py",
            include_str!("../../stdlib/traceback_stack.py"),
        )),
        "subprocess" if cfg!(unix) => {
            bundled_source!("subprocess.py")
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
                    // Some native descriptors implement assignment in the runtime without a
                    // Python-visible setter on their type.
                    let descriptors = section(
                        "def ismethoddescriptor(object):",
                        "if hasattr(types, 'GetSetDescriptorType'):",
                    )
                    .replace(
                        "or hasattr(tp, \"__delete__\")",
                        "or hasattr(tp, \"__delete__\") or _native_data_descriptor(object)",
                    );
                    // Keep CPython's class-creation checks and read Imre's abstract flag.
                    let abstract_predicate = section("def isabstract(object):", "def _getmembers(")
                        .replace("object.__flags__", "_abstract_flags(object)");
                    // Load the parser dependencies on first signature inspection. The boxed
                    // list iterator exposes next() but has no Python-visible __next__ method.
                    let text_signatures = section(
                        "def _signature_strip_non_python_syntax(",
                        "def _signature_from_builtin(",
                    )
                    .replace("    if not signature:", "    import token\n    import tokenize\n    if not signature:")
                    .replace("iter(lines).__next__", "functools.partial(next, iter(lines))")
                    .replace("    Parameter = cls._parameter_cls", "    import ast\n    Parameter = cls._parameter_cls");
                    // BlockFinder and getblock share tokenize. Delay its import until the
                    // first scan so importing inspect alone does not initialize the tokenizer.
                    let source_blocks = section("class EndOfBlock(", "def walktree(")
                        .replace(
                            "        self.indent = 0",
                            "        global tokenize\n        if 'tokenize' not in globals():\n            import tokenize\n        self.indent = 0",
                        )
                        .replace("iter(lines).__next__", "functools.partial(next, iter(lines))");
                    // Imre records native source positions without CPython bytecode offsets.
                    let frame_info = section(
                        "def getframeinfo(",
                        "# ------------------------------------------------ static version of getattr",
                    )
                        .replace("_get_code_position_from_tb(frame)", "_frame_position(frame)")
                        .replace("_get_code_position(frame.f_code, frame.f_lasti)", "_frame_position(frame)");
                    [
                        include_str!("../../stdlib/inspect.py"),
                        include_str!("../../stdlib/inspect_signature.py"),
                        "import abc\nimport collections.abc\nimport dis\nimport functools\nimport importlib.machinery\nimport linecache\nimport os\nimport sys\nfrom collections import namedtuple\nfrom inspect import _abstract_flags, _frame_position",
                        section("TPFLAGS_IS_ABSTRACT =", "def ismethoddescriptor(object):"),
                        &descriptors,
                        section("def isfunction(object):", "# A marker for markcoroutinefunction"),
                        section("def isasyncgenfunction(obj):", "def isabstract(object):"),
                        &abstract_predicate,
                        section("def _getmembers(", "def getmro(cls):"),
                        section("def indentsize(line):", "def _findclass(func):"),
                        section("def getfile(object):", "class ClassFoundException("),
                        section("def findsource(object):", "class EndOfBlock("),
                        &source_blocks,
                        section("AGEN_CREATED =", "_NonUserDefinedCallables ="),
                        section("_Traceback = namedtuple(", "def _get_code_position_from_tb("),
                        &frame_info,
                        section("def getmro(cls):", "def indentsize(line):"),
                        section("def _findclass(func):", "def cleandoc(doc):"),
                        section("GEN_CREATED = ", "def getgeneratorlocals("),
                        section("CORO_CREATED = ", "AGEN_CREATED ="),
                        section("def formatannotation(", "def formatannotationrelativeto("),
                        section("Arguments = namedtuple(", "def getargs(co):"),
                        section("FullArgSpec = namedtuple(", "def formatannotation("),
                        section("def formatargvalues(", "ClosureVars = namedtuple("),
                        section("_NonUserDefinedCallables =", "def _signature_get_user_defined_method("),
                        section("def _signature_get_partial(", "def _signature_strip_non_python_syntax("),
                        &text_signatures,
                        section("class _void:", "class BufferFlags("),
                    ]
                    .join("\n")
                }),
            ))
        }
        "bisect" => bundled_source!("bisect.py"),
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
        "shlex" => Some((
            "shlex.py",
            include_str!("../../../../benchmarks/cpython_stdlib/full/shlex/upstream/Lib/shlex.py"),
        )),
        // CPython 3.14.5 Lib/shutil.py. See stdlib/docstrings/LICENSE.
        "shutil" => bundled_source!("shutil.py"),
        "socket" => bundled_source!("socket.py"),
        "ssl" => bundled_source!("ssl.py"),
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
                // The frozen loader reads Imre's registered code objects and module metadata.
                let frozen = section("class FrozenImporter:", "# Import itself")
                    .replace("_imp.find_frozen", "_frozen_info")
                    .replace("_imp.get_frozen_object", "_frozen_code")
                    .replace("_imp.is_frozen_package", "_frozen_package");
                let requires_frozen = section("def _requires_frozen(", "def _load_module_shim(")
                    .replace("not _imp.is_frozen(fullname)", "_frozen_info(fullname) is None");
                // Preserve CPython's module creation hooks and its fresh/reload attribute rules.
                [
                    include_str!("../../stdlib/importlib/legacy_source_loader.py"),
                    section("def _module_repr(module):", "def _spec_from_module("),
                    section("def _init_module_attrs(", "def _exec(spec, module):"),
                    section("def _call_with_frames_removed(", "def _requires_builtin("),
                    section("def _wrap(", "def _new_module("),
                    &requires_frozen,
                    &frozen,
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
                let members: Vec<_> = include_str!(concat!(env!("OUT_DIR"), "/imre-stdlib-members.txt"))
                    .lines()
                    .collect();
                let members = serde_json::to_string(&members)
                    .expect("stdlib source paths serialize as strings");
                let installed_sources = format!("_installed_source_members = {members}");
                [
                    include_str!("../../stdlib/importlib/source_loader.py"),
                    section("_CASE_INSENSITIVE_PLATFORMS_STR_KEY =", "def _unpack_uint16("),
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
                    &installed_sources,
                    include_str!("../../stdlib/importlib/registered_finder.py"),
                    section("def _set_bootstrap_module(", "def _install("),
                ].join("\n")
            })))
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
