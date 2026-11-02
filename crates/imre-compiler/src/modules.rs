//! Loads a closed graph of first-party modules and orders their initialization.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::analysis::{CheckedDiagnostic, Session};
use crate::ir::{
    CopyOperation, FnmatchOperation, PosixPathOperation, SysEncoding, WrapperAttributes,
};
use crate::watch::FileSystem;

mod declarations;
pub(crate) mod doctest;
mod empty_keywords;
mod entry_profile;
mod namedtuple_fields;
mod ordered_imports;
mod os;
pub(crate) use os::credential_arity;
mod reachability;
pub(crate) use reachability::{definition_effect, http_command_key};
mod type_aliases;

pub(crate) type ModuleId = usize;

const KEYWORD_SOURCE: &str = include_str!("../stdlib/keyword.py");

/// Mode constants and operations from the target's POSIX stat provider.
fn stat_mode_source() -> Option<&'static str> {
    macro_rules! modes {
        ($platform:literal) => {
            concat!(
                include_str!($platform),
                "\n",
                include_str!("../stdlib/stat/common.py")
            )
        };
    }
    match std::env::consts::OS {
        "linux" => Some(modes!("../stdlib/stat/linux.py")),
        "macos" => Some(modes!("../stdlib/stat/darwin.py")),
        _ => None,
    }
}

/// Python source modules bundled for runtime imports.
pub(crate) fn bundled_runtime_source(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "unittest.runner" => Some((
            "unittest/runner.py",
            include_str!("../stdlib/unittest/runner.py"),
        )),
        "unittest.signals" => Some((
            "unittest/signals.py",
            include_str!("../stdlib/unittest/signals.py"),
        )),
        // CPython 3.14.5 Lib/unittest/_log.py. See stdlib/docstrings/LICENSE.
        "unittest._log" => Some((
            "unittest/_log.py",
            include_str!("../stdlib/unittest/_log.py"),
        )),
        // CPython 3.14.5 Lib/unittest/util.py. See stdlib/docstrings/LICENSE.
        "unittest.util" => Some((
            "unittest/util.py",
            include_str!("../stdlib/unittest/util.py"),
        )),
        "unittest.case" => Some((
            "unittest/case.py",
            include_str!("../stdlib/unittest/boxed_case.py"),
        )),
        "pickle" => Some((
            "pickle.py",
            concat!(
                include_str!("../stdlib/pickle.py"),
                "\n",
                include_str!("../stdlib/pickle_writer.py")
            ),
        )),
        // CPython 3.14.5 Lib/_colorize.py. See licenses/CPython.txt.
        "_colorize" => Some(("_colorize.py", include_str!("../stdlib/_colorize.py"))),
        // CPython 3.14.5 Lib/_strptime.py and its calendar dependency. See licenses/CPython.txt.
        "_strptime" => Some(("_strptime.py", include_str!("../stdlib/_strptime.py"))),
        "calendar" => Some(("calendar.py", include_str!("../stdlib/calendar.py"))),
        "_compat_pickle" => Some((
            "_compat_pickle.py",
            include_str!("../stdlib/_compat_pickle.py"),
        )),
        "_tokenize" => Some(("_tokenize.py", include_str!("../stdlib/_tokenize.py"))),
        "__future__" => Some(("__future__.py", include_str!("../stdlib/__future__.py"))),
        "__hello__" => Some(("__hello__.py", include_str!("../stdlib/__hello__.py"))),
        "ast" => Some(("ast.py", include_str!("../stdlib/ast.py"))),
        "stat" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let modes = stat_mode_source()?;
            Some((
                "stat.py",
                SOURCE.get_or_init(|| {
                    format!("{modes}\n{}", include_str!("../stdlib/stat/boxed.py"))
                }),
            ))
        }
        "zipimport" if cfg!(unix) => Some((
            "zipimport.py",
            concat!(
                include_str!("../stdlib/zipimport_boxed.py"),
                "\n",
                include_str!("../stdlib/zipimport.py"),
                "\n",
                include_str!("../stdlib/zipimport_source.py"),
            ),
        )),
        "importlib.metadata._files" => Some((
            "importlib/metadata/_files.py",
            include_str!("../stdlib/importlib/metadata_files.py"),
        )),
        "importlib.metadata._adapters" => Some((
            "importlib/metadata/_adapters.py",
            include_str!("../stdlib/importlib/metadata_adapters.py"),
        )),
        "importlib.metadata" => Some((
            "importlib/metadata/__init__.py",
            concat!(
                include_str!("../stdlib/importlib/metadata_boxed.py"),
                "\n",
                include_str!("../stdlib/importlib/metadata_version.py"),
                "\n",
                include_str!("../stdlib/importlib/metadata_entrypoints.py"),
            ),
        )),
        // CPython 3.14.5 Lib/queue.py with condition variables backed by native locks.
        "_imre_queue_sync" => Some((
            "_imre_queue_sync.py",
            include_str!("../stdlib/_imre_queue_sync.py"),
        )),
        "queue" => Some(("queue.py", include_str!("../stdlib/queue.py"))),
        "csv" => Some((
            "csv.py",
            concat!(
                include_str!("../stdlib/csv/dialect.py"),
                "\n",
                include_str!("../stdlib/csv/writer.py"),
                "\n",
                include_str!("../stdlib/csv/reader.py"),
                "\n",
                include_str!("../stdlib/csv/dict.py"),
                "\n",
                include_str!("../stdlib/csv/sniffer.py"),
            ),
        )),
        "copy" => Some(("copy.py", include_str!("../stdlib/copy.py"))),
        "copyreg" => Some(("copyreg.py", include_str!("../stdlib/copyreg.py"))),
        "dis" => Some(("dis.py", include_str!("../stdlib/dis.py"))),
        "opcode" => Some(("opcode.py", include_str!("../stdlib/opcode.py"))),
        "_opcode_metadata" => Some((
            "_opcode_metadata.py",
            include_str!("../stdlib/_opcode_metadata.py"),
        )),
        "warnings" => Some((
            "warnings.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/warnings/upstream/Lib/warnings.py"
            ),
        )),
        "_py_warnings" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some(("_py_warnings.py", SOURCE.get_or_init(|| {
                const UPSTREAM: &str = include_str!(
                    "../../../benchmarks/cpython_stdlib/full/warnings/upstream/Lib/_py_warnings.py"
                );
                // Native import warnings may be emitted while all imports are disabled.
                // Dispatch their WarningMessage without priming the Python line cache.
                UPSTREAM
                    .replace("def warn_explicit(", "_native_import_warning = object()\n\ndef warn_explicit(")
                    .replace("    import linecache\n    linecache.getlines(filename, module_globals)",
                             "    if module_globals is not _native_import_warning:\n        import linecache\n        linecache.getlines(filename, module_globals)")
            })))
        }
        "re" => Some(("re.py", include_str!("../stdlib/re.py"))),
        "decimal" => Some(("decimal.py", include_str!("../stdlib/decimal.py"))),
        // CPython 3.14.5 Lib/py_compile.py. See stdlib/docstrings/LICENSE.
        "py_compile" => Some(("py_compile.py", include_str!("../stdlib/py_compile.py"))),
        // CPython 3.14.5 Lib/pprint.py. See stdlib/docstrings/LICENSE.
        "pprint" => Some(("pprint.py", include_str!("../stdlib/pprint.py"))),
        "difflib" => Some((
            "difflib.py",
            include_str!("../../../benchmarks/cpython_stdlib/full/difflib/upstream/Lib/difflib.py"),
        )),
        "heapq" => Some((
            "heapq.py",
            include_str!("../../../benchmarks/cpython_stdlib/full/heapq/upstream/Lib/heapq.py"),
        )),
        "importlib" => Some((
            "importlib/__init__.py",
            include_str!("../stdlib/importlib/__init__.py"),
        )),
        "os" => Some(("os.py", include_str!("../stdlib/os.py"))),
        "genericpath" => Some(("genericpath.py", include_str!("../stdlib/genericpath.py"))),
        "posixpath" => Some(("posixpath.py", include_str!("../stdlib/posixpath.py"))),
        "pkgutil" => Some((
            "pkgutil.py",
            concat!(
                include_str!("../stdlib/pkgutil/boxed.py"),
                "\n",
                include_str!("../stdlib/pkgutil/iter_modules.py"),
            ),
        )),
        "_collections_abc" => Some((
            "_collections_abc.py",
            include_str!("../stdlib/_collections_abc.py"),
        )),
        "unittest.mock" => Some((
            "unittest/mock.py",
            concat!(
                include_str!("../stdlib/unittest/mock.py"),
                "\n",
                include_str!("../stdlib/unittest/mock_calls.py"),
            ),
        )),
        // CPython 3.14.5 Lib/weakref.py and Lib/_weakrefset.py.
        "weakref" => Some(("weakref.py", include_str!("../stdlib/weakref.py"))),
        "_weakrefset" => Some(("_weakrefset.py", include_str!("../stdlib/_weakrefset.py"))),
        "abc" => Some(("abc.py", include_str!("../stdlib/abc.py"))),
        "base64" => Some(("base64.py", include_str!("../stdlib/base64.py"))),
        // CPython 3.14.5 Lib/code.py and Lib/codeop.py. See stdlib/docstrings/LICENSE.
        "code" => Some(("code.py", include_str!("../stdlib/code.py"))),
        "codecs" => Some(("codecs.py", include_str!("../stdlib/codecs.py"))),
        "encodings" => Some((
            "encodings/__init__.py",
            include_str!("../stdlib/encodings/__init__.py"),
        )),
        "encodings.ascii" => Some((
            "encodings/ascii.py",
            include_str!("../stdlib/encodings/writer_ascii.py"),
        )),
        "encodings.latin_1" => Some((
            "encodings/latin_1.py",
            include_str!("../stdlib/encodings/writer_latin_1.py"),
        )),
        "encodings.utf_8" => Some((
            "encodings/utf_8.py",
            include_str!("../stdlib/encodings/writer_utf_8.py"),
        )),
        "encodings.utf_8_sig" => Some((
            "encodings/utf_8_sig.py",
            include_str!("../stdlib/encodings/writer_utf_8_sig.py"),
        )),
        "encodings.cp1251" => Some((
            "encodings/cp1251.py",
            include_str!("../stdlib/encodings/writer_charmap.py"),
        )),
        "encodings.cp424" => Some((
            "encodings/cp424.py",
            include_str!("../stdlib/encodings/writer_charmap.py"),
        )),
        "encodings.cp437" => Some((
            "encodings/cp437.py",
            include_str!("../stdlib/encodings/writer_charmap.py"),
        )),
        "codeop" => Some(("codeop.py", include_str!("../stdlib/codeop.py"))),
        "keyword" => Some(("keyword.py", KEYWORD_SOURCE)),
        "token" => Some(("token.py", include_str!("../stdlib/token.py"))),
        "collections" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some(("collections/__init__.py", SOURCE.get_or_init(|| {
                const UPSTREAM: &str = include_str!(
                    "../../../benchmarks/cpython_stdlib/full/collections/upstream/Lib/collections/__init__.py"
                );
                let (_, wrappers) = UPSTREAM
                    .split_once("### UserDict\n")
                    .expect("CPython 3.14.5 UserDict section");
                let (ordered, _) = UPSTREAM
                    .split_once("### OrderedDict\n")
                    .and_then(|(_, tail)| tail.split_once("### namedtuple\n"))
                    .expect("CPython 3.14.5 OrderedDict section");
                // Define the Python class in the fresh module's globals and let the
                // upstream module select _collections.OrderedDict when it is available.
                concat!(
                    include_str!("../stdlib/collections/__init__.py"),
                    "\n",
                    include_str!("../stdlib/collections/namedtuple_repr.py"),
                    "\n",
                    include_str!("../stdlib/collections/recursive_repr.py"),
                    "\nimport _collections_abc\nimport sys as _sys\n",
                    "from _weakref import proxy as _proxy\nfrom operator import eq as _eq\n",
                ).to_owned() + ordered + wrappers
            })))
        }
        // CPython 3.14.5 Lib/configparser.py.
        "configparser" => Some(("configparser.py", include_str!("../stdlib/configparser.py"))),
        "contextlib" => Some((
            "contextlib.py",
            concat!(
                include_str!("../stdlib/contextlib.py"),
                "\n",
                include_str!("../stdlib/contextlib_abstract_async.py"),
                "\n",
                include_str!("../stdlib/contextlib_nullcontext.py"),
                "\n",
                include_str!("../stdlib/contextlib_exitstack.py"),
                "\n",
                include_str!("../stdlib/contextlib_redirect.py"),
                "\n",
                include_str!("../stdlib/contextlib_closing.py"),
                "\n",
                include_str!("../stdlib/contextlib_contextmanager.py"),
                "\n",
                include_str!("../stdlib/contextlib_suppress.py")
            ),
        )),
        "_imre_socketserver_poll" => Some((
            "_imre_socketserver_poll.py",
            if cfg!(any(target_os = "linux", target_os = "macos")) {
                include_str!("../stdlib/_socketserver_poll.py")
            } else {
                include_str!("../stdlib/_socketserver_no_poll.py")
            },
        )),
        "_imre_socketserver_reader" => Some((
            "_imre_socketserver_reader.py",
            include_str!("../stdlib/_socketserver_reader.py"),
        )),
        "inspect" => Some(("inspect.py", include_str!("../stdlib/inspect.py"))),
        "itertools" => Some(("itertools.py", include_str!("../stdlib/itertools.py"))),
        "signal" if cfg!(unix) => Some(("signal.py", include_str!("../stdlib/signal.py"))),
        "typing" => Some(("typing.py", include_str!("../stdlib/typing.py"))),
        "logging" => Some((
            "logging/__init__.py",
            concat!(
                include_str!("../stdlib/logging/records.py"),
                "\n",
                include_str!("../stdlib/logging/formatters.py"),
                "\n",
                include_str!("../stdlib/logging/boxed_handler.py"),
                "\n",
                include_str!("../stdlib/logging/stream.py.inc"),
                "\n",
                include_str!("../stdlib/logging/boxed_stream.py.inc"),
                "\n",
                include_str!("../stdlib/logging/boxed.py"),
            ),
        )),
        // CPython 3.14.5 Lib/logging/config.py.
        "logging.config" => Some((
            "logging/config.py",
            include_str!("../stdlib/logging/config.py"),
        )),
        // CPython 3.14.5 Lib/logging/handlers.py.
        "logging.handlers" => Some((
            "logging/handlers.py",
            include_str!("../stdlib/logging/boxed_handlers.py"),
        )),
        "platform" => Some((
            "platform.py",
            concat!(
                include_str!("../stdlib/platform/version.py"),
                "\n",
                include_str!("../stdlib/platform/metadata.py"),
            ),
        )),
        // CPython 3.14.5 Lib/ftplib.py. See stdlib/docstrings/LICENSE.
        "ftplib" if cfg!(unix) => Some(("ftplib.py", include_str!("../stdlib/ftplib.py"))),
        "mimetypes" if cfg!(unix) => Some((
            "mimetypes.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/mimetypes/upstream/Lib/mimetypes.py"
            ),
        )),
        "socketserver" => Some((
            "socketserver.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/socketserver/upstream/Lib/socketserver.py"
            ),
        )),
        // CPython 3.14.5 Lib/string/__init__.py, blob eab5067c9b133ea1210efacd92eb5f69c2336236.
        "string" => Some((
            "string/__init__.py",
            include_str!("../stdlib/string/cpython.py"),
        )),
        "string.templatelib" => Some((
            "string/templatelib.py",
            include_str!("../stdlib/string/templatelib.py"),
        )),
        "email.message" => Some((
            "email/message.py",
            concat!(
                include_str!("../stdlib/email/message.py"),
                "\n",
                include_str!("../stdlib/email/boxed_message.py"),
            ),
        )),
        "http" => Some((
            "http/__init__.py",
            include_str!("../stdlib/http/__init__.py"),
        )),
        "http.client" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            Some((
                "http/client.py",
                SOURCE.get_or_init(|| {
                    const UPSTREAM: &str = include_str!(
                        "../../../benchmarks/cpython_stdlib/full/http/upstream/Lib/http/client.py"
                    );
                    let transport = UPSTREAM
                        .replace("import email.parser\n", "")
                        // Match the same invalid newline bytes without regex lookahead.
                        .replace(
                            r"\n(?![ \t])|\r(?![ \t\n])",
                            r"\n[^ \t]|\r[^ \t\n]|[\n\r]\Z",
                        );
                    concat!(
                        include_str!("../stdlib/http/client.py"),
                        "\n_parse_http_fields = HTTPMessage._parse\n",
                    )
                    .to_owned()
                        + &transport
                        + include_str!("../stdlib/http/boxed_headers.py")
                }),
            ))
        }
        "http.server" => Some(("http/server.py", include_str!("../stdlib/http/server.py"))),
        "locale" => Some(("locale.py", include_str!("../stdlib/locale.py"))),
        "_imre_gettext_locale" => Some((
            "_imre_gettext_locale.py",
            include_str!("../stdlib/_imre_gettext_locale.py"),
        )),
        "getopt" => Some(("getopt.py", include_str!("../stdlib/getopt.py"))),
        "gettext" => Some(("gettext.py", include_str!("../stdlib/gettext.py"))),
        "uuid" => Some(("uuid.py", include_str!("../stdlib/uuid.py"))),
        _ => None,
    }
}

/// Spell filesystem import suffixes in longest-first order for both pkgutil frontends.
pub(crate) fn pkgutil_suffixes() -> String {
    let version = crate::PYTHON_VERSION;
    let abi = if cfg!(target_os = "macos") {
        "darwin".to_owned()
    } else {
        let libc = if cfg!(target_env = "musl") {
            "musl"
        } else {
            "gnu"
        };
        format!("{}-linux-{libc}", std::env::consts::ARCH)
    };
    let tagged = format!(".cpython-{}{}-{abi}.so", version.major, version.minor);
    let archive = format!(".{}", crate::NATIVE_ARCHIVE_EXTENSION);
    serde_json::to_string(&[&tagged, ".abi3.so", &archive, ".so", ".py", ".pyc"]).unwrap()
}

// CPython's built-in/frozen lookup precedes first-party files. These internal and
// frozen test modules have no corresponding stubs in the pinned typeshed inventory.
const INTRINSIC_MODULES: &[&str] = &[
    "__main__",
    "__hello__",
    "__hello_alias__",
    "__hello_only__",
    "__phello__",
    "__phello_alias__",
    "_hmac",
    "_suggestions",
    "_sysconfig",
    "_types",
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DefinitionId {
    pub(crate) module: ModuleId,
    pub(crate) statement: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Binding {
    Module(ModuleId),
    Definition(DefinitionId),
    /// The module initializer establishes this binding when its guard executes.
    ConditionalValue,
    Value(DefinitionId),
    TypeAlias(ModuleId, usize),
    IntrinsicModule(IntrinsicModule),
    Intrinsic(Intrinsic),
}

/// Sealed namespaces expose only compiler-known symbols, never module values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicModule {
    ContextVars,
    ContextVarsNative,
    Importlib,
    ImportlibAbc,
    ImportlibLoaderAbc,
    ImportlibMachinery,
    ImportlibBootstrapExternal,
    ImportlibMetadata,
    ImportlibReaders,
    ImportlibResources,
    ImportlibResourceReaders,
    ImportlibResourcesAbc,
    ImportlibUtil,
    Array,
    Imp,
    Ast,
    Atexit,
    Marshal,
    Gc,
    Pickle,
    Struct,
    AnnotationLib,
    AnnotationFormat,
    Declaration(&'static str),
    Argparse,
    Errno,
    Future,
    Builtins,
    Sys,
    SysJit,
    Traceback,
    Typing,
    Dataclasses,
    Difflib,
    DoctestRuntime,
    Select,
    MockRuntime,
    Logging,
    LoggingConfig,
    LoggingHandlers,
    Datetime,
    DatetimeNative,
    Abc,
    Asyncio,
    Contextlib,
    Inspect,
    Linecache,
    Locale,
    LocaleNative,
    Functools,
    Fnmatch,
    Time,
    Warnings,
    Tomli,
    TomliW,
    Shutil,
    Resource,
    Fcntl,
    Random,
    Secrets,
    Sysconfig,
    Signal,
    SignalNative,
    Opcode,
    Unidiff,
    Pathlib,
    Shlex,
    Tempfile,
    Subprocess,
    Selectors,
    Os,
    Posix,
    OsPath,
    GenericPath,
    PosixPath,
    Urllib,
    UrllibParse,
    UrllibRequest,
    UrllibError,
    Email,
    EmailHeaderValueParser,
    EmailUtils,
    Collections,
    CollectionsNative,
    CollectionsAbc,
    Copy,
    Itertools,
    Math,
    Numbers,
    Operator,
    Textwrap,
    Io,
    IoNative,
    Gzip,
    UnicodeData,
    WeakRef,
    Threading,
    Thread,
    Codecs,
    CodecsNative,
    Socket,
    SocketNative,
    Hashlib,
    Blake2,
    Sha3,
    Md5,
    Hmac,
    Types,
    Base64,
    Binascii,
    Zipfile,
    Csv,
    StringFormatting,
    Re,
    Enum,
    Zlib,
    Json,
    JsonNative,
    JsonEncoder,
    JsonDecoder,
    Imre,
    ImreJson,
    ImreToml,
    Tomllib,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic {
    Context,
    ContextVar,
    ContextToken,
    CopyContext,
    ModuleSpec,
    CachePath {
        reverse: bool,
    },
    NominalClass(&'static str),
    Number,
    GetAnnotations,
    Overload,
    GetOverloads,
    OverloadRegistry,
    OverloadDummy,
    ClearOverloads,
    ForwardRef,
    AnnotationTypeRepr,
    AnnotationFormat(i64),
    Declaration(&'static str),
    BuiltinsLast,
    BuiltinsLen,
    BuiltinsAscii,
    BuiltinsHex,
    SysDisplayHook,
    ArgumentParser,
    ArgparseNamespace,
    ArgparseSuppress,
    BooleanOptionalAction,
    RawDescriptionHelpFormatter,
    FutureAnnotations,
    ShallowCopy,
    DeepCopy,
    TypeChecking,
    GetLogger,
    GetLevelName,
    LoggingDisable,
    AddLevelName,
    LevelNamesMapping,
    HandlerRename,
    HandlerLock,
    HandlerLookup,
    FileEncodingUsesUtf8,
    PrintActiveException,
    TracebackPrintExc,
    TracebackFormatter(&'static str),
    HandlerLevelType,
    TextOutputType,
    HandlerFallback,
    LevelMap {
        names: bool,
    },
    LogRecord,
    MakeLogRecord,
    LogFormatter,
    ClockConverter(bool),
    StructTime,
    Mktime,
    LogRecordFlag(u8),
    BasicConfig,
    DictConfig,
    FileConfig,
    LogLevel(i64),
    Abc,
    AbcMeta,
    AbstractMethod,
    CreateSubprocessExec,
    RunLoop,
    NewLoop,
    SetLoop,
    EnsureTask,
    CancelledError,
    Gather,
    TaskGroup,
    Semaphore,
    ContextManager,
    Linecache(crate::ir::LinecacheOperation),
    InspectGetattrStatic,
    InspectIsMethodDescriptor,
    InspectCoroutine {
        mark: bool,
    },
    InspectSentinel,
    WrapperAttributes(WrapperAttributes),
    FunctionCache,
    CacheInfo,
    LruCache,
    SingleDispatch,
    UpdateWrapper,
    Wraps,
    CachedProperty,
    NewType,
    Partial,
    Placeholder,
    Reduce,
    CmpToKey,
    TomlDumps,
    TomliLoads,
    TomliLoad,
    TomliDump,
    StandardEnum,
    IsDataclass,
    Pipe,
    Devnull,
    CheckOutput,
    ShellOutput {
        status: bool,
    },
    NullContext,
    IntConstant(i64),
    WallTime,
    TimeSleep,
    WarningFilter {
        simple: bool,
    },
    WarningReset,
    CatchWarnings,
    WarningDisplay {
        show: bool,
    },
    PatchSet,
    PatchedFile,
    Hunk,
    UnidiffParseError,
    Awaitable,
    TypeVar,
    Stderr,
    Stdout,
    OriginalStdout,
    SysExecutable,
    SysFlags,
    SysDontWriteBytecode,
    SysImplementation,
    SysPycachePrefix,
    SysPlatform,
    SysVersionInfo,
    SysThreadInfo,
    SysStdlibModuleNames,
    SysVersion,
    SysHexversion,
    SysEncoding(SysEncoding),
    SysIntern {
        query: bool,
    },
    SysException,
    SysGetFrameModuleName,
    SysGetRecursionLimit,
    SysSetRecursionLimit,
    SysJitPredicate(&'static str),
    Stdin,
    OsName,
    OsLinesep,
    OsPathsep {
        posix: bool,
    },
    OsSeparator {
        alternate: bool,
        posix: bool,
    },
    ErrnoAttribute,
    ErrnoFrom(usize),
    ModuleType,
    GetSysconfigPath,
    OsPathExists,
    StringConstant(&'static str),
    SysExit,
    ShlexSplit,
    ShlexQuote,
    ShlexJoin,
    Ndiff,
    DocCaptureStart,
    DocCaptureTake,
    DocDisplay,
    DocAscii,
    MockCallList {
        mark: bool,
    },
    DocExecute,
    Final,
    Argv,
    OrigArgv,
    Logger,
    LogManager,
    LogRoot,
    LogEmit(Option<u8>),
    TimedFileHandler,
    ClockTime,
    Calendar(crate::ir::CalendarType),
    CalendarUtc,
    TextIO,
    BinaryIO,
    Union,
    Literal,
    Callable,
    Mapping,
    Iterable,
    Iterator,
    UnsupportedOperation,
    GzipOpen,
    UrlsafeB64encode,
    Base64Encode,
    Base64Decode,
    BinasciiDecode,
    BinasciiEncode,
    BinasciiUnhexlify(&'static str),
    BinasciiError,
    BinasciiIncomplete,
    PackageNotFoundError,
    RegisteredImport(&'static str),
    Socket,
    GaiError,
    HError,
    SocketGetFqdn,
    SocketConstant(&'static str),
    Sha1,
    Sha256,
    NamedHash(&'static str),
    UnicodeNormalize {
        check: bool,
    },
    ObjectRuntimeExport(&'static str),
    /// Bind the object provider when this export is used, after checking typed calls.
    ScopedObjectExport(&'static str),
    WeakProxy,
    WeakProxyClass {
        callable: bool,
    },
    CodecLookup,
    CodecGetWriter,
    CodecBom(&'static [u8]),
    HashNew,
    HashFileDigest,
    HmacCompareDigest,
    SimpleNamespace,
    HashAlgorithms {
        guaranteed: bool,
    },
    ZipFile,
    ZipInfo,
    BytesIo,
    StringIo,
    BadZipFile,
    BadGzipFile,
    CsvReader,
    FormatterParser,
    FormatterFieldNameSplit,
    CsvError,
    RegexCompile,
    RegexSearch,
    RegexMatchStart,
    RegexSub,
    RegexVerbose,
    RegexUnicode,
    RegexPattern,
    RegexMatch,
    StrEnum,
    IntEnum,
    SimpleEnum,
    EnumAuto,
    DefaultDict,
    ChainMap,
    OrderedDict,
    NamedTuple,
    Counter,
    CountElements,
    Deque,
    Repeat,
    AttrGetter,
    LengthHint,
    MathProd,
    Count,
    Islice,
    Combinatorial(crate::ir::Combinatorial),
    Tee,
    Cycle,
    GroupBy,
    Chain,
    AbstractSet,
    TomllibLoad,
    TomlDecodeError,
    Dedent,
    ZlibError,
    Adler32,
    JsonDecode,
    JsonEncode,
    JsonDumps,
    JsonDump,
    JsonLoads,
    JsonDecoderClass,
    JsonEncoderClass,
    JsonLoad,
    JsonDecodeError,
    Any,
    TomlDecode,
    Dataclass,
    Struct,
    DataclassField,
    AsDict,
    SelfType,
    ClassVar,
    AssertNever,
    Cast,
    Path,
    PurePath(crate::ir::PathFlavor),
    Which,
    Copy(CopyOperation),
    RmTree,
    CopyTree,
    Move,
    DestInSrc,
    IgnorePatterns,
    ShutilError,
    RandomChoice {
        secure: bool,
    },
    Random(crate::ir::RandomOperation),
    SameFileError,
    SpecialFileError,
    TemporaryDirectory,
    TemporaryFile,
    Popen,
    PopenActive,
    PopenCleanup,
    PopenSelector,
    SelectorClass(bool),
    SubprocessRun,
    SubprocessCall,
    SubprocessCheckCall,
    SubprocessList2Cmdline,
    SubprocessStdout,
    CompletedProcess,
    SubprocessError,
    CalledProcessError,
    TimeoutExpired,
    AccessMode(u8),
    Environ,
    EnvironBytes,
    BoolConstant(bool),
    GetEnv,
    GetCwd,
    Chdir,
    Strerror,
    StatResult,
    Fd(crate::ir::FdOperation),
    Credential(&'static str),
    Readiness(crate::ir::Readiness),
    DescriptorConstant(&'static str),
    FsPath,
    FsDecode,
    FsEncode,
    PosixNormcase,
    PosixPath(PosixPathOperation),
    Fnmatch(FnmatchOperation),
    Quote,
    QuoteFromBytes,
    QuotePlus,
    Unquote,
    UrlEncode,
    UrlSplit,
    UrlUnsplit,
    UrlJoin,
    ParseQsl,
    EmailParseDate,
    EmailFormatDateTime,
    EmailFormatDate,
    SplitResult {
        bytes: bool,
    },
}

impl Intrinsic {
    pub(crate) const SOCKETSERVER_CLASS: Self = Self::ScopedObjectExport("socketserver classes");
    pub(crate) const PICKLER: Self = Self::ObjectRuntimeExport("pickle.Pickler");
    pub(crate) const IO_BASE: Self = Self::ObjectRuntimeExport("io.IOBase");
    pub(crate) const RAW_IO_BASE: Self = Self::ObjectRuntimeExport("io.RawIOBase");
    pub(crate) const BUFFERED_IO_BASE: Self = Self::ObjectRuntimeExport("io.BufferedIOBase");
    pub(crate) const TEXT_IO_BASE: Self = Self::ObjectRuntimeExport("io.TextIOBase");
    pub(crate) const TYPING_PROTOCOL: Self = Self::ObjectRuntimeExport("typing.Protocol");
    pub(crate) const TYPING_MAPPING: Self = Self::ObjectRuntimeExport("typing.Mapping");
    pub(crate) const TYPING_SEQUENCE: Self = Self::ObjectRuntimeExport("typing.Sequence");

    /// Subclasses of these immutable types keep their calendar value in the object runtime.
    pub(crate) fn is_subclassable_calendar(self) -> bool {
        use crate::ir::CalendarType::{Date, DateTime, Delta};
        matches!(
            self,
            Self::Calendar(Date | DateTime | Delta) | Self::ClockTime
        )
    }

    /// Resolve the actual public export, including when the caller imported it under an alias.
    pub(crate) fn exported_by(self, module: &str, name: &str) -> bool {
        IntrinsicModule::named(module).and_then(|m| m.symbol(name))
            == Some(Binding::Intrinsic(self))
    }
}

impl IntrinsicModule {
    /// Ty resolves these builtin names solely against its bundled stdlib. Only
    /// the exact import proved by the closed checker needs an import diagnostic.
    pub(crate) fn lacks_builtin_stub(self) -> bool {
        matches!(
            self,
            Self::Opcode | Self::StringFormatting | Self::CollectionsNative | Self::DatetimeNative
        )
    }

    fn named(name: &str) -> Option<Self> {
        match name {
            "array" => Some(Self::Array),
            "_imp" => Some(Self::Imp),
            "ast" => Some(Self::Ast),
            "atexit" => Some(Self::Atexit),
            "marshal" => Some(Self::Marshal),
            "gc" => Some(Self::Gc),
            "pickle" => Some(Self::Pickle),
            "struct" => Some(Self::Struct),
            "annotationlib" => Some(Self::AnnotationLib),
            "__future__" => Some(Self::Future),
            "logging" => Some(Self::Logging),
            "logging.config" => Some(Self::LoggingConfig),
            "logging.handlers" => Some(Self::LoggingHandlers),
            "datetime" => Some(Self::Datetime),
            "_datetime" => Some(Self::DatetimeNative),
            "abc" => Some(Self::Abc),
            "asyncio" => Some(Self::Asyncio),
            "contextvars" => Some(Self::ContextVars),
            "_contextvars" => Some(Self::ContextVarsNative),
            "inspect" => Some(Self::Inspect),
            "linecache" => Some(Self::Linecache),
            "locale" => Some(Self::Locale),
            "_locale" => Some(Self::LocaleNative),
            "contextlib" => Some(Self::Contextlib),
            "functools" => Some(Self::Functools),
            "fnmatch" => Some(Self::Fnmatch),
            "time" => Some(Self::Time),
            "warnings" => Some(Self::Warnings),
            "tomli" => Some(Self::Tomli),
            "tomli_w" => Some(Self::TomliW),
            "shutil" => Some(Self::Shutil),
            "fcntl" if cfg!(target_os = "linux") => Some(Self::Fcntl),
            "resource" if cfg!(target_os = "linux") => Some(Self::Resource),
            "random" => Some(Self::Random),
            "secrets" => Some(Self::Secrets),
            "sysconfig" => Some(Self::Sysconfig),
            "signal" => Some(Self::Signal),
            "_signal" if cfg!(unix) => Some(Self::SignalNative),
            "_opcode" => Some(Self::Opcode),
            "unidiff" => Some(Self::Unidiff),
            "argparse" => Some(Self::Argparse),
            "builtins" => Some(Self::Builtins),
            "sys" => Some(Self::Sys),
            "traceback" => Some(Self::Traceback),
            "typing" => Some(Self::Typing),
            "dataclasses" => Some(Self::Dataclasses),
            "difflib" => Some(Self::Difflib),
            "select" if cfg!(any(target_os = "linux", target_os = "macos")) => Some(Self::Select),
            "_imre_doctest" => Some(Self::DoctestRuntime),
            "_imre_mock" => Some(Self::MockRuntime),
            "pathlib" => Some(Self::Pathlib),
            "shlex" => Some(Self::Shlex),
            "tempfile" => Some(Self::Tempfile),
            "subprocess" => Some(Self::Subprocess),
            "selectors" => Some(Self::Selectors),
            "os" => Some(Self::Os),
            "posix" if cfg!(unix) => Some(Self::Posix),
            "os.path" => Some(Self::OsPath),
            "genericpath" => Some(Self::GenericPath),
            "posixpath" => Some(Self::PosixPath),
            "email" => Some(Self::Email),
            "email._header_value_parser" => Some(Self::EmailHeaderValueParser),
            "email.utils" => Some(Self::EmailUtils),
            "urllib" => Some(Self::Urllib),
            "urllib.parse" => Some(Self::UrllibParse),
            "urllib.request" => Some(Self::UrllibRequest),
            "urllib.error" => Some(Self::UrllibError),
            "importlib" => Some(Self::Importlib),
            "importlib.abc" => Some(Self::ImportlibAbc),
            "importlib._abc" => Some(Self::ImportlibLoaderAbc),
            "importlib.machinery" => Some(Self::ImportlibMachinery),
            "_frozen_importlib_external" | "importlib._bootstrap_external" => {
                Some(Self::ImportlibBootstrapExternal)
            }
            "importlib.metadata" => Some(Self::ImportlibMetadata),
            "importlib.readers" => Some(Self::ImportlibReaders),
            "importlib.resources" => Some(Self::ImportlibResources),
            "importlib.resources.readers" => Some(Self::ImportlibResourceReaders),
            "importlib.resources.abc" => Some(Self::ImportlibResourcesAbc),
            "importlib.util" => Some(Self::ImportlibUtil),
            "collections" => Some(Self::Collections),
            "_collections" => Some(Self::CollectionsNative),
            "collections.abc" => Some(Self::CollectionsAbc),
            "copy" => Some(Self::Copy),
            "itertools" => Some(Self::Itertools),
            "math" => Some(Self::Math),
            "numbers" => Some(Self::Numbers),
            "operator" => Some(Self::Operator),
            "textwrap" => Some(Self::Textwrap),
            "io" => Some(Self::Io),
            "_io" => Some(Self::IoNative),
            "gzip" => Some(Self::Gzip),
            "types" => Some(Self::Types),
            "unicodedata" => Some(Self::UnicodeData),
            "weakref" => Some(Self::WeakRef),
            "threading" => Some(Self::Threading),
            "_thread" => Some(Self::Thread),
            "codecs" => Some(Self::Codecs),
            "_codecs" => Some(Self::CodecsNative),
            "socket" => Some(Self::Socket),
            "_socket" => Some(Self::SocketNative),
            "hashlib" => Some(Self::Hashlib),
            "_blake2" => Some(Self::Blake2),
            "_sha3" => Some(Self::Sha3),
            "_md5" => Some(Self::Md5),
            "hmac" => Some(Self::Hmac),
            "base64" => Some(Self::Base64),
            "binascii" => Some(Self::Binascii),
            "zipfile" => Some(Self::Zipfile),
            "csv" => Some(Self::Csv),
            "_string" => Some(Self::StringFormatting),
            "re" => Some(Self::Re),
            "enum" => Some(Self::Enum),
            "zlib" => Some(Self::Zlib),
            "json" => Some(Self::Json),
            "_json" => Some(Self::JsonNative),
            "json.encoder" => Some(Self::JsonEncoder),
            "json.decoder" => Some(Self::JsonDecoder),
            "imre" => Some(Self::Imre),
            "imre.json" => Some(Self::ImreJson),
            "imre.toml" => Some(Self::ImreToml),
            "errno" => Some(Self::Errno),
            "tomllib" => Some(Self::Tomllib),
            _ => declarations::module(name).map(Self::Declaration),
        }
    }

    pub(crate) fn symbol(self, name: &str) -> Option<Binding> {
        if self == Self::DatetimeNative {
            return Self::Datetime.symbol(name);
        }
        if self == Self::Inspect
            && let Some(&(_, flag)) = imre_ast::code_flags::INSPECT_FLAGS
                .iter()
                .find(|&&(attr, _)| attr == name)
        {
            return Some(Binding::Intrinsic(Intrinsic::IntConstant(i64::from(flag))));
        }
        if self == Self::AnnotationLib && name == "Format" {
            return Some(Binding::IntrinsicModule(Self::AnnotationFormat));
        }
        if self == Self::Logging && name == "handlers" {
            return Some(Binding::IntrinsicModule(Self::LoggingHandlers));
        }
        if self == Self::Logging && name == "config" {
            return Some(Binding::IntrinsicModule(Self::LoggingConfig));
        }
        if self == Self::Sys && name == "_jit" {
            return Some(Binding::IntrinsicModule(Self::SysJit));
        }
        if matches!(self, Self::Os | Self::OsPath | Self::PosixPath) && name == "pathsep" {
            return Some(Binding::Intrinsic(Intrinsic::OsPathsep {
                posix: self == Self::PosixPath,
            }));
        }
        if matches!(self, Self::Os | Self::OsPath | Self::PosixPath)
            && matches!(name, "sep" | "altsep")
        {
            return Some(Binding::Intrinsic(Intrinsic::OsSeparator {
                alternate: name == "altsep",
                posix: self == Self::PosixPath,
            }));
        }
        if self == Self::OsPath {
            return Self::PosixPath.symbol(name);
        }
        if self == Self::Os {
            if name == "path" {
                return Some(Binding::IntrinsicModule(Self::OsPath));
            }
            if let Some(symbol) = os::symbol(name) {
                return Some(Binding::Intrinsic(symbol));
            }
        }
        if self == Self::Json && matches!(name, "encoder" | "decoder") {
            return Some(Binding::IntrinsicModule(if name == "encoder" {
                Self::JsonEncoder
            } else {
                Self::JsonDecoder
            }));
        }
        if self == Self::Email && name == "_header_value_parser" {
            return Some(Binding::IntrinsicModule(Self::EmailHeaderValueParser));
        }
        if self == Self::Email && name == "utils" {
            return Some(Binding::IntrinsicModule(Self::EmailUtils));
        }
        if self == Self::Urllib {
            let child = match name {
                "parse" => Self::UrllibParse,
                "request" => Self::UrllibRequest,
                "error" => Self::UrllibError,
                _ => return None,
            };
            return Some(Binding::IntrinsicModule(child));
        }
        if self == Self::ImportlibAbc && name == "machinery" {
            return Some(Binding::IntrinsicModule(Self::ImportlibMachinery));
        }
        if self == Self::ImportlibResources && name == "abc" {
            return Some(Binding::IntrinsicModule(Self::ImportlibResourcesAbc));
        }
        if self == Self::ImportlibResources && name == "readers" {
            return Some(Binding::IntrinsicModule(Self::ImportlibResourceReaders));
        }
        if self == Self::Collections && name == "abc" {
            return Some(Binding::IntrinsicModule(Self::CollectionsAbc));
        }
        if self == Self::Imre && name == "json" {
            return Some(Binding::IntrinsicModule(Self::ImreJson));
        }
        if self == Self::Imre && name == "toml" {
            return Some(Binding::IntrinsicModule(Self::ImreToml));
        }
        let symbol = match (self, name) {
            (Self::Array, "array") => Intrinsic::NominalClass("array.array"),
            (Self::Numbers, "Number") => Intrinsic::Number,
            (Self::Imp, "extension_suffixes") => {
                Intrinsic::ObjectRuntimeExport("_imp.extension_suffixes")
            }
            (Self::Imp, "pyc_magic_number_token") => {
                Intrinsic::ObjectRuntimeExport("_imp.pyc_magic_number_token")
            }
            (Self::Imp, "is_frozen") => Intrinsic::ObjectRuntimeExport("_imp.is_frozen"),
            (Self::Imp, "_override_frozen_modules_for_tests") => {
                Intrinsic::ObjectRuntimeExport("_imp._override_frozen_modules_for_tests")
            }
            (Self::Imp, "_override_multi_interp_extensions_check") => {
                Intrinsic::ObjectRuntimeExport("_imp._override_multi_interp_extensions_check")
            }
            (Self::Atexit, "register") => Intrinsic::ObjectRuntimeExport("atexit.register"),
            (Self::Atexit, "unregister") => Intrinsic::ObjectRuntimeExport("atexit.unregister"),
            (Self::Atexit, "_run_exitfuncs") => {
                Intrinsic::ObjectRuntimeExport("atexit._run_exitfuncs")
            }
            (Self::Atexit, "_ncallbacks") => Intrinsic::ObjectRuntimeExport("atexit._ncallbacks"),
            (Self::Atexit, "_clear") => Intrinsic::ObjectRuntimeExport("atexit._clear"),
            (Self::Ast, "parse") => Intrinsic::ObjectRuntimeExport("ast.parse"),
            (Self::Ast, "walk") => Intrinsic::ObjectRuntimeExport("ast.walk"),
            (Self::Ast, "iter_fields") => Intrinsic::ObjectRuntimeExport("ast.iter_fields"),
            (Self::Ast, "NodeVisitor") => Intrinsic::ObjectRuntimeExport("ast.NodeVisitor"),
            (Self::Ast, "NodeTransformer") => Intrinsic::ObjectRuntimeExport("ast.NodeTransformer"),
            (Self::Ast, "TemplateStr") => Intrinsic::ObjectRuntimeExport("ast.TemplateStr"),
            (Self::Ast, "Interpolation") => Intrinsic::ObjectRuntimeExport("ast.Interpolation"),
            (Self::Ast, "literal_eval") => Intrinsic::ObjectRuntimeExport("ast.literal_eval"),
            (Self::Gc, "collect") => Intrinsic::ObjectRuntimeExport("gc.collect"),
            (Self::Gc, "disable") => Intrinsic::ObjectRuntimeExport("gc.disable"),
            (Self::Gc, "enable") => Intrinsic::ObjectRuntimeExport("gc.enable"),
            (Self::Gc, "isenabled") => Intrinsic::ObjectRuntimeExport("gc.isenabled"),
            (Self::Marshal, "loads") => Intrinsic::ObjectRuntimeExport("marshal.loads"),
            (Self::Codecs, "utf_16_ex_decode") => {
                Intrinsic::ObjectRuntimeExport("codecs.utf_16_ex_decode")
            }
            (Self::CodecsNative, "utf_16_ex_decode") => {
                Intrinsic::ObjectRuntimeExport("_codecs.utf_16_ex_decode")
            }
            (Self::Codecs, "escape_decode") => {
                Intrinsic::ObjectRuntimeExport("codecs.escape_decode")
            }
            (Self::CodecsNative, "escape_decode") => {
                Intrinsic::ObjectRuntimeExport("_codecs.escape_decode")
            }
            (Self::Codecs | Self::CodecsNative, "register") => {
                Intrinsic::ObjectRuntimeExport("_codecs.register")
            }
            (Self::Codecs | Self::CodecsNative, "unregister") => {
                Intrinsic::ObjectRuntimeExport("_codecs.unregister")
            }
            (Self::CodecsNative, "lookup") => Intrinsic::ObjectRuntimeExport("_codecs.lookup"),
            (Self::CodecsNative, "_imre_codec_functions") => {
                Intrinsic::ObjectRuntimeExport("_codecs._imre_codec_functions")
            }
            (Self::CodecsNative, "_imre_native_search") => {
                Intrinsic::ObjectRuntimeExport("_codecs._imre_native_search")
            }
            (Self::CodecsNative, "encode") => Intrinsic::ObjectRuntimeExport("_codecs.encode"),
            (Self::Pickle, "Pickler") => Intrinsic::PICKLER,
            (Self::Pickle, "dumps") => Intrinsic::ObjectRuntimeExport("pickle.dumps"),
            (Self::Pickle, "loads") => Intrinsic::ObjectRuntimeExport("pickle.loads"),
            (Self::Pickle, "_loads") => Intrinsic::ObjectRuntimeExport("pickle._loads"),
            (Self::Pickle, "UnpicklingError") => {
                Intrinsic::ObjectRuntimeExport("pickle.UnpicklingError")
            }
            (Self::Struct, "Struct") => Intrinsic::ObjectRuntimeExport("struct.Struct"),
            (Self::Struct, "calcsize") => Intrinsic::ObjectRuntimeExport("struct.calcsize"),
            (Self::Struct, "pack") => Intrinsic::ObjectRuntimeExport("struct.pack"),
            (Self::Struct, "unpack") => Intrinsic::ObjectRuntimeExport("struct.unpack"),
            (Self::Struct, "error") => Intrinsic::ObjectRuntimeExport("struct.error"),
            (Self::CollectionsAbc, "Collection") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Collection")
            }
            (Self::CollectionsAbc, "Hashable") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Hashable")
            }
            (Self::CollectionsAbc, "Generator") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Generator")
            }
            (Self::CollectionsAbc, "AsyncIterable") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.AsyncIterable")
            }
            (Self::CollectionsAbc, "AsyncIterator") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.AsyncIterator")
            }
            (Self::CollectionsAbc, "AsyncGenerator") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.AsyncGenerator")
            }
            (Self::CollectionsAbc, "ByteString") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.ByteString")
            }
            (Self::CollectionsAbc, "Buffer") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Buffer")
            }
            (Self::CollectionsAbc, "Coroutine") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Coroutine")
            }
            (Self::CollectionsAbc, "MappingView") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.MappingView")
            }
            (Self::CollectionsAbc, "KeysView") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.KeysView")
            }
            (Self::CollectionsAbc, "ItemsView") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.ItemsView")
            }
            (Self::CollectionsAbc, "ValuesView") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.ValuesView")
            }
            (Self::CollectionsAbc, "Sized") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Sized")
            }
            (Self::CollectionsAbc, "Container") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Container")
            }
            (Self::CollectionsAbc, "MutableSet") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.MutableSet")
            }
            (Self::CollectionsAbc, "Reversible") => {
                Intrinsic::ObjectRuntimeExport("collections.abc.Reversible")
            }
            (Self::Tempfile, "NamedTemporaryFile") => {
                Intrinsic::ObjectRuntimeExport("tempfile.NamedTemporaryFile")
            }
            (Self::Tempfile, "mkstemp") => Intrinsic::ObjectRuntimeExport("tempfile.mkstemp"),
            (Self::Tempfile, "mkdtemp") => Intrinsic::ObjectRuntimeExport("tempfile.mkdtemp"),
            (Self::CollectionsAbc, "Sequence") => {
                Intrinsic::NominalClass("collections.abc.Sequence")
            }
            (Self::CollectionsAbc, "MutableSequence") => {
                Intrinsic::NominalClass("collections.abc.MutableSequence")
            }
            (Self::CollectionsAbc, "MutableMapping") => {
                Intrinsic::NominalClass("collections.abc.MutableMapping")
            }
            (Self::AnnotationLib, "ForwardRef") => Intrinsic::ForwardRef,
            (Self::AnnotationLib, "annotations_to_string") => {
                Intrinsic::ObjectRuntimeExport("annotationlib.annotations_to_string")
            }
            (Self::AnnotationLib, "get_annotations") => Intrinsic::GetAnnotations,
            (Self::AnnotationLib, "type_repr") => Intrinsic::AnnotationTypeRepr,
            (Self::AnnotationFormat, "VALUE") => Intrinsic::AnnotationFormat(1),
            (Self::AnnotationFormat, "VALUE_WITH_FAKE_GLOBALS") => Intrinsic::AnnotationFormat(2),
            (Self::AnnotationFormat, "FORWARDREF") => Intrinsic::AnnotationFormat(3),
            (Self::AnnotationFormat, "STRING") => Intrinsic::AnnotationFormat(4),
            // Execution requests the native loader. Binding the name alone stays on the typed path.
            (Self::Importlib, "import_module") => {
                Intrinsic::RegisteredImport("importlib.import_module")
            }
            (Self::Importlib, "invalidate_caches") => {
                Intrinsic::ObjectRuntimeExport("importlib.invalidate_caches")
            }
            (Self::ImportlibAbc, "Loader") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.Loader")
            }
            (Self::ImportlibAbc, "MetaPathFinder") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.MetaPathFinder")
            }
            (Self::ImportlibAbc, "PathEntryFinder") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.PathEntryFinder")
            }
            (Self::ImportlibAbc, "ResourceLoader") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.ResourceLoader")
            }
            (Self::ImportlibAbc, "InspectLoader") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.InspectLoader")
            }
            (Self::ImportlibAbc, "ExecutionLoader") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.ExecutionLoader")
            }
            (Self::ImportlibAbc, "FileLoader") => {
                Intrinsic::ObjectRuntimeExport("importlib.abc.FileLoader")
            }
            (Self::ImportlibLoaderAbc, "Loader") => {
                Intrinsic::ObjectRuntimeExport("importlib._abc.Loader")
            }
            (Self::ImportlibUtil, "Loader") => {
                Intrinsic::ObjectRuntimeExport("importlib.util.Loader")
            }
            (Self::ImportlibResourcesAbc, "ResourceReader") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.abc.ResourceReader")
            }
            (Self::ImportlibResourcesAbc, "Traversable") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.abc.Traversable")
            }
            (Self::ImportlibReaders | Self::ImportlibResourceReaders, "MultiplexedPath") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.readers.MultiplexedPath")
            }
            (Self::ImportlibReaders | Self::ImportlibResourceReaders, "NamespaceReader") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.readers.NamespaceReader")
            }
            (Self::ImportlibResourcesAbc, "TraversalError") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.abc.TraversalError")
            }
            (Self::ImportlibResourcesAbc, "TraversableResources") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.abc.TraversableResources")
            }
            (Self::ImportlibResources, "as_file") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.as_file")
            }
            (Self::ImportlibResources, "contents") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.contents")
            }
            (Self::ImportlibResources, "is_resource") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.is_resource")
            }
            (Self::ImportlibResources, "files") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.files")
            }
            (Self::ImportlibResources, "open_binary") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.open_binary")
            }
            (Self::ImportlibResources, "open_text") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.open_text")
            }
            (Self::ImportlibResources, "path") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.path")
            }
            (Self::ImportlibResources, "read_binary") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.read_binary")
            }
            (Self::ImportlibResources, "read_text") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.read_text")
            }
            (Self::ImportlibMetadata, "Distribution") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.Distribution")
            }
            (Self::ImportlibMetadata, "PackagePath") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.PackagePath")
            }
            (Self::ImportlibMetadata, "FileHash") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.FileHash")
            }
            (Self::ImportlibMetadata, "packages_distributions") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.packages_distributions")
            }
            (Self::ImportlibMetadata, "files") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.files")
            }
            (Self::ImportlibMetadata, "PathDistribution") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.PathDistribution")
            }
            (Self::ImportlibMetadata, "distributions") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.distributions")
            }
            (Self::ImportlibMetadata, "requires") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.requires")
            }
            (Self::ImportlibMetadata, "metadata") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.metadata")
            }
            (Self::ImportlibMetadata, "distribution") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.distribution")
            }
            (Self::ImportlibMetadata, "_unique") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata._unique")
            }
            (Self::ImportlibMetadata, "PackageNotFoundError") => Intrinsic::PackageNotFoundError,
            (Self::ImportlibMetadata, "EntryPoint") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.EntryPoint")
            }
            (Self::ImportlibMetadata, "EntryPoints") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.EntryPoints")
            }
            (Self::ImportlibMetadata, "entry_points") => {
                Intrinsic::ObjectRuntimeExport("importlib.metadata.entry_points")
            }
            (Self::ImportlibMachinery, "PathFinder") => {
                Intrinsic::ObjectRuntimeExport("importlib.machinery.PathFinder")
            }
            (Self::ImportlibMachinery, "FileFinder") => {
                Intrinsic::ObjectRuntimeExport("importlib.machinery.FileFinder")
            }
            (Self::ImportlibMachinery, "ModuleSpec") => Intrinsic::ModuleSpec,
            (Self::ImportlibMachinery, "all_suffixes") => {
                Intrinsic::ObjectRuntimeExport("importlib.machinery.all_suffixes")
            }
            (Self::ImportlibMachinery, "SOURCE_SUFFIXES") => {
                Intrinsic::ObjectRuntimeExport("importlib.machinery.SOURCE_SUFFIXES")
            }
            (Self::ImportlibMachinery, "BYTECODE_SUFFIXES") => {
                Intrinsic::ObjectRuntimeExport("importlib.machinery.BYTECODE_SUFFIXES")
            }
            (Self::ImportlibMachinery, "EXTENSION_SUFFIXES") => {
                Intrinsic::ObjectRuntimeExport("importlib.machinery.EXTENSION_SUFFIXES")
            }
            (Self::ImportlibMachinery | Self::ImportlibBootstrapExternal, "NamespaceLoader") => {
                Intrinsic::ObjectRuntimeExport("_frozen_importlib_external.NamespaceLoader")
            }
            (Self::ImportlibBootstrapExternal, "SourceFileLoader") => {
                Intrinsic::ObjectRuntimeExport("_frozen_importlib_external.SourceFileLoader")
            }
            (Self::ImportlibMachinery | Self::ImportlibBootstrapExternal, "SourcelessFileLoader") => {
                Intrinsic::ObjectRuntimeExport("_frozen_importlib_external.SourcelessFileLoader")
            }
            (Self::ImportlibUtil, "MAGIC_NUMBER") => {
                Intrinsic::ObjectRuntimeExport("importlib.util.MAGIC_NUMBER")
            }
            (Self::ImportlibUtil, "module_from_spec") => {
                Intrinsic::ObjectRuntimeExport("importlib.util.module_from_spec")
            }
            (Self::ImportlibUtil, "spec_from_file_location") => {
                Intrinsic::ObjectRuntimeExport("importlib.util.spec_from_file_location")
            }
            (Self::ImportlibUtil, "spec_from_loader") => {
                Intrinsic::ObjectRuntimeExport("importlib.util.spec_from_loader")
            }
            (Self::ImportlibUtil, "find_spec") => {
                Intrinsic::RegisteredImport("importlib.util.find_spec")
            }
            (Self::ImportlibUtil, "cache_from_source") => Intrinsic::CachePath { reverse: false },
            (Self::ImportlibUtil, "source_from_cache") => Intrinsic::CachePath { reverse: true },
            (Self::Future, "annotations") => Intrinsic::FutureAnnotations,
            (Self::Logging, "Formatter") => Intrinsic::LogFormatter,
            (Self::Time, "get_clock_info") => Intrinsic::ObjectRuntimeExport("time.get_clock_info"),
            (Self::Time, "tzset") => Intrinsic::ObjectRuntimeExport("time.tzset"),
            (Self::Time, "timezone") => Intrinsic::ObjectRuntimeExport("time.timezone"),
            (Self::Time, "altzone") => Intrinsic::ObjectRuntimeExport("time.altzone"),
            (Self::Time, "daylight") => Intrinsic::ObjectRuntimeExport("time.daylight"),
            (Self::Time, "tzname") => Intrinsic::ObjectRuntimeExport("time.tzname"),
            (Self::Time, "ctime") => Intrinsic::ObjectRuntimeExport("time.ctime"),
            (Self::Time, "time_ns") => Intrinsic::ObjectRuntimeExport("time.time_ns"),
            (Self::Time, "monotonic") => Intrinsic::ObjectRuntimeExport("time.monotonic"),
            (Self::Time, "perf_counter") => Intrinsic::ObjectRuntimeExport("time.perf_counter"),
            (Self::Time, "struct_time") => Intrinsic::StructTime,
            (Self::Time, "mktime") => Intrinsic::Mktime,
            (Self::Time, "gmtime") => Intrinsic::ClockConverter(true),
            (Self::Time, "localtime") => Intrinsic::ClockConverter(false),
            (Self::Logging, "LogRecord" | "_logRecordFactory") => Intrinsic::LogRecord,
            (Self::Logging, "logThreads") => Intrinsic::LogRecordFlag(1),
            (Self::Logging, "logProcesses") => Intrinsic::LogRecordFlag(2),
            (Self::Logging, "logMultiprocessing") => Intrinsic::LogRecordFlag(4),
            (Self::Logging, "logAsyncioTasks") => Intrinsic::LogRecordFlag(8),
            (Self::Logging, "raiseExceptions") => Intrinsic::LogRecordFlag(16),
            (Self::Logging, "makeLogRecord") => Intrinsic::MakeLogRecord,
            (Self::Logging, "getLogger") => Intrinsic::GetLogger,
            (Self::Logging, "disable") => Intrinsic::LoggingDisable,
            (Self::Logging, "getLevelName") => Intrinsic::GetLevelName,
            (Self::Logging, "addLevelName") => Intrinsic::AddLevelName,
            (Self::Logging, "getLevelNamesMapping") => Intrinsic::LevelNamesMapping,
            (Self::Logging, "_nameToLevel") => Intrinsic::LevelMap { names: true },
            (Self::Logging, "_levelToName") => Intrinsic::LevelMap { names: false },
            (Self::Logging, "basicConfig") => Intrinsic::BasicConfig,
            (Self::LoggingConfig, "dictConfig") => Intrinsic::DictConfig,
            (Self::LoggingConfig, "fileConfig") => Intrinsic::FileConfig,
            (Self::Logging, "DEBUG") => Intrinsic::LogLevel(10),
            (Self::Logging, "INFO") => Intrinsic::LogLevel(20),
            (Self::Abc, "ABC") => Intrinsic::Abc,
            (Self::Abc, "ABCMeta") => Intrinsic::AbcMeta,
            (Self::Abc, "get_cache_token") => Intrinsic::ObjectRuntimeExport("abc.get_cache_token"),
            (Self::Abc, "abstractmethod") => Intrinsic::AbstractMethod,
            (Self::Asyncio, "create_subprocess_exec") => Intrinsic::CreateSubprocessExec,
            (Self::Asyncio, "run") => Intrinsic::RunLoop,
            (Self::ContextVars | Self::ContextVarsNative, "Context") => Intrinsic::Context,
            (Self::ContextVars | Self::ContextVarsNative, "ContextVar") => Intrinsic::ContextVar,
            (Self::ContextVars | Self::ContextVarsNative, "Token") => Intrinsic::ContextToken,
            (Self::ContextVars | Self::ContextVarsNative, "copy_context") => Intrinsic::CopyContext,
            (Self::Asyncio, "new_event_loop") => Intrinsic::NewLoop,
            (Self::Asyncio, "set_event_loop") => Intrinsic::SetLoop,
            (Self::Asyncio, "ensure_future") => Intrinsic::EnsureTask,
            (Self::Asyncio, "CancelledError") => Intrinsic::CancelledError,
            (Self::Asyncio, "gather") => Intrinsic::Gather,
            (Self::Asyncio, "TaskGroup") => Intrinsic::TaskGroup,
            (Self::Asyncio, "Semaphore") => Intrinsic::Semaphore,
            (Self::Linecache, name) => Intrinsic::Linecache(match name {
                "getline" => crate::ir::LinecacheOperation::Getline,
                "getlines" => crate::ir::LinecacheOperation::Getlines,
                "updatecache" => crate::ir::LinecacheOperation::Update,
                "clearcache" => crate::ir::LinecacheOperation::Clear,
                "checkcache" => crate::ir::LinecacheOperation::Check,
                "lazycache" => crate::ir::LinecacheOperation::Lazy,
                "cache" => crate::ir::LinecacheOperation::Cache,
                _ => return None,
            }),
            (Self::Inspect, "getmodulename") => {
                Intrinsic::ObjectRuntimeExport("inspect.getmodulename")
            }
            (Self::Inspect, "Signature") => Intrinsic::ObjectRuntimeExport("inspect.Signature"),
            (Self::Inspect, "Parameter") => Intrinsic::ObjectRuntimeExport("inspect.Parameter"),
            (Self::Inspect, "BoundArguments") => {
                Intrinsic::ObjectRuntimeExport("inspect.BoundArguments")
            }
            (Self::Inspect, "getmodule") => Intrinsic::ObjectRuntimeExport("inspect.getmodule"),
            (Self::Inspect, "isroutine") => Intrinsic::ObjectRuntimeExport("inspect.isroutine"),
            (Self::Inspect, "ismethodwrapper") => {
                Intrinsic::ObjectRuntimeExport("inspect.ismethodwrapper")
            }
            (Self::Inspect, "isbuiltin") => Intrinsic::ObjectRuntimeExport("inspect.isbuiltin"),
            (Self::Inspect, "unwrap") => Intrinsic::ObjectRuntimeExport("inspect.unwrap"),
            (Self::Inspect, "signature") => Intrinsic::ObjectRuntimeExport("inspect.signature"),
            (Self::Inspect, "getattr_static") => Intrinsic::InspectGetattrStatic,
            (Self::Inspect, "ismethoddescriptor") => Intrinsic::InspectIsMethodDescriptor,
            (Self::Inspect, "iscoroutinefunction") => Intrinsic::InspectCoroutine { mark: false },
            (Self::Inspect, "markcoroutinefunction") => Intrinsic::InspectCoroutine { mark: true },
            (Self::Inspect, "_sentinel") => Intrinsic::InspectSentinel,
            (Self::Contextlib, "contextmanager") => Intrinsic::ContextManager,
            (Self::Contextlib, "redirect_stdout") => {
                Intrinsic::ObjectRuntimeExport("contextlib.redirect_stdout")
            }
            (Self::Contextlib, "redirect_stderr") => {
                Intrinsic::ObjectRuntimeExport("contextlib.redirect_stderr")
            }
            (Self::Contextlib, "suppress") => Intrinsic::ObjectRuntimeExport("contextlib.suppress"),
            (Self::Contextlib, "AbstractAsyncContextManager") => {
                Intrinsic::ObjectRuntimeExport("contextlib.AbstractAsyncContextManager")
            }
            (Self::Contextlib, "closing") => Intrinsic::ObjectRuntimeExport("contextlib.closing"),
            (Self::Contextlib, "nullcontext") => Intrinsic::NullContext,
            (Self::Warnings, "warn") => Intrinsic::ObjectRuntimeExport("warnings.warn"),
            (Self::Warnings, "simplefilter") => Intrinsic::WarningFilter { simple: true },
            (Self::Warnings, "filterwarnings") => Intrinsic::WarningFilter { simple: false },
            (Self::Warnings, "catch_warnings") => Intrinsic::CatchWarnings,
            (Self::Warnings, "resetwarnings") => Intrinsic::WarningReset,
            (Self::Warnings, "formatwarning") => Intrinsic::WarningDisplay { show: false },
            (Self::Warnings, "showwarning") => Intrinsic::WarningDisplay { show: true },
            (Self::Time, "sleep") => Intrinsic::TimeSleep,
            (Self::Time, "time") => Intrinsic::WallTime,
            (Self::Signal | Self::SignalNative, "signal") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.signal")
            }
            (Self::Signal | Self::SignalNative, "getsignal") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.getsignal")
            }
            (Self::Signal | Self::SignalNative, "default_int_handler") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.default_int_handler")
            }
            (Self::Signal | Self::SignalNative, "SIG_DFL" | "SIG_IGN") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.Handlers")
            }
            (Self::Signal, "Handlers") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.Handlers")
            }
            (Self::Signal, "SIGINT") => Intrinsic::IntConstant(2),
            (Self::Signal, "SIGTERM") => Intrinsic::IntConstant(15),
            (Self::SignalNative, "SIGINT") => Intrinsic::ObjectRuntimeExport("_signal.SIGINT"),
            (Self::SignalNative, "SIGALRM") => Intrinsic::ObjectRuntimeExport("_signal.SIGALRM"),
            (Self::SignalNative, "SIGTERM") => Intrinsic::ObjectRuntimeExport("_signal.SIGTERM"),
            (Self::Signal | Self::SignalNative, "alarm") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.alarm")
            }
            (Self::Signal, "SIGALRM") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.SIGALRM")
            }
            (Self::Signal, "Signals") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.Signals")
            }
            #[cfg(unix)]
            (Self::Signal, "sigpending") => Intrinsic::ObjectRuntimeExport("signal.sigpending"),
            #[cfg(unix)]
            (Self::SignalNative, "sigpending") => {
                Intrinsic::ObjectRuntimeExport("_signal.sigpending")
            }
            #[cfg(unix)]
            (Self::Signal | Self::SignalNative, "NSIG") => {
                Intrinsic::ObjectRuntimeExport("signal.NSIG")
            }
            // Signal numbers are IntEnum instances when accessed through the boxed module.
            #[cfg(unix)]
            (
                Self::Signal | Self::SignalNative,
                "SIGHUP" | "SIGQUIT" | "SIGILL" | "SIGTRAP" | "SIGABRT" | "SIGIOT" | "SIGBUS"
                | "SIGFPE" | "SIGKILL" | "SIGUSR1" | "SIGSEGV" | "SIGUSR2" | "SIGPIPE" | "SIGCHLD"
                | "SIGCLD" | "SIGCONT" | "SIGSTOP" | "SIGTSTP" | "SIGTTIN" | "SIGTTOU" | "SIGURG"
                | "SIGXCPU" | "SIGXFSZ" | "SIGVTALRM" | "SIGPROF" | "SIGWINCH" | "SIGIO"
                | "SIGPOLL" | "SIGPWR" | "SIGSYS" | "SIGSTKFLT" | "SIGEMT" | "SIGRTMIN"
                | "SIGRTMAX",
            ) => Intrinsic::ObjectRuntimeExport("signal.Signals"),
            // Native executables have no adaptive bytecode interpreter.
            (Self::Opcode, "ENABLE_SPECIALIZATION" | "ENABLE_SPECIALIZATION_FT") => {
                Intrinsic::IntConstant(0)
            }
            (Self::Opcode, "stack_effect") => {
                Intrinsic::ObjectRuntimeExport("_opcode.stack_effect")
            }
            (Self::Opcode, "is_valid") => Intrinsic::ObjectRuntimeExport("_opcode.is_valid"),
            (Self::Opcode, "has_arg") => Intrinsic::ObjectRuntimeExport("_opcode.has_arg"),
            (Self::Opcode, "has_const") => Intrinsic::ObjectRuntimeExport("_opcode.has_const"),
            (Self::Opcode, "has_name") => Intrinsic::ObjectRuntimeExport("_opcode.has_name"),
            (Self::Opcode, "has_jump") => Intrinsic::ObjectRuntimeExport("_opcode.has_jump"),
            (Self::Opcode, "has_free") => Intrinsic::ObjectRuntimeExport("_opcode.has_free"),
            (Self::Opcode, "has_local") => Intrinsic::ObjectRuntimeExport("_opcode.has_local"),
            (Self::Opcode, "has_exc") => Intrinsic::ObjectRuntimeExport("_opcode.has_exc"),
            (Self::Opcode, "get_specialization_stats") => {
                Intrinsic::ObjectRuntimeExport("_opcode.get_specialization_stats")
            }
            (Self::Opcode, "get_nb_ops") => Intrinsic::ObjectRuntimeExport("_opcode.get_nb_ops"),
            (Self::Opcode, "get_intrinsic1_descs") => {
                Intrinsic::ObjectRuntimeExport("_opcode.get_intrinsic1_descs")
            }
            (Self::Opcode, "get_intrinsic2_descs") => {
                Intrinsic::ObjectRuntimeExport("_opcode.get_intrinsic2_descs")
            }
            (Self::Opcode, "get_special_method_names") => {
                Intrinsic::ObjectRuntimeExport("_opcode.get_special_method_names")
            }
            (Self::Opcode, "get_executor") => {
                Intrinsic::ObjectRuntimeExport("_opcode.get_executor")
            }
            (Self::Unidiff, "PatchSet") => Intrinsic::PatchSet,
            (Self::Unidiff, "PatchedFile") => Intrinsic::PatchedFile,
            (Self::Unidiff, "Hunk") => Intrinsic::Hunk,
            (Self::Unidiff, "UnidiffParseError") => Intrinsic::UnidiffParseError,
            (Self::CollectionsAbc, "Awaitable") => Intrinsic::Awaitable,
            (Self::Typing, "overload") => Intrinsic::Overload,
            (Self::Typing, "get_overloads") => Intrinsic::GetOverloads,
            (Self::Typing, "_overload_registry") => Intrinsic::OverloadRegistry,
            (Self::Typing, "_overload_dummy") => Intrinsic::OverloadDummy,
            (Self::Typing, "clear_overloads") => Intrinsic::ClearOverloads,
            (Self::Typing, "TypeVar") => Intrinsic::TypeVar,
            (Self::Typing, "NewType") => Intrinsic::NewType,
            (Self::Functools, "cache") => Intrinsic::FunctionCache,
            (Self::Functools, "_CacheInfo") => Intrinsic::CacheInfo,
            (Self::Functools, "WRAPPER_ASSIGNMENTS") => {
                Intrinsic::WrapperAttributes(WrapperAttributes::Assigned)
            }
            (Self::Functools, "WRAPPER_UPDATES") => {
                Intrinsic::WrapperAttributes(WrapperAttributes::Updated)
            }
            (Self::Functools, "lru_cache") => Intrinsic::LruCache,
            (Self::Functools, "total_ordering") => {
                Intrinsic::ObjectRuntimeExport("functools.total_ordering")
            }
            (Self::Functools, "singledispatch") => Intrinsic::SingleDispatch,
            (Self::Functools, "update_wrapper") => Intrinsic::UpdateWrapper,
            (Self::Functools, "wraps") => Intrinsic::Wraps,
            (Self::Fnmatch, "fnmatch" | "fnmatchcase") => {
                Intrinsic::Fnmatch(FnmatchOperation::Match {
                    normcase: name == "fnmatch",
                })
            }
            (Self::Fnmatch, "filter" | "filterfalse") => {
                Intrinsic::Fnmatch(FnmatchOperation::Filter {
                    inverse: name == "filterfalse",
                })
            }
            (Self::Fnmatch, "translate") => Intrinsic::Fnmatch(FnmatchOperation::Translate),
            (Self::Fnmatch, "_translate") => Intrinsic::Fnmatch(FnmatchOperation::Parts),
            (Self::Functools, "cached_property") => Intrinsic::CachedProperty,
            (Self::Functools, "partial") => Intrinsic::Partial,
            (Self::Functools, "singledispatchmethod") => {
                Intrinsic::ObjectRuntimeExport("functools.singledispatchmethod")
            }
            (Self::Functools, "partialmethod") => {
                Intrinsic::ObjectRuntimeExport("functools.partialmethod")
            }
            (Self::Functools, "Placeholder") => Intrinsic::Placeholder,
            (Self::Functools, "reduce") => Intrinsic::Reduce,
            (Self::Functools, "cmp_to_key") => Intrinsic::CmpToKey,
            (Self::TomliW, "dumps") => Intrinsic::TomlDumps,
            (Self::Tomli, "loads") => Intrinsic::TomliLoads,
            (Self::Tomli, "load") => Intrinsic::TomliLoad,
            (Self::TomliW, "dump") => Intrinsic::TomliDump,
            (Self::Enum, "Enum") => Intrinsic::StandardEnum,
            (Self::Dataclasses, "is_dataclass") => Intrinsic::IsDataclass,
            (Self::Subprocess, "__all__") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("subprocess.__all__")
            }
            (Self::Subprocess, "PIPE") => Intrinsic::Pipe,
            (Self::Subprocess, "DEVNULL") => Intrinsic::Devnull,
            (Self::Subprocess, "check_output") => Intrinsic::CheckOutput,
            (Self::Subprocess, "getoutput") => Intrinsic::ShellOutput { status: false },
            (Self::Subprocess, "getstatusoutput") => Intrinsic::ShellOutput { status: true },
            (Self::Typing, "TYPE_CHECKING") => Intrinsic::TypeChecking,
            (Self::Sys, "stdin" | "__stdin__") => Intrinsic::Stdin,
            (Self::Sys, "warnoptions") => Intrinsic::ObjectRuntimeExport("sys.warnoptions"),
            (Self::Sys, "byteorder") => Intrinsic::ObjectRuntimeExport("sys.byteorder"),
            (Self::Sys, "hash_info") => Intrinsic::ObjectRuntimeExport("sys.hash_info"),
            (Self::Sys, "monitoring") => Intrinsic::ObjectRuntimeExport("sys.monitoring"),
            (Self::Sys, "stdout") => Intrinsic::Stdout,
            (Self::Sys, "__stdout__") => Intrinsic::OriginalStdout,
            (Self::Sys, "stderr" | "__stderr__") => Intrinsic::Stderr,
            (Self::Sys, "exception") => Intrinsic::SysException,
            (Self::Traceback, "print_stack") => {
                Intrinsic::ObjectRuntimeExport("traceback.print_stack")
            }
            (Self::Traceback, "print_exc") => Intrinsic::TracebackPrintExc,
            (Self::Traceback, "format_exc") => {
                Intrinsic::TracebackFormatter("traceback.format_exc")
            }
            (Self::Traceback, "format_exception") => {
                Intrinsic::TracebackFormatter("traceback.format_exception")
            }
            (Self::Traceback, "print_exception") => {
                Intrinsic::TracebackFormatter("traceback.print_exception")
            }
            (Self::Sys, "_getframemodulename") => Intrinsic::SysGetFrameModuleName,
            (Self::Sys, "getsizeof") => Intrinsic::ObjectRuntimeExport("sys.getsizeof"),
            (Self::Sys, "getrecursionlimit") => Intrinsic::SysGetRecursionLimit,
            (Self::Sys, "setrecursionlimit") => Intrinsic::SysSetRecursionLimit,
            (Self::SysJit, "is_available") => Intrinsic::SysJitPredicate("_jit.is_available"),
            (Self::SysJit, "is_enabled") => Intrinsic::SysJitPredicate("_jit.is_enabled"),
            (Self::SysJit, "is_active") => Intrinsic::SysJitPredicate("_jit.is_active"),
            (Self::Locale | Self::LocaleNative, "getencoding") => {
                Intrinsic::SysEncoding(SysEncoding::Locale)
            }
            (Self::Locale, "getlocale") => Intrinsic::ObjectRuntimeExport("locale.getlocale"),
            (Self::Locale, "setlocale") => Intrinsic::ObjectRuntimeExport("locale.setlocale"),
            (Self::LocaleNative, "setlocale") => {
                Intrinsic::ObjectRuntimeExport("_locale.setlocale")
            }
            (Self::Locale | Self::LocaleNative, "Error") => {
                Intrinsic::ObjectRuntimeExport("_locale.Error")
            }
            (Self::Locale | Self::LocaleNative, "LC_ALL") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_ALL")
            }
            (Self::Locale | Self::LocaleNative, "LC_COLLATE") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_COLLATE")
            }
            (Self::Locale | Self::LocaleNative, "LC_CTYPE") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_CTYPE")
            }
            (Self::Locale | Self::LocaleNative, "LC_MESSAGES") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_MESSAGES")
            }
            (Self::Locale | Self::LocaleNative, "LC_MONETARY") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_MONETARY")
            }
            (Self::Locale | Self::LocaleNative, "LC_NUMERIC") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_NUMERIC")
            }
            (Self::Locale | Self::LocaleNative, "LC_TIME") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_TIME")
            }
            (Self::Sys, "getdefaultencoding") => Intrinsic::SysEncoding(SysEncoding::Default),
            (Self::Sys, "getfilesystemencoding") => Intrinsic::SysEncoding(SysEncoding::Filesystem),
            (Self::Sys, "getfilesystemencodeerrors") => {
                Intrinsic::SysEncoding(SysEncoding::FilesystemErrors)
            }
            (Self::Sys, "intern") => Intrinsic::SysIntern { query: false },
            (Self::Sys, "_is_interned") => Intrinsic::SysIntern { query: true },
            (Self::Sys, "flags") => Intrinsic::SysFlags,
            (Self::Sys, "implementation") => Intrinsic::SysImplementation,
            (Self::Sys, "pycache_prefix") => Intrinsic::SysPycachePrefix,
            (Self::Sys, "dont_write_bytecode") => Intrinsic::SysDontWriteBytecode,
            (Self::Sys, "version_info") => Intrinsic::SysVersionInfo,
            (Self::Sys, "thread_info") => Intrinsic::SysThreadInfo,
            (Self::Sys, "stdlib_module_names") => Intrinsic::SysStdlibModuleNames,
            (Self::Sys, "version") => Intrinsic::SysVersion,
            (Self::Sys, "maxunicode") => Intrinsic::IntConstant(0x10FFFF),
            (Self::Sys, "hexversion") => Intrinsic::SysHexversion,
            (Self::Sys, "platform") => Intrinsic::SysPlatform,
            (Self::Select, "poll") => Intrinsic::Readiness(crate::ir::Readiness::New),
            (Self::Select, "select") => Intrinsic::Readiness(crate::ir::Readiness::Select),
            (Self::Select, "POLLIN") => Intrinsic::DescriptorConstant("pollin"),
            (Self::Select, "POLLPRI") => Intrinsic::DescriptorConstant("pollpri"),
            (Self::Select, "POLLOUT") => Intrinsic::DescriptorConstant("pollout"),
            (Self::Select, "POLLERR") => Intrinsic::DescriptorConstant("pollerr"),
            (Self::Select, "POLLHUP") => Intrinsic::DescriptorConstant("pollhup"),
            (Self::Select, "POLLNVAL") => Intrinsic::DescriptorConstant("pollnval"),
            (Self::Os, "name") => Intrinsic::OsName,
            (Self::Os, "linesep") => Intrinsic::OsLinesep,
            (Self::Errno, _) => Intrinsic::ErrnoAttribute,
            (Self::Argparse, "ArgumentParser") => Intrinsic::ArgumentParser,
            (Self::Argparse, "Namespace") => Intrinsic::ArgparseNamespace,
            (Self::Argparse, "SUPPRESS") => Intrinsic::ArgparseSuppress,
            (Self::Argparse, "BooleanOptionalAction") => Intrinsic::BooleanOptionalAction,
            (Self::Argparse, "RawDescriptionHelpFormatter") => {
                Intrinsic::RawDescriptionHelpFormatter
            }
            (Self::Builtins, "_") => Intrinsic::BuiltinsLast,
            (Self::Builtins, "len") => Intrinsic::BuiltinsLen,
            (Self::Builtins, "ascii") => Intrinsic::BuiltinsAscii,
            (Self::Builtins, "hex") => Intrinsic::BuiltinsHex,
            (Self::Builtins, "exec") => Intrinsic::ScopedObjectExport("builtins.exec"),
            (Self::Sys, "argv") => Intrinsic::Argv,
            (Self::Sys, "orig_argv") => Intrinsic::OrigArgv,
            (Self::Sys, "executable") => Intrinsic::SysExecutable,
            (Self::Sysconfig, "get_path") => Intrinsic::GetSysconfigPath,
            (Self::Sys, "exit") => Intrinsic::SysExit,
            (Self::Sys, "__displayhook__") => Intrinsic::SysDisplayHook,
            (Self::LoggingHandlers, "TimedRotatingFileHandler") => Intrinsic::TimedFileHandler,
            (Self::Datetime, "time") => Intrinsic::ClockTime,
            (Self::Datetime, "date") => Intrinsic::Calendar(crate::ir::CalendarType::Date),
            (Self::Datetime, "datetime") => Intrinsic::Calendar(crate::ir::CalendarType::DateTime),
            (Self::Datetime, "timedelta") => Intrinsic::Calendar(crate::ir::CalendarType::Delta),
            (Self::Datetime, "timezone") => Intrinsic::Calendar(crate::ir::CalendarType::Zone),
            (Self::Datetime, "UTC") => Intrinsic::CalendarUtc,
            (Self::Datetime, "MINYEAR") => Intrinsic::IntConstant(1),
            (Self::Datetime, "MAXYEAR") => Intrinsic::IntConstant(9999),
            (Self::Datetime, "tzinfo") => Intrinsic::ObjectRuntimeExport("datetime.tzinfo"),
            (Self::Logging, "Logger") => Intrinsic::Logger,
            (Self::Logging, "Manager") => Intrinsic::LogManager,
            (Self::Logging, "root") => Intrinsic::LogRoot,
            (Self::Logging, "log") => Intrinsic::LogEmit(None),
            (Self::Logging, "debug") => Intrinsic::LogEmit(Some(10)),
            (Self::Logging, "info") => Intrinsic::LogEmit(Some(20)),
            (Self::Logging, "warning") => Intrinsic::LogEmit(Some(30)),
            (Self::Logging, "error") => Intrinsic::LogEmit(Some(40)),
            (Self::Logging, "critical" | "fatal") => Intrinsic::LogEmit(Some(50)),
            (Self::Logging, "NOTSET") => Intrinsic::LogLevel(0),
            (Self::Logging, "WARNING" | "WARN") => Intrinsic::LogLevel(30),
            (Self::Logging, "ERROR") => Intrinsic::LogLevel(40),
            (Self::Logging, "CRITICAL" | "FATAL") => Intrinsic::LogLevel(50),
            (Self::Shlex, "split") => Intrinsic::ShlexSplit,
            (Self::Shlex, "quote") => Intrinsic::ShlexQuote,
            (Self::Shlex, "join") => Intrinsic::ShlexJoin,
            (Self::Difflib, "ndiff") => Intrinsic::Ndiff,
            (Self::DoctestRuntime, "capture_start") => Intrinsic::DocCaptureStart,
            (Self::DoctestRuntime, "capture_take") => Intrinsic::DocCaptureTake,
            (Self::DoctestRuntime, "display") => Intrinsic::DocDisplay,
            (Self::DoctestRuntime, "ascii_output") => Intrinsic::DocAscii,
            (Self::DoctestRuntime, "execute") => Intrinsic::DocExecute,
            (Self::MockRuntime, "mark_call_list") => Intrinsic::MockCallList { mark: true },
            (Self::MockRuntime, "is_call_list") => Intrinsic::MockCallList { mark: false },
            (Self::Typing, "ParamSpec") => Intrinsic::ObjectRuntimeExport("typing.ParamSpec"),
            (Self::Typing, "ParamSpecArgs") => {
                Intrinsic::ObjectRuntimeExport("typing.ParamSpecArgs")
            }
            (Self::Typing, "ParamSpecKwargs") => {
                Intrinsic::ObjectRuntimeExport("typing.ParamSpecKwargs")
            }
            (Self::Typing, "TypeAliasType") => {
                Intrinsic::ObjectRuntimeExport("typing.TypeAliasType")
            }
            (Self::Typing, "TypeVarTuple") => Intrinsic::ObjectRuntimeExport("typing.TypeVarTuple"),
            (Self::Typing, "Concatenate") => Intrinsic::ObjectRuntimeExport("typing.Concatenate"),
            (Self::Typing, "Unpack") => Intrinsic::ObjectRuntimeExport("typing.Unpack"),
            (Self::Typing, "NoDefault") => Intrinsic::ObjectRuntimeExport("typing.NoDefault"),
            (Self::Typing, "T") => Intrinsic::ObjectRuntimeExport("typing.T"),
            (Self::Typing, "KT") => Intrinsic::ObjectRuntimeExport("typing.KT"),
            (Self::Typing, "VT") => Intrinsic::ObjectRuntimeExport("typing.VT"),
            (Self::Typing, "Type") => Intrinsic::ObjectRuntimeExport("typing.Type"),
            (Self::Typing, "Tuple") => Intrinsic::ObjectRuntimeExport("typing.Tuple"),
            (Self::Typing, "Set") => Intrinsic::ObjectRuntimeExport("typing.Set"),
            (Self::Typing, "FrozenSet") => Intrinsic::ObjectRuntimeExport("typing.FrozenSet"),
            (Self::Typing, "Dict") => Intrinsic::ObjectRuntimeExport("typing.Dict"),
            (Self::Typing, "Generator") => Intrinsic::ObjectRuntimeExport("typing.Generator"),
            (Self::Typing, "Reversible") => Intrinsic::ObjectRuntimeExport("typing.Reversible"),
            (Self::Typing, "Sized") => Intrinsic::ObjectRuntimeExport("typing.Sized"),
            (Self::Typing, "ByteString") => Intrinsic::ObjectRuntimeExport("typing.ByteString"),
            (Self::Typing, "ContextManager") => {
                Intrinsic::ObjectRuntimeExport("typing.ContextManager")
            }
            (Self::Typing, "AsyncContextManager") => {
                Intrinsic::ObjectRuntimeExport("typing.AsyncContextManager")
            }
            (Self::Typing, "Awaitable") => Intrinsic::ObjectRuntimeExport("typing.Awaitable"),
            (Self::Typing, "AbstractSet") => Intrinsic::ObjectRuntimeExport("typing.AbstractSet"),
            (Self::Typing, "AsyncGenerator") => {
                Intrinsic::ObjectRuntimeExport("typing.AsyncGenerator")
            }
            (Self::Typing, "AsyncIterable") => {
                Intrinsic::ObjectRuntimeExport("typing.AsyncIterable")
            }
            (Self::Typing, "Collection") => Intrinsic::ObjectRuntimeExport("typing.Collection"),
            (Self::Typing, "Container") => Intrinsic::ObjectRuntimeExport("typing.Container"),
            (Self::Typing, "Coroutine") => Intrinsic::ObjectRuntimeExport("typing.Coroutine"),
            (Self::Typing, "Hashable") => Intrinsic::ObjectRuntimeExport("typing.Hashable"),
            (Self::Typing, "ItemsView") => Intrinsic::ObjectRuntimeExport("typing.ItemsView"),
            (Self::Typing, "KeysView") => Intrinsic::ObjectRuntimeExport("typing.KeysView"),
            (Self::Typing, "MappingView") => Intrinsic::ObjectRuntimeExport("typing.MappingView"),
            (Self::Typing, "MutableSequence") => {
                Intrinsic::ObjectRuntimeExport("typing.MutableSequence")
            }
            (Self::Typing, "MutableSet") => Intrinsic::ObjectRuntimeExport("typing.MutableSet"),
            (Self::Typing, "ValuesView") => Intrinsic::ObjectRuntimeExport("typing.ValuesView"),
            (Self::Typing, "AsyncIterator") => {
                Intrinsic::ObjectRuntimeExport("typing.AsyncIterator")
            }
            (Self::Typing, "Pattern") => Intrinsic::ObjectRuntimeExport("typing.Pattern"),
            (Self::Typing, "Match") => Intrinsic::ObjectRuntimeExport("typing.Match"),
            (Self::Typing, "ChainMap") => Intrinsic::ObjectRuntimeExport("typing.ChainMap"),
            (Self::Typing, "Deque") => Intrinsic::ObjectRuntimeExport("typing.Deque"),
            (Self::Typing, "OrderedDict") => Intrinsic::ObjectRuntimeExport("typing.OrderedDict"),
            (Self::Typing, "Counter") => Intrinsic::ObjectRuntimeExport("typing.Counter"),
            (Self::Typing, "DefaultDict") => Intrinsic::ObjectRuntimeExport("typing.DefaultDict"),
            (Self::Typing, "Mapping") => Intrinsic::TYPING_MAPPING,
            (Self::Typing, "MutableMapping") => {
                Intrinsic::ObjectRuntimeExport("typing.MutableMapping")
            }
            (Self::Typing, "get_args") => Intrinsic::ObjectRuntimeExport("typing.get_args"),
            (Self::Typing, "get_origin") => Intrinsic::ObjectRuntimeExport("typing.get_origin"),
            (Self::Typing, "get_type_hints") => {
                Intrinsic::ObjectRuntimeExport("typing.get_type_hints")
            }
            (Self::Typing, "List") => Intrinsic::ObjectRuntimeExport("typing.List"),
            (Self::Typing, "dataclass_transform") => {
                Intrinsic::ObjectRuntimeExport("typing.dataclass_transform")
            }
            (Self::Typing, "no_type_check_decorator") => {
                Intrinsic::ObjectRuntimeExport("typing.no_type_check_decorator")
            }
            (Self::Typing, "final") => Intrinsic::ObjectRuntimeExport("typing.final"),
            (Self::Typing, "Protocol") => Intrinsic::TYPING_PROTOCOL,
            (Self::Typing, "SupportsInt") => Intrinsic::ObjectRuntimeExport("typing.SupportsInt"),
            (Self::Typing, "SupportsFloat") => {
                Intrinsic::ObjectRuntimeExport("typing.SupportsFloat")
            }
            (Self::Typing, "SupportsComplex") => {
                Intrinsic::ObjectRuntimeExport("typing.SupportsComplex")
            }
            (Self::Typing, "SupportsBytes") => {
                Intrinsic::ObjectRuntimeExport("typing.SupportsBytes")
            }
            (Self::Typing, "SupportsIndex") => {
                Intrinsic::ObjectRuntimeExport("typing.SupportsIndex")
            }
            (Self::Typing, "SupportsAbs") => Intrinsic::ObjectRuntimeExport("typing.SupportsAbs"),
            (Self::Typing, "SupportsRound") => {
                Intrinsic::ObjectRuntimeExport("typing.SupportsRound")
            }
            (Self::Typing, "runtime_checkable") => {
                Intrinsic::ObjectRuntimeExport("typing.runtime_checkable")
            }
            (Self::Typing, "get_protocol_members") => {
                Intrinsic::ObjectRuntimeExport("typing.get_protocol_members")
            }
            (Self::Typing, "override") => Intrinsic::ObjectRuntimeExport("typing.override"),
            (Self::Typing, "is_protocol") => Intrinsic::ObjectRuntimeExport("typing.is_protocol"),
            (Self::Typing, "Annotated") => Intrinsic::ObjectRuntimeExport("typing.Annotated"),
            (Self::Typing, "ForwardRef") => Intrinsic::ObjectRuntimeExport("typing.ForwardRef"),
            (Self::Typing, "TypeGuard") => Intrinsic::ObjectRuntimeExport("typing.TypeGuard"),
            (Self::Typing, "TypeIs") => Intrinsic::ObjectRuntimeExport("typing.TypeIs"),
            (Self::Typing, "Required") => Intrinsic::ObjectRuntimeExport("typing.Required"),
            (Self::Typing, "NotRequired") => Intrinsic::ObjectRuntimeExport("typing.NotRequired"),
            (Self::Typing, "ReadOnly") => Intrinsic::ObjectRuntimeExport("typing.ReadOnly"),
            (Self::Typing, "NamedTuple") => Intrinsic::ObjectRuntimeExport("typing.NamedTuple"),
            (Self::Typing, "TypedDict") => Intrinsic::ObjectRuntimeExport("typing.TypedDict"),
            (Self::Typing, "is_typeddict") => Intrinsic::ObjectRuntimeExport("typing.is_typeddict"),
            (Self::Typing, "Final") => Intrinsic::Final,
            (Self::Typing, "Text") => Intrinsic::ObjectRuntimeExport("typing.Text"),
            (Self::Typing, "TextIO") => Intrinsic::TextIO,
            (Self::Typing, "BinaryIO") => Intrinsic::BinaryIO,
            (Self::Typing, "Union") => Intrinsic::Union,
            (Self::Typing, "Literal") => Intrinsic::Literal,
            (Self::Typing, "Self") => Intrinsic::SelfType,
            (Self::Typing, "ClassVar") => Intrinsic::ClassVar,
            (Self::Typing, "Any") => Intrinsic::Any,
            (Self::Typing, "assert_never") => Intrinsic::AssertNever,
            (Self::Typing, "reveal_type") => Intrinsic::ObjectRuntimeExport("typing.reveal_type"),
            (Self::Typing, "assert_type") => Intrinsic::ObjectRuntimeExport("typing.assert_type"),
            (Self::Typing, "cast") => Intrinsic::Cast,
            (Self::Typing | Self::CollectionsAbc, "Callable") => Intrinsic::Callable,
            (Self::Typing, "Sequence") => Intrinsic::TYPING_SEQUENCE,
            (Self::CollectionsAbc, "Mapping") => Intrinsic::Mapping,
            (Self::Typing | Self::CollectionsAbc, "Iterable") => Intrinsic::Iterable,
            (Self::Typing | Self::CollectionsAbc, "Iterator") => Intrinsic::Iterator,
            (Self::Io, "UnsupportedOperation") => Intrinsic::UnsupportedOperation,
            (Self::Gzip, "open") => Intrinsic::GzipOpen,
            (Self::Types, "get_original_bases") => {
                Intrinsic::ObjectRuntimeExport("types.get_original_bases")
            }
            (Self::Types, "new_class") => Intrinsic::ObjectRuntimeExport("types.new_class"),
            (Self::Types, "prepare_class") => Intrinsic::ObjectRuntimeExport("types.prepare_class"),
            (Self::Types, "resolve_bases") => Intrinsic::ObjectRuntimeExport("types.resolve_bases"),
            (Self::Types, "MappingProxyType") => {
                Intrinsic::ObjectRuntimeExport("types.MappingProxyType")
            }
            (Self::Types, "ModuleType") => Intrinsic::ModuleType,
            (Self::Types, "SimpleNamespace") => Intrinsic::SimpleNamespace,
            (Self::Types, "TracebackType") => Intrinsic::NominalClass("traceback"),
            (Self::Types, "FrameType") => Intrinsic::NominalClass("frame"),
            (Self::Types, "CodeType") => Intrinsic::NominalClass("code"),
            (Self::Codecs, "IncrementalDecoder") => {
                Intrinsic::ObjectRuntimeExport("codecs.IncrementalDecoder")
            }
            (Self::Codecs, "Codec") => Intrinsic::ObjectRuntimeExport("codecs.Codec"),
            (Self::Codecs, "StreamWriter") => Intrinsic::ObjectRuntimeExport("codecs.StreamWriter"),
            (Self::Codecs, "CodecInfo") => Intrinsic::ObjectRuntimeExport("codecs.CodecInfo"),
            (Self::Threading, "_RLock") => Intrinsic::ObjectRuntimeExport("threading._RLock"),
            (Self::Threading, "_PyRLock") => Intrinsic::ObjectRuntimeExport("threading._PyRLock"),
            (Self::Threading, "_CRLock") => Intrinsic::ObjectRuntimeExport("threading._CRLock"),
            (Self::Threading, "Semaphore") => Intrinsic::ObjectRuntimeExport("threading.Semaphore"),
            (Self::Threading, "BoundedSemaphore") => {
                Intrinsic::ObjectRuntimeExport("threading.BoundedSemaphore")
            }
            (Self::Threading, "Barrier") => Intrinsic::ObjectRuntimeExport("threading.Barrier"),
            (Self::Threading, "BrokenBarrierError") => {
                Intrinsic::ObjectRuntimeExport("threading.BrokenBarrierError")
            }
            (Self::Threading, "Condition") => Intrinsic::ObjectRuntimeExport("threading.Condition"),
            (Self::Threading, "Event") => Intrinsic::ObjectRuntimeExport("threading.Event"),
            (Self::Threading, "Thread") => Intrinsic::ObjectRuntimeExport("threading.Thread"),
            (Self::Threading, "_dangling") => Intrinsic::ObjectRuntimeExport("threading._dangling"),
            (Self::Threading, "main_thread") => {
                Intrinsic::ObjectRuntimeExport("threading.main_thread")
            }
            (Self::Threading, "currentThread") => {
                Intrinsic::ObjectRuntimeExport("threading.currentThread")
            }
            (Self::Threading, "current_thread") => {
                Intrinsic::ObjectRuntimeExport("threading.current_thread")
            }
            (Self::Threading, "enumerate") => Intrinsic::ObjectRuntimeExport("threading.enumerate"),
            (Self::Threading, "activeCount") => {
                Intrinsic::ObjectRuntimeExport("threading.activeCount")
            }
            (Self::Threading, "active_count") => {
                Intrinsic::ObjectRuntimeExport("threading.active_count")
            }
            (Self::Thread | Self::Threading, "get_ident") => {
                Intrinsic::ObjectRuntimeExport("_thread.get_ident")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Thread | Self::Threading, "get_native_id") => {
                Intrinsic::ObjectRuntimeExport("_thread.get_native_id")
            }
            (Self::Thread, "allocate_lock") => {
                Intrinsic::ObjectRuntimeExport("_thread.allocate_lock")
            }
            (Self::Thread, "allocate") => Intrinsic::ObjectRuntimeExport("_thread.allocate"),
            (Self::Thread | Self::Threading, "TIMEOUT_MAX") => {
                Intrinsic::ObjectRuntimeExport("_thread.TIMEOUT_MAX")
            }
            (Self::Thread, "_count") => Intrinsic::ObjectRuntimeExport("_thread._count"),
            (Self::Thread, "start_new_thread") => {
                Intrinsic::ObjectRuntimeExport("_thread.start_new_thread")
            }
            (Self::Threading, "Lock") | (Self::Thread, "LockType") => {
                Intrinsic::ObjectRuntimeExport("_thread.lock")
            }
            (Self::Threading, "local") | (Self::Thread, "_local") => {
                Intrinsic::ObjectRuntimeExport("_thread._local")
            }
            (Self::Thread | Self::Functools, "RLock") => {
                Intrinsic::ObjectRuntimeExport("_thread.RLock")
            }
            (Self::WeakRef, "WeakKeyDictionary") => {
                Intrinsic::ObjectRuntimeExport("weakref.WeakKeyDictionary")
            }
            (Self::WeakRef, "finalize") => Intrinsic::ObjectRuntimeExport("weakref.finalize"),
            (Self::WeakRef, "ref") => Intrinsic::ObjectRuntimeExport("weakref.ref"),
            (Self::WeakRef, "WeakMethod") => Intrinsic::ObjectRuntimeExport("weakref.WeakMethod"),
            (Self::WeakRef, "WeakSet") => Intrinsic::ObjectRuntimeExport("weakref.WeakSet"),
            (Self::WeakRef, "proxy") => Intrinsic::WeakProxy,
            (Self::WeakRef, "ProxyType") => Intrinsic::WeakProxyClass { callable: false },
            (Self::WeakRef, "CallableProxyType") => Intrinsic::WeakProxyClass { callable: true },
            (Self::UnicodeData, "decomposition") => {
                Intrinsic::ObjectRuntimeExport("unicodedata.decomposition")
            }
            (Self::UnicodeData, "normalize") => Intrinsic::UnicodeNormalize { check: false },
            (Self::UnicodeData, "is_normalized") => Intrinsic::UnicodeNormalize { check: true },
            (Self::Codecs, "lookup") => Intrinsic::CodecLookup,
            (Self::Codecs, "BOM_UTF8") => Intrinsic::CodecBom(b"\xef\xbb\xbf"),
            (Self::Codecs, "BOM_UTF16_LE") => Intrinsic::CodecBom(b"\xff\xfe"),
            (Self::Codecs, "BOM_UTF16_BE") => Intrinsic::CodecBom(b"\xfe\xff"),
            (Self::Codecs, "BOM_UTF32_LE") => Intrinsic::CodecBom(b"\xff\xfe\0\0"),
            (Self::Codecs, "BOM_UTF32_BE") => Intrinsic::CodecBom(b"\0\0\xfe\xff"),
            (Self::Socket | Self::SocketNative, "herror") => Intrinsic::HError,
            (Self::SocketNative, name) => Intrinsic::ObjectRuntimeExport(match name {
                "gaierror" => "_socket.gaierror",
                "getnameinfo" => "_socket.getnameinfo",
                "if_indextoname" => "_socket.if_indextoname",
                "if_nameindex" => "_socket.if_nameindex",
                "if_nametoindex" => "_socket.if_nametoindex",
                "NI_DGRAM" => "_socket.NI_DGRAM",
                "NI_NAMEREQD" => "_socket.NI_NAMEREQD",
                "NI_NOFQDN" => "_socket.NI_NOFQDN",
                "NI_NUMERICHOST" => "_socket.NI_NUMERICHOST",
                "NI_NUMERICSERV" => "_socket.NI_NUMERICSERV",
                "has_ipv6" => "_socket.has_ipv6",
                "SocketType" => "_socket.SocketType",
                "getaddrinfo" => "_socket.getaddrinfo",
                "AI_NUMERICSERV" => "_socket.AI_NUMERICSERV",
                "EAI_SERVICE" => "_socket.EAI_SERVICE",
                "EAI_NODATA" => "_socket.EAI_NODATA",
                "EAI_NONAME" => "_socket.EAI_NONAME",
                "AI_PASSIVE" => "_socket.AI_PASSIVE",
                "SOL_TCP" => "_socket.SOL_TCP",
                "AF_INET" => "_socket.AF_INET",
                "AF_INET6" => "_socket.AF_INET6",
                "AF_UNSPEC" => "_socket.AF_UNSPEC",
                "AF_UNIX" => "_socket.AF_UNIX",
                "SOCK_STREAM" => "_socket.SOCK_STREAM",
                "SOCK_DGRAM" => "_socket.SOCK_DGRAM",
                "SOCK_RAW" => "_socket.SOCK_RAW",
                "SOCK_RDM" => "_socket.SOCK_RDM",
                "SOCK_SEQPACKET" => "_socket.SOCK_SEQPACKET",
                "SOL_SOCKET" => "_socket.SOL_SOCKET",
                "IPPROTO_TCP" => "_socket.IPPROTO_TCP",
                "IPPROTO_UDP" => "_socket.IPPROTO_UDP",
                "IPPROTO_IPV6" => "_socket.IPPROTO_IPV6",
                "IPV6_V6ONLY" => "_socket.IPV6_V6ONLY",
                "SO_REUSEADDR" => "_socket.SO_REUSEADDR",
                "SO_REUSEPORT" => "_socket.SO_REUSEPORT",
                "SO_SNDBUF" => "_socket.SO_SNDBUF",
                "TCP_NODELAY" => "_socket.TCP_NODELAY",
                "SHUT_RD" => "_socket.SHUT_RD",
                "SHUT_WR" => "_socket.SHUT_WR",
                "SHUT_RDWR" => "_socket.SHUT_RDWR",
                "SOMAXCONN" => "_socket.SOMAXCONN",
                "socket" => "_socket.socket",
                "socketpair" => "_socket.socketpair",
                "close" => "_socket.close",
                "getservbyname" => "_socket.getservbyname",
                "getservbyport" => "_socket.getservbyport",
                "gethostname" => "_socket.gethostname",
                "gethostbyname" => "_socket.gethostbyname",
                "gethostbyaddr" => "_socket.gethostbyaddr",
                "error" => "_socket.error",
                _ => return None,
            }),
            (Self::Socket, "getnameinfo") => Intrinsic::ObjectRuntimeExport("socket.getnameinfo"),
            (Self::Socket, "if_indextoname") => {
                Intrinsic::ObjectRuntimeExport("socket.if_indextoname")
            }
            (Self::Socket, "if_nameindex") => Intrinsic::ObjectRuntimeExport("socket.if_nameindex"),
            (Self::Socket, "if_nametoindex") => {
                Intrinsic::ObjectRuntimeExport("socket.if_nametoindex")
            }
            (Self::Socket, "NI_DGRAM") => Intrinsic::ObjectRuntimeExport("socket.NI_DGRAM"),
            (Self::Socket, "NI_NAMEREQD") => Intrinsic::ObjectRuntimeExport("socket.NI_NAMEREQD"),
            (Self::Socket, "NI_NOFQDN") => Intrinsic::ObjectRuntimeExport("socket.NI_NOFQDN"),
            (Self::Socket, "NI_NUMERICHOST") => {
                Intrinsic::ObjectRuntimeExport("socket.NI_NUMERICHOST")
            }
            (Self::Socket, "NI_NUMERICSERV") => {
                Intrinsic::ObjectRuntimeExport("socket.NI_NUMERICSERV")
            }
            (Self::Socket, "AddressFamily") => {
                Intrinsic::ObjectRuntimeExport("socket.AddressFamily")
            }
            (Self::Socket, "SocketKind") => Intrinsic::ObjectRuntimeExport("socket.SocketKind"),
            (Self::Socket, "getaddrinfo") => Intrinsic::ObjectRuntimeExport("socket.getaddrinfo"),
            (Self::Socket, "AI_NUMERICSERV") => {
                Intrinsic::ObjectRuntimeExport("socket.AI_NUMERICSERV")
            }
            (Self::Socket, "AddressInfo") => Intrinsic::ObjectRuntimeExport("socket.AddressInfo"),
            (Self::Socket, "AI_PASSIVE") => Intrinsic::ObjectRuntimeExport("socket.AI_PASSIVE"),
            (Self::Socket, "EAI_NODATA") => Intrinsic::ObjectRuntimeExport("socket.EAI_NODATA"),
            (Self::Socket, "EAI_NONAME") => Intrinsic::ObjectRuntimeExport("socket.EAI_NONAME"),
            (Self::Socket, "EAI_SERVICE") => Intrinsic::ObjectRuntimeExport("socket.EAI_SERVICE"),
            (Self::Socket, "SOL_TCP") => Intrinsic::ObjectRuntimeExport("socket.SOL_TCP"),
            (Self::Socket, "SocketType") => Intrinsic::ObjectRuntimeExport("socket.SocketType"),
            (Self::Socket, "create_connection") => {
                Intrinsic::ObjectRuntimeExport("socket.create_connection")
            }
            (Self::Socket, "create_server") => {
                Intrinsic::ObjectRuntimeExport("socket.create_server")
            }
            (Self::Socket, "SO_REUSEPORT") => Intrinsic::ObjectRuntimeExport("socket.SO_REUSEPORT"),
            (Self::Socket, "has_dualstack_ipv6") => {
                Intrinsic::ObjectRuntimeExport("socket.has_dualstack_ipv6")
            }
            (Self::Socket, "IPPROTO_UDP") => Intrinsic::ObjectRuntimeExport("socket.IPPROTO_UDP"),
            (Self::Socket, "IPPROTO_IPV6") => Intrinsic::ObjectRuntimeExport("socket.IPPROTO_IPV6"),
            (Self::Socket, "IPV6_V6ONLY") => Intrinsic::ObjectRuntimeExport("socket.IPV6_V6ONLY"),
            (Self::Socket, "has_ipv6") => Intrinsic::ObjectRuntimeExport("socket.has_ipv6"),
            (Self::Socket, "AF_INET6") => Intrinsic::ObjectRuntimeExport("socket.AF_INET6"),
            (Self::Socket, "SOCK_DGRAM") => Intrinsic::ObjectRuntimeExport("socket.SOCK_DGRAM"),
            (Self::Socket, "SOCK_RAW") => Intrinsic::ObjectRuntimeExport("socket.SOCK_RAW"),
            (Self::Socket, "SOCK_RDM") => Intrinsic::ObjectRuntimeExport("socket.SOCK_RDM"),
            (Self::Socket, "SOCK_SEQPACKET") => {
                Intrinsic::ObjectRuntimeExport("socket.SOCK_SEQPACKET")
            }
            (Self::Socket, "socketpair") => Intrinsic::ObjectRuntimeExport("socket.socketpair"),
            (Self::Socket, "close") => Intrinsic::ObjectRuntimeExport("socket.close"),
            (Self::Socket, "getservbyname") => {
                Intrinsic::ObjectRuntimeExport("socket.getservbyname")
            }
            (Self::Socket, "getservbyport") => {
                Intrinsic::ObjectRuntimeExport("socket.getservbyport")
            }
            (Self::Socket, "gethostname") => Intrinsic::ObjectRuntimeExport("socket.gethostname"),
            (Self::Socket, "gethostbyname") => {
                Intrinsic::ObjectRuntimeExport("socket.gethostbyname")
            }
            (Self::Socket, "gethostbyaddr") => {
                Intrinsic::ObjectRuntimeExport("socket.gethostbyaddr")
            }
            (Self::Socket, "getfqdn") => Intrinsic::SocketGetFqdn,
            (Self::Socket, "socket") => Intrinsic::Socket,
            (Self::Socket, "gaierror") => Intrinsic::GaiError,
            (Self::Socket, name) => Intrinsic::SocketConstant(match name {
                "AF_INET" => "af_inet",
                "SOCK_STREAM" => "sock_stream",
                "SOL_SOCKET" => "sol_socket",
                "SO_REUSEADDR" => "so_reuseaddr",
                "IPPROTO_TCP" => "ipproto_tcp",
                "TCP_NODELAY" => "tcp_nodelay",
                "SOMAXCONN" => "somaxconn",
                "SO_SNDBUF" => "so_sndbuf",
                "SO_RCVBUF" => "so_rcvbuf",
                "SO_LINGER" => "so_linger",
                "MSG_PEEK" => "msg_peek",
                "SHUT_RD" => "shut_rd",
                "SHUT_WR" => "shut_wr",
                "SHUT_RDWR" => "shut_rdwr",

                _ => return None,
            }),
            (Self::Codecs, "getwriter") => Intrinsic::CodecGetWriter,
            (Self::Md5, "md5") => Intrinsic::ObjectRuntimeExport("_md5.md5"),
            (Self::Md5, "MD5Type") => Intrinsic::ObjectRuntimeExport("_md5.MD5Type"),
            (Self::Md5, "_GIL_MINSIZE") => Intrinsic::ObjectRuntimeExport("_md5._GIL_MINSIZE"),
            (Self::Sha3, "sha3_224") => Intrinsic::ObjectRuntimeExport("_sha3.sha3_224"),
            (Self::Sha3, "sha3_256") => Intrinsic::ObjectRuntimeExport("_sha3.sha3_256"),
            (Self::Sha3, "sha3_384") => Intrinsic::ObjectRuntimeExport("_sha3.sha3_384"),
            (Self::Sha3, "sha3_512") => Intrinsic::ObjectRuntimeExport("_sha3.sha3_512"),
            (Self::Sha3, "shake_128") => Intrinsic::ObjectRuntimeExport("_sha3.shake_128"),
            (Self::Sha3, "shake_256") => Intrinsic::ObjectRuntimeExport("_sha3.shake_256"),
            (Self::Sha3, "implementation") => {
                Intrinsic::ObjectRuntimeExport("_sha3.implementation")
            }
            (Self::Sha3, "_GIL_MINSIZE") => Intrinsic::ObjectRuntimeExport("_sha3._GIL_MINSIZE"),
            (Self::Blake2, "blake2b") => Intrinsic::ObjectRuntimeExport("_blake2.blake2b"),
            (Self::Blake2, "blake2s") => Intrinsic::ObjectRuntimeExport("_blake2.blake2s"),
            (Self::Blake2, "_GIL_MINSIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2._GIL_MINSIZE")
            }
            (Self::Blake2, "BLAKE2B_SALT_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2B_SALT_SIZE")
            }
            (Self::Blake2, "BLAKE2B_PERSON_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2B_PERSON_SIZE")
            }
            (Self::Blake2, "BLAKE2B_MAX_KEY_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2B_MAX_KEY_SIZE")
            }
            (Self::Blake2, "BLAKE2B_MAX_DIGEST_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2B_MAX_DIGEST_SIZE")
            }
            (Self::Blake2, "BLAKE2S_SALT_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2S_SALT_SIZE")
            }
            (Self::Blake2, "BLAKE2S_PERSON_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2S_PERSON_SIZE")
            }
            (Self::Blake2, "BLAKE2S_MAX_KEY_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2S_MAX_KEY_SIZE")
            }
            (Self::Blake2, "BLAKE2S_MAX_DIGEST_SIZE") => {
                Intrinsic::ObjectRuntimeExport("_blake2.BLAKE2S_MAX_DIGEST_SIZE")
            }
            (Self::Hashlib, "md5") => Intrinsic::ObjectRuntimeExport("hashlib.md5"),
            (Self::Hashlib, "sha1") => Intrinsic::Sha1,
            (Self::Hmac, "new") => Intrinsic::ObjectRuntimeExport("hmac.new"),
            (Self::Hmac, "HMAC") => Intrinsic::ObjectRuntimeExport("hmac.HMAC"),
            (Self::Hmac, "compare_digest") => Intrinsic::HmacCompareDigest,
            (Self::Hashlib, "sha256") => Intrinsic::Sha256,
            (Self::Hashlib, "sha224") => Intrinsic::NamedHash("sha224"),
            (Self::Hashlib, "sha384") => Intrinsic::NamedHash("sha384"),
            (Self::Hashlib, "sha512") => Intrinsic::NamedHash("sha512"),
            (Self::Hashlib, "blake2b") => Intrinsic::NamedHash("blake2b"),
            (Self::Hashlib, "blake2s") => Intrinsic::NamedHash("blake2s"),
            (Self::Hashlib, "sha3_224") => Intrinsic::NamedHash("sha3_224"),
            (Self::Hashlib, "sha3_256") => Intrinsic::NamedHash("sha3_256"),
            (Self::Hashlib, "sha3_384") => Intrinsic::NamedHash("sha3_384"),
            (Self::Hashlib, "sha3_512") => Intrinsic::NamedHash("sha3_512"),
            (Self::Hashlib, "shake_128") => Intrinsic::NamedHash("shake_128"),
            (Self::Hashlib, "shake_256") => Intrinsic::NamedHash("shake_256"),
            (Self::Base64, "urlsafe_b64encode") => Intrinsic::UrlsafeB64encode,
            (Self::Base64, "b64encode") => Intrinsic::Base64Encode,
            (Self::Base64, "b64decode") => Intrinsic::Base64Decode,
            (Self::Binascii, "unhexlify") => Intrinsic::BinasciiUnhexlify("binascii.unhexlify"),
            (Self::Binascii, "a2b_hex") => Intrinsic::BinasciiUnhexlify("binascii.a2b_hex"),
            (Self::Binascii, "a2b_base64") => Intrinsic::BinasciiDecode,
            (Self::Binascii, "b2a_base64") => Intrinsic::BinasciiEncode,
            (Self::Binascii, "Error") => Intrinsic::BinasciiError,
            (Self::Binascii, "Incomplete") => Intrinsic::BinasciiIncomplete,
            (Self::Hashlib, "pbkdf2_hmac") => Intrinsic::ObjectRuntimeExport("hashlib.pbkdf2_hmac"),
            (Self::Hashlib, "scrypt") => Intrinsic::ObjectRuntimeExport("hashlib.scrypt"),
            (Self::Hashlib, "file_digest") => Intrinsic::HashFileDigest,
            (Self::Hashlib, "new") => Intrinsic::HashNew,
            (Self::Hashlib, "__get_builtin_constructor") => {
                Intrinsic::ObjectRuntimeExport("hashlib.__get_builtin_constructor")
            }
            (Self::Hashlib, "__builtin_constructor_cache") => {
                Intrinsic::ObjectRuntimeExport("hashlib.__builtin_constructor_cache")
            }
            (Self::Hashlib, "algorithms_available") => {
                Intrinsic::HashAlgorithms { guaranteed: false }
            }
            (Self::Hashlib, "algorithms_guaranteed") => {
                Intrinsic::HashAlgorithms { guaranteed: true }
            }
            (Self::Zipfile, "ZipFile") => Intrinsic::ZipFile,
            (Self::Zipfile, "ZipInfo") => Intrinsic::ZipInfo,
            (Self::Io, "__all__") => Intrinsic::ObjectRuntimeExport("io.__all__"),
            (Self::Io | Self::IoNative, "BlockingIOError") => {
                Intrinsic::ObjectRuntimeExport("io.BlockingIOError")
            }
            (Self::Io, "Reader") => Intrinsic::ObjectRuntimeExport("io.Reader"),
            (Self::Io, "Writer") => Intrinsic::ObjectRuntimeExport("io.Writer"),
            (Self::Io, "SEEK_SET") => Intrinsic::IntConstant(0),
            (Self::Io, "SEEK_CUR") => Intrinsic::IntConstant(1),
            (Self::Io, "SEEK_END") => Intrinsic::IntConstant(2),
            (Self::Io | Self::IoNative, "text_encoding") => {
                Intrinsic::ObjectRuntimeExport("_io.text_encoding")
            }
            (Self::Io, "IncrementalNewlineDecoder") => {
                Intrinsic::ObjectRuntimeExport("io.IncrementalNewlineDecoder")
            }
            (Self::IoNative, "IncrementalNewlineDecoder") => {
                Intrinsic::ObjectRuntimeExport("_io.IncrementalNewlineDecoder")
            }
            (Self::Io, "TextIOWrapper") => Intrinsic::ObjectRuntimeExport("io.TextIOWrapper"),
            (Self::IoNative, "TextIOWrapper") => {
                Intrinsic::ObjectRuntimeExport("_io.TextIOWrapper")
            }
            (Self::Io, "open") => Intrinsic::ObjectRuntimeExport("io.open"),
            (Self::IoNative, "open") => Intrinsic::ObjectRuntimeExport("_io.open"),
            (Self::Io | Self::IoNative, "open_code") => {
                Intrinsic::ObjectRuntimeExport("io.open_code")
            }
            (Self::Io, "BufferedRandom") => Intrinsic::ObjectRuntimeExport("io.BufferedRandom"),
            (Self::IoNative, "BufferedRandom") => {
                Intrinsic::ObjectRuntimeExport("_io.BufferedRandom")
            }
            (Self::Io, "BufferedReader") => Intrinsic::ObjectRuntimeExport("io.BufferedReader"),
            (Self::IoNative, "BufferedReader") => {
                Intrinsic::ObjectRuntimeExport("_io.BufferedReader")
            }
            (Self::Io, "BufferedRWPair") => Intrinsic::ObjectRuntimeExport("io.BufferedRWPair"),
            (Self::IoNative, "BufferedRWPair") => {
                Intrinsic::ObjectRuntimeExport("_io.BufferedRWPair")
            }
            (Self::Io, "BufferedWriter") => Intrinsic::ObjectRuntimeExport("io.BufferedWriter"),
            (Self::IoNative, "BufferedWriter") => {
                Intrinsic::ObjectRuntimeExport("_io.BufferedWriter")
            }
            (Self::Io, "BytesIO") => Intrinsic::BytesIo,
            (Self::Io, "FileIO") => Intrinsic::ObjectRuntimeExport("io.FileIO"),
            (Self::IoNative, "FileIO") => Intrinsic::ObjectRuntimeExport("_io.FileIO"),
            (Self::IoNative, "_IOBase") => Intrinsic::ObjectRuntimeExport("_io._IOBase"),
            (Self::IoNative, "_RawIOBase") => Intrinsic::ObjectRuntimeExport("_io._RawIOBase"),
            (Self::IoNative, "_BufferedIOBase") => {
                Intrinsic::ObjectRuntimeExport("_io._BufferedIOBase")
            }
            (Self::IoNative, "_TextIOBase") => Intrinsic::ObjectRuntimeExport("_io._TextIOBase"),
            (Self::Io, "IOBase") => Intrinsic::IO_BASE,
            (Self::Io, "RawIOBase") => Intrinsic::RAW_IO_BASE,
            (Self::Io, "BufferedIOBase") => Intrinsic::BUFFERED_IO_BASE,
            (Self::Io, "TextIOBase") => Intrinsic::TEXT_IO_BASE,
            (Self::Io, "StringIO") => Intrinsic::StringIo,
            (Self::Zipfile, "BadZipFile") => Intrinsic::BadZipFile,
            (Self::Csv, "reader") => Intrinsic::CsvReader,
            (Self::StringFormatting, "formatter_parser") => Intrinsic::FormatterParser,
            (Self::StringFormatting, "formatter_field_name_split") => {
                Intrinsic::FormatterFieldNameSplit
            }
            (Self::Csv, "Error") => Intrinsic::CsvError,
            (Self::Gzip, "BadGzipFile") => Intrinsic::BadGzipFile,
            (Self::Re, "escape") => Intrinsic::ObjectRuntimeExport("re.escape"),
            (Self::Re, "compile") => Intrinsic::RegexCompile,
            (Self::Re, "search") => Intrinsic::RegexSearch,
            (Self::Re, "match") => Intrinsic::RegexMatchStart,
            (Self::Re, "findall") => Intrinsic::ObjectRuntimeExport("re.findall"),
            (Self::Re, "finditer") => Intrinsic::ObjectRuntimeExport("re.finditer"),
            (Self::Re, "Scanner") => Intrinsic::ObjectRuntimeExport("re.Scanner"),
            (Self::Re, "split") => Intrinsic::ObjectRuntimeExport("re.split"),
            (Self::Re, "sub") => Intrinsic::RegexSub,
            (Self::Re, "subn") => Intrinsic::ObjectRuntimeExport("re.subn"),
            (Self::Re, "VERBOSE") => Intrinsic::RegexVerbose,
            (Self::Re, "UNICODE") => Intrinsic::RegexUnicode,
            (Self::Re, "Pattern") => Intrinsic::RegexPattern,
            (Self::Re, "Match") => Intrinsic::RegexMatch,
            (Self::Enum, "StrEnum") => Intrinsic::StrEnum,
            (Self::Enum, "IntEnum") => Intrinsic::IntEnum,
            (Self::Enum, "IntFlag") => Intrinsic::ObjectRuntimeExport("enum.IntFlag"),
            (Self::Enum, "global_enum") => Intrinsic::ObjectRuntimeExport("enum.global_enum"),
            (Self::Enum, "global_enum_repr") => {
                Intrinsic::ObjectRuntimeExport("enum.global_enum_repr")
            }
            (Self::Enum, "global_flag_repr") => {
                Intrinsic::ObjectRuntimeExport("enum.global_flag_repr")
            }
            (Self::Enum, "global_str") => Intrinsic::ObjectRuntimeExport("enum.global_str"),
            (Self::Enum, "_simple_enum") => Intrinsic::SimpleEnum,
            (Self::Enum, "auto") => Intrinsic::EnumAuto,
            (Self::Collections | Self::CollectionsNative, "_count_elements") => {
                Intrinsic::CountElements
            }
            (Self::Collections | Self::CollectionsNative, "defaultdict") => Intrinsic::DefaultDict,
            (Self::Collections | Self::CollectionsNative, "deque") => Intrinsic::Deque,
            (Self::Collections, "ChainMap") => Intrinsic::ChainMap,
            (Self::Collections | Self::CollectionsNative, "OrderedDict") => Intrinsic::OrderedDict,
            (Self::Collections, "namedtuple") => Intrinsic::NamedTuple,
            (Self::Collections, "Counter") => Intrinsic::Counter,
            (Self::Collections, "UserDict") => {
                Intrinsic::ObjectRuntimeExport("collections.UserDict")
            }
            (Self::Collections, "UserList") => {
                Intrinsic::ObjectRuntimeExport("collections.UserList")
            }
            (Self::Collections, "UserString") => {
                Intrinsic::ObjectRuntimeExport("collections.UserString")
            }
            (Self::Operator, "attrgetter") => Intrinsic::AttrGetter,
            (Self::Operator, "itemgetter") => Intrinsic::ObjectRuntimeExport("operator.itemgetter"),
            (Self::Operator, "length_hint") => Intrinsic::LengthHint,
            (Self::Math, "isqrt") => Intrinsic::ObjectRuntimeExport("math.isqrt"),
            (Self::Math, "prod") => Intrinsic::MathProd,
            (Self::Copy, "copy") => Intrinsic::ShallowCopy,
            (Self::Copy, "deepcopy") => Intrinsic::DeepCopy,
            (Self::Itertools, "product") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::Product)
            }
            (Self::Itertools, "permutations") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::Permutations)
            }
            (Self::Itertools, "combinations") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::Combinations)
            }
            (Self::Itertools, "combinations_with_replacement") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::CombinationsWithReplacement)
            }
            (Self::Itertools, "batched") => Intrinsic::ObjectRuntimeExport("itertools.batched"),
            (Self::Itertools, "count") => Intrinsic::Count,
            (Self::Itertools, "filterfalse") => {
                Intrinsic::ObjectRuntimeExport("itertools.filterfalse")
            }
            (Self::Itertools, "tee") => Intrinsic::Tee,
            (Self::Itertools, "islice") => Intrinsic::Islice,
            (Self::Itertools, "cycle") => Intrinsic::Cycle,
            (Self::Itertools, "repeat") => Intrinsic::Repeat,
            (Self::Itertools, "groupby") => Intrinsic::GroupBy,
            (Self::Itertools, "chain") => Intrinsic::Chain,
            (Self::CollectionsAbc, "Set") => Intrinsic::AbstractSet,
            (Self::Tomllib, "load") => Intrinsic::TomllibLoad,
            (Self::Tomllib, "TOMLDecodeError") => Intrinsic::TomlDecodeError,
            (Self::Textwrap, "dedent") => Intrinsic::Dedent,
            (Self::Zlib, "error") => Intrinsic::ZlibError,
            (Self::Zlib, "adler32") => Intrinsic::Adler32,
            (Self::Zlib, "compress") => Intrinsic::ObjectRuntimeExport("zlib.compress"),
            (Self::Zlib, "decompress") => Intrinsic::ObjectRuntimeExport("zlib.decompress"),
            (Self::Zlib, "crc32") => Intrinsic::ObjectRuntimeExport("zlib.crc32"),
            (Self::ImreJson, "decode") => Intrinsic::JsonDecode,
            (Self::JsonNative, "make_scanner") => {
                Intrinsic::ObjectRuntimeExport("_json.make_scanner")
            }
            (Self::JsonNative, "make_encoder") => {
                Intrinsic::ObjectRuntimeExport("_json.make_encoder")
            }
            (Self::JsonNative, "scanstring") => Intrinsic::ObjectRuntimeExport("_json.scanstring"),
            (Self::JsonNative, "encode_basestring") => {
                Intrinsic::ObjectRuntimeExport("_json.encode_basestring")
            }
            (Self::JsonNative, "encode_basestring_ascii") => {
                Intrinsic::ObjectRuntimeExport("_json.encode_basestring_ascii")
            }
            (Self::Json, "loads") => Intrinsic::JsonLoads,
            (Self::Json | Self::JsonDecoder, "JSONDecoder") => Intrinsic::JsonDecoderClass,
            (Self::Json | Self::JsonEncoder, "JSONEncoder") => Intrinsic::JsonEncoderClass,
            (Self::Json, "load") => Intrinsic::JsonLoad,
            (Self::Json, "dumps") => Intrinsic::JsonDumps,
            (Self::Json, "dump") => Intrinsic::JsonDump,
            (Self::Json, "JSONDecodeError") => Intrinsic::JsonDecodeError,
            (Self::ImreJson, "encode") => Intrinsic::JsonEncode,
            (Self::ImreToml, "decode") => Intrinsic::TomlDecode,
            (Self::Imre, "struct") => Intrinsic::Struct,
            (Self::Dataclasses, "dataclass") => Intrinsic::Dataclass,
            (Self::Dataclasses, "field") => Intrinsic::DataclassField,
            (Self::Dataclasses, "make_dataclass") => {
                Intrinsic::ObjectRuntimeExport("dataclasses.make_dataclass")
            }
            (Self::Dataclasses, "Field") => Intrinsic::ObjectRuntimeExport("dataclasses.Field"),
            (Self::Dataclasses, "replace") => Intrinsic::ObjectRuntimeExport("dataclasses.replace"),
            (Self::Dataclasses, "asdict") => Intrinsic::AsDict,
            (Self::Dataclasses, "InitVar") => Intrinsic::ObjectRuntimeExport("dataclasses.InitVar"),
            (Self::Pathlib, "Path" | "PosixPath") => Intrinsic::Path,
            (Self::Pathlib, "PurePath" | "PurePosixPath") => {
                Intrinsic::PurePath(crate::ir::PathFlavor::Posix)
            }
            (Self::Pathlib, "PureWindowsPath") => {
                Intrinsic::PurePath(crate::ir::PathFlavor::Windows)
            }
            (Self::Fcntl, "fcntl") => Intrinsic::ObjectRuntimeExport("fcntl.fcntl"),
            (Self::Fcntl, "FD_CLOEXEC") => Intrinsic::ObjectRuntimeExport("fcntl.FD_CLOEXEC"),
            (Self::Fcntl, "F_GETFD") => Intrinsic::ObjectRuntimeExport("fcntl.F_GETFD"),
            (Self::Fcntl, "F_SETFD") => Intrinsic::ObjectRuntimeExport("fcntl.F_SETFD"),
            (Self::Fcntl, "F_GETFL") => Intrinsic::ObjectRuntimeExport("fcntl.F_GETFL"),
            (Self::Fcntl, "F_SETFL") => Intrinsic::ObjectRuntimeExport("fcntl.F_SETFL"),
            (Self::Fcntl, "F_GETPIPE_SZ") => Intrinsic::ObjectRuntimeExport("fcntl.F_GETPIPE_SZ"),
            (Self::Fcntl, "F_SETPIPE_SZ") => Intrinsic::ObjectRuntimeExport("fcntl.F_SETPIPE_SZ"),
            (Self::Resource, "getrlimit") => Intrinsic::ObjectRuntimeExport("resource.getrlimit"),
            (Self::Resource, "setrlimit") => Intrinsic::ObjectRuntimeExport("resource.setrlimit"),
            (Self::Resource, "RLIMIT_CORE") => {
                Intrinsic::ObjectRuntimeExport("resource.RLIMIT_CORE")
            }
            (Self::Resource, "RLIMIT_STACK") => {
                Intrinsic::ObjectRuntimeExport("resource.RLIMIT_STACK")
            }
            (Self::Resource, "RLIM_INFINITY") => {
                Intrinsic::ObjectRuntimeExport("resource.RLIM_INFINITY")
            }
            (Self::Random, "shuffle") => Intrinsic::Random(crate::ir::RandomOperation::Shuffle),
            (Self::Random, "choices") => Intrinsic::ObjectRuntimeExport("random.choices"),
            (Self::Random, "choice") => Intrinsic::RandomChoice { secure: false },
            (Self::Secrets, "choice") => Intrinsic::RandomChoice { secure: true },
            (Self::Random, "seed") => Intrinsic::Random(crate::ir::RandomOperation::Seed),
            (Self::Random, "random") => Intrinsic::Random(crate::ir::RandomOperation::Float),
            (Self::Random, "getrandbits") => Intrinsic::Random(crate::ir::RandomOperation::Bits),
            (Self::Random, "randint") => Intrinsic::Random(crate::ir::RandomOperation::Randint),
            (Self::Random, "randrange") => Intrinsic::Random(crate::ir::RandomOperation::Randrange),
            (Self::Shutil, "rmtree") => Intrinsic::RmTree,
            (Self::Shutil, "copytree") => Intrinsic::CopyTree,
            (Self::Shutil, "move") => Intrinsic::Move,
            (Self::Shutil, "_destinsrc") => Intrinsic::DestInSrc,
            (Self::Shutil, "ignore_patterns") => Intrinsic::IgnorePatterns,
            (Self::Shutil, "which") => Intrinsic::Which,
            (Self::Shutil, "copyfile") => Intrinsic::Copy(CopyOperation::File),
            (Self::Shutil, "copymode") => Intrinsic::Copy(CopyOperation::Mode),
            (Self::Shutil, "copystat") => Intrinsic::Copy(CopyOperation::Stat),
            (Self::Shutil, "_copyxattr") => Intrinsic::Copy(CopyOperation::Xattr),
            (Self::Shutil, "copy") => Intrinsic::Copy(CopyOperation::Copy),
            (Self::Shutil, "copy2") => Intrinsic::Copy(CopyOperation::Copy2),
            (Self::Shutil, "Error") => Intrinsic::ShutilError,
            (Self::Shutil, "SameFileError") => Intrinsic::SameFileError,
            (Self::Shutil, "SpecialFileError") => Intrinsic::SpecialFileError,
            (Self::Tempfile, "TemporaryDirectory") => Intrinsic::TemporaryDirectory,
            (Self::Tempfile, "TemporaryFile") => Intrinsic::TemporaryFile,
            (Self::Subprocess, "STDOUT") => Intrinsic::SubprocessStdout,
            (Self::Subprocess, "Popen") => Intrinsic::Popen,
            (Self::Subprocess, "_active") => Intrinsic::PopenActive,
            (Self::Subprocess, "_cleanup") => Intrinsic::PopenCleanup,
            (Self::Subprocess, "_PopenSelector") => Intrinsic::PopenSelector,
            (Self::Selectors, "SelectSelector") => Intrinsic::SelectorClass(true),
            (Self::Selectors, "PollSelector") => Intrinsic::SelectorClass(false),
            (Self::Subprocess, "run") => Intrinsic::SubprocessRun,
            (Self::Subprocess, "call") => Intrinsic::SubprocessCall,
            (Self::Subprocess, "check_call") => Intrinsic::SubprocessCheckCall,
            (Self::Subprocess, "list2cmdline") => Intrinsic::SubprocessList2Cmdline,
            (Self::Subprocess, "CompletedProcess") => Intrinsic::CompletedProcess,
            (Self::Subprocess, "SubprocessError") => Intrinsic::SubprocessError,
            (Self::Subprocess, "CalledProcessError") => Intrinsic::CalledProcessError,
            (Self::Subprocess, "TimeoutExpired") => Intrinsic::TimeoutExpired,
            (Self::Posix, "_path_splitroot_ex") | (Self::PosixPath, "splitroot") => {
                Intrinsic::ObjectRuntimeExport("posix._path_splitroot_ex")
            }
            (Self::Posix, name) => os::symbol(name)?,
            (Self::Os, "makedirs") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os.makedirs"),
            (Self::Os, "removedirs") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.removedirs")
            }
            (Self::Os, "walk") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os.walk"),
            (Self::Os, "fwalk") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os.fwalk"),
            (Self::Os, "F_OK") => Intrinsic::AccessMode(0),
            (Self::Os, "X_OK") => Intrinsic::AccessMode(1),
            (Self::Os, "W_OK") => Intrinsic::AccessMode(2),
            (Self::Os, "R_OK") => Intrinsic::AccessMode(4),
            (Self::Os, "supports_dir_fd") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_dir_fd")
            }
            (Self::Os, "supports_fd") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_fd")
            }
            (Self::Os, "supports_follow_symlinks") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_follow_symlinks")
            }
            (Self::Os, "supports_effective_ids") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_effective_ids")
            }
            (Self::Os, "environ") => Intrinsic::Environ,
            (Self::Os, "environb") if cfg!(unix) => Intrinsic::EnvironBytes,
            (Self::Os, "supports_bytes_environ") => Intrinsic::BoolConstant(cfg!(unix)),
            (Self::Os, "getenv") => Intrinsic::GetEnv,
            (Self::Os, "getcwd") => Intrinsic::GetCwd,
            (Self::Os, "fspath") => Intrinsic::FsPath,
            (Self::Os, "_fspath") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os._fspath"),
            (Self::Os, "fsdecode") => Intrinsic::FsDecode,
            (Self::Os, "fsencode") => Intrinsic::FsEncode,
            (Self::PosixPath, "normcase") => Intrinsic::PosixNormcase,
            (Self::Os, "devnull") => Intrinsic::StringConstant("/dev/null"),
            (Self::Os | Self::OsPath | Self::PosixPath, "extsep") => Intrinsic::StringConstant("."),
            (Self::GenericPath | Self::PosixPath, "commonprefix") => {
                Intrinsic::ObjectRuntimeExport("genericpath.commonprefix")
            }
            (Self::GenericPath | Self::PosixPath, "getatime") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getatime")
            }
            (Self::GenericPath | Self::PosixPath, "getctime") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getctime")
            }
            (Self::GenericPath | Self::PosixPath, "getmtime") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getmtime")
            }
            (Self::GenericPath | Self::PosixPath, "isjunction") => {
                Intrinsic::ObjectRuntimeExport("genericpath.isjunction")
            }
            (Self::GenericPath | Self::PosixPath, "lexists") => {
                Intrinsic::ObjectRuntimeExport("genericpath.lexists")
            }
            (Self::GenericPath | Self::PosixPath, "getsize") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getsize")
            }
            (Self::GenericPath | Self::PosixPath, "samefile") => {
                Intrinsic::ObjectRuntimeExport("genericpath.samefile")
            }
            (Self::GenericPath | Self::PosixPath, "sameopenfile") => {
                Intrinsic::ObjectRuntimeExport("genericpath.sameopenfile")
            }
            (Self::GenericPath | Self::PosixPath, "samestat") => {
                Intrinsic::ObjectRuntimeExport("genericpath.samestat")
            }
            (Self::GenericPath | Self::PosixPath, "exists") => Intrinsic::OsPathExists,
            (Self::PosixPath, "commonpath") => Intrinsic::PosixPath(PosixPathOperation::Commonpath),
            (Self::PosixPath, "join") => Intrinsic::PosixPath(PosixPathOperation::Join),
            (Self::PosixPath, "basename") => Intrinsic::PosixPath(PosixPathOperation::Basename),
            (Self::PosixPath, "dirname") => Intrinsic::PosixPath(PosixPathOperation::Dirname),
            (Self::PosixPath, "ismount") => Intrinsic::ObjectRuntimeExport("posixpath.ismount"),
            (Self::PosixPath, "split") => Intrinsic::ObjectRuntimeExport("posixpath.split"),
            (Self::PosixPath, "splitext") => Intrinsic::PosixPath(PosixPathOperation::Splitext),
            (Self::PosixPath, "normpath") => Intrinsic::PosixPath(PosixPathOperation::Normpath),
            (Self::PosixPath, "abspath") => Intrinsic::PosixPath(PosixPathOperation::Abspath),
            (Self::UrllibRequest, "Request") => {
                Intrinsic::ScopedObjectExport("urllib.request.Request")
            }
            (Self::UrllibRequest, "urlopen") => {
                Intrinsic::ScopedObjectExport("urllib.request.urlopen")
            }
            (Self::UrllibRequest, "urlretrieve") => {
                Intrinsic::ScopedObjectExport("urllib.request.urlretrieve")
            }
            (Self::UrllibRequest, "urlcleanup") => {
                Intrinsic::ScopedObjectExport("urllib.request.urlcleanup")
            }
            (Self::UrllibRequest, "build_opener") => {
                Intrinsic::ScopedObjectExport("urllib.request.build_opener")
            }
            (Self::UrllibRequest, "install_opener") => {
                Intrinsic::ScopedObjectExport("urllib.request.install_opener")
            }
            (Self::UrllibError, "URLError") => {
                Intrinsic::ScopedObjectExport("urllib.error.URLError")
            }
            (Self::UrllibError, "HTTPError") => {
                Intrinsic::ScopedObjectExport("urllib.error.HTTPError")
            }
            (Self::UrllibError, "ContentTooShortError") => {
                Intrinsic::ScopedObjectExport("urllib.error.ContentTooShortError")
            }
            (Self::UrllibParse, "uses_relative") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.uses_relative")
            }
            (Self::UrllibParse, "uses_netloc") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.uses_netloc")
            }
            (Self::UrllibParse, "quote") => Intrinsic::Quote,
            (Self::UrllibParse, "quote_from_bytes") => Intrinsic::QuoteFromBytes,
            (Self::UrllibParse, "quote_plus") => Intrinsic::QuotePlus,
            (Self::UrllibParse, "urlencode") => Intrinsic::UrlEncode,
            (Self::UrllibParse, "unquote") => Intrinsic::Unquote,
            (Self::UrllibParse, "parse_qs") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.parse_qs")
            }
            (Self::UrllibParse, "unquote_to_bytes") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.unquote_to_bytes")
            }
            (Self::UrllibParse, "unquote_plus") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.unquote_plus")
            }
            (Self::EmailHeaderValueParser, "get_addr_spec") => {
                Intrinsic::ObjectRuntimeExport("email._header_value_parser.get_addr_spec")
            }
            (Self::EmailHeaderValueParser, "get_angle_addr") => {
                Intrinsic::ObjectRuntimeExport("email._header_value_parser.get_angle_addr")
            }
            (Self::EmailUtils, "parsedate_to_datetime") => Intrinsic::EmailParseDate,
            (Self::EmailUtils, "format_datetime") => Intrinsic::EmailFormatDateTime,
            (Self::EmailUtils, "formatdate") => Intrinsic::EmailFormatDate,
            (Self::UrllibParse, "urljoin") => Intrinsic::UrlJoin,
            (Self::UrllibParse, "parse_qsl") => Intrinsic::ParseQsl,
            (Self::UrllibParse, "splittype") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splittype")
            }
            (Self::UrllibParse, "_splittype") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splittype")
            }
            (Self::UrllibParse, "splithost") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splithost")
            }
            (Self::UrllibParse, "_splithost") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splithost")
            }
            (Self::UrllibParse, "splituser") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splituser")
            }
            (Self::UrllibParse, "_splituser") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splituser")
            }
            (Self::UrllibParse, "splitpasswd") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splitpasswd")
            }
            (Self::UrllibParse, "_splitpasswd") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splitpasswd")
            }
            (Self::UrllibParse, "splitport") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splitport")
            }
            (Self::UrllibParse, "_splitport") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splitport")
            }
            (Self::UrllibParse, "splitnport") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splitnport")
            }
            (Self::UrllibParse, "_splitnport") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splitnport")
            }
            (Self::UrllibParse, "splitquery") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splitquery")
            }
            (Self::UrllibParse, "_splitquery") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splitquery")
            }
            (Self::UrllibParse, "splittag") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splittag")
            }
            (Self::UrllibParse, "_splittag") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splittag")
            }
            (Self::UrllibParse, "splitattr") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splitattr")
            }
            (Self::UrllibParse, "_splitattr") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splitattr")
            }
            (Self::UrllibParse, "splitvalue") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.splitvalue")
            }
            (Self::UrllibParse, "_splitvalue") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._splitvalue")
            }
            (Self::UrllibParse, "to_bytes") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.to_bytes")
            }
            (Self::UrllibParse, "_to_bytes") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse._to_bytes")
            }
            (Self::UrllibParse, "unwrap") => Intrinsic::ObjectRuntimeExport("urllib.parse.unwrap"),
            (Self::UrllibParse, "urldefrag") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.urldefrag")
            }
            (Self::UrllibParse, "ParseResult") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.ParseResult")
            }
            (Self::UrllibParse, "ParseResultBytes") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.ParseResultBytes")
            }
            (Self::UrllibParse, "DefragResult") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.DefragResult")
            }
            (Self::UrllibParse, "DefragResultBytes") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.DefragResultBytes")
            }
            (Self::UrllibParse, "clear_cache") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.clear_cache")
            }
            (Self::UrllibParse, "urlparse") => {
                Intrinsic::ObjectRuntimeExport("urllib.parse.urlparse")
            }
            (Self::UrllibParse, "urlsplit") => Intrinsic::UrlSplit,
            (Self::UrllibParse, "urlunsplit") => Intrinsic::UrlUnsplit,
            (Self::UrllibParse, "SplitResult") => Intrinsic::SplitResult { bytes: false },
            (Self::UrllibParse, "SplitResultBytes") => Intrinsic::SplitResult { bytes: true },
            _ => return declarations::symbol(self, name),
        };
        Some(Binding::Intrinsic(symbol))
    }
}

pub(crate) struct TypeAlias {
    pub(crate) value: ast::Expr,
    pub(crate) constructor: bool,
}

pub(crate) struct SourceModule {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) bundled: bool,
    pub(crate) parsed: ParsedModuleRef,
    pub(crate) bindings: HashMap<String, Binding>,
    type_bindings: HashMap<String, Binding>,
    /// Only an executed explicit submodule import populates the bare importlib namespace.
    importlib_children: HashMap<&'static str, TextSize>,
    local_static_imports: Vec<LocalStaticImport>,
    pub(crate) aliases: Vec<TypeAlias>,
    /// Decorated declarations register their original function at module initialization.
    pub(crate) overloads: HashSet<usize>,
    pub(crate) type_guards: Vec<usize>,
    /// Entry bindings and guards that run during initialization, before later definitions.
    pub(crate) entry_globals: Vec<usize>,
    /// Guarded standard-library imports that run in source order through the object importer.
    pub(crate) optional_imports: HashMap<TextRange, String>,
    /// First assignment of each module value that needs a rebindable cell.
    pub(crate) rebound_values: HashSet<usize>,
    /// Mutable intrinsic exports read at the source from-import, keyed by statement.
    pub(crate) snapshots: HashMap<usize, Intrinsic>,
    /// Executable entry statements, or inactive main guards that still require checking.
    pub(crate) executable: Vec<usize>,
    is_package: bool,
    search_roots: SearchRoots,
    /// A rebuilt module is declared by a .pyi beside its platform-specific archive.
    pub(crate) archive: Option<PathBuf>,
}

enum SearchRoots {
    Single(usize),
    Namespace(Vec<usize>),
}

impl SearchRoots {
    fn contains(&self, index: usize) -> bool {
        match self {
            Self::Single(root) => *root == index,
            Self::Namespace(roots) => roots.contains(&index),
        }
    }
}

/// These public names are supplied by the boxed unittest.mock module.
pub(crate) fn boxed_mock_export(name: &str) -> bool {
    matches!(
        name,
        "sentinel"
            | "DEFAULT"
            | "ANY"
            | "MagicMock"
            | "mock_open"
            | "seal"
            | "call"
            | "_Call"
            | "_CallList"
    )
}

impl SourceModule {
    /// An entry capability guard runs before its subsequent declarations and calls.
    pub(crate) fn has_entry_guard(&self) -> bool {
        self.entry_globals
            .iter()
            .any(|&index| matches!(self.parsed.syntax().body[index], ast::Stmt::If(_)))
    }

    fn local_value(&self, id: ModuleId, statement: &ast::Stmt) -> Option<DefinitionId> {
        let name = value_name(statement)?;
        let Binding::Value(first) = *self.bindings.get(name)? else {
            return None;
        };
        (first.module == id
            && value_name(&self.parsed.syntax().body[first.statement]) == Some(name))
        .then_some(first)
    }

    pub(crate) fn rebound_target(
        &self,
        id: ModuleId,
        statement: &ast::Stmt,
    ) -> Option<DefinitionId> {
        self.local_value(id, statement)
            .filter(|first| self.rebound_values.contains(&first.statement))
    }

    pub(crate) fn is_namespace(&self) -> bool {
        matches!(self.search_roots, SearchRoots::Namespace(_))
    }

    /// Bundled contexts whose enter returns the receiver and whose exit closes it.
    pub(crate) fn closes_context(&self, class: &str) -> bool {
        (self.is_bundled("selectors") && class == "PollSelector")
            || (self.is_bundled(TEMPFILE_SPOOL) && class == "SpooledTemporaryFile")
    }

    pub(crate) fn is_bundled(&self, name: &str) -> bool {
        self.bundled && self.name == name
    }

    /// A type-only import in a native stub does not add a member to the C module.
    fn export(&self, name: &str) -> Option<Binding> {
        if self.is_bundled("socketserver")
            && matches!(
                name,
                "DatagramRequestHandler"
                    | "UDPServer"
                    | "ThreadingMixIn"
                    | "ThreadingTCPServer"
                    | "ThreadingUDPServer"
            )
        {
            return Some(Binding::Intrinsic(Intrinsic::SOCKETSERVER_CLASS));
        }
        if self.is_bundled("unittest") {
            let export = match name {
                "registerResult" => Some("unittest.registerResult"),
                "removeResult" => Some("unittest.removeResult"),
                _ => None,
            };
            if let Some(export) = export {
                return Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(export)));
            }
        }
        if self.is_bundled("unittest.mock") && boxed_mock_export(name) {
            return Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                "unittest.mock objects",
            )));
        }
        let binding = *self.bindings.get(name)?;
        (self.archive.is_none() || matches!(binding, Binding::Definition(_))).then_some(binding)
    }

    pub(crate) fn error(&self, node: &impl Ranged, message: &str) -> String {
        let prefix = &self.source[..usize::from(node.start())];
        let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        format!("{}:{line}:{column}: Imre: {message}", self.path.display())
    }
}

pub(crate) struct SourceGraph {
    pub(crate) command_startup: Option<(crate::CommandFlags, String)>,
    pub(crate) virtual_command: bool,
    pub(crate) handled_tracebacks: bool,
    pub(crate) traceback_objects: bool,
    pub(crate) empty_keywords: empty_keywords::EmptyKeywords,
    checked_diagnostics: std::cell::RefCell<Vec<Vec<CheckedDiagnostic>>>,
    pub(crate) needs_object_runtime: std::cell::Cell<bool>,
    pub(crate) class_body_error: std::cell::RefCell<Option<String>>,
    pub(crate) object_builtin_errors: std::cell::RefCell<HashSet<String>>,
    pub(crate) deferred_object_error: std::cell::RefCell<Option<String>>,
    pub(crate) return_errors: std::cell::RefCell<HashSet<String>>,
    pub(crate) entry_profile: entry_profile::Profile,
    pub(crate) mutable_documents: bool,
    pub(crate) mutable_builtin_len: bool,
    reachable: Option<reachability::Reachable>,
    required_library: Option<(ModuleId, reachability::Reachable)>,
    pub(crate) inputs: crate::SourceInputs,
    pub(crate) session: Session,
    pub(crate) modules: Vec<SourceModule>,
    pub(crate) entry: ModuleId,
    ordered_bindings: Option<ordered_imports::Bindings>,
    /// The absolute entry spelling used for Python's `__file__`.
    entry_filename: PathBuf,
    /// Split literal prefixes at imports so allocations and package exports are
    /// available to the child at the original point in Python's initialization.
    initialization: Vec<(ModuleId, Range<usize>)>,
    pub(crate) errno_names: Vec<String>,
    pub(crate) doctest_imports: Vec<String>,
}

impl SourceGraph {
    /// The native value-struct decorator has no corresponding Python object runtime export.
    pub(crate) fn imports_struct_decorator(&self) -> bool {
        self.modules.iter().any(|module| {
            module.bindings.values().any(|binding| {
                matches!(
                    binding,
                    Binding::Intrinsic(Intrinsic::Struct)
                        | Binding::IntrinsicModule(IntrinsicModule::Imre)
                )
            })
        })
    }

    pub(crate) fn boxed_pattern_method(
        &self,
        module: usize,
        expr: &ast::Expr,
    ) -> Option<&'static str> {
        let attr = expr.as_attribute_expr()?;
        let method = match attr.attr.as_str() {
            "split" => "re.Pattern.split",
            "subn" => "re.Pattern.subn",
            _ => return None,
        };
        self.session
            .is_regex_pattern(module, &attr.value)
            .then_some(method)
    }

    /// A pattern created in an imported helper can reach a boxed method in another module.
    pub(crate) fn uses_boxed_pattern_method(&self) -> bool {
        self.modules.iter().enumerate().any(|(id, module)| {
            !module.bundled
                && ast::helpers::any_over_body(&module.parsed.syntax().body, |expr| {
                    self.boxed_pattern_method(id, expr).is_some()
                })
        })
    }

    pub(crate) fn is_abstract_context_manager(&self, definition: DefinitionId) -> bool {
        let source = &self.modules[definition.module];
        source.is_bundled(CONTEXTLIB_ABSTRACT)
            && source.export("AbstractContextManager") == Some(Binding::Definition(definition))
    }

    /// Resolve a bundled logging definition called implicitly by native lowering.
    pub(crate) fn logging_definition(&self, name: &str) -> Option<DefinitionId> {
        let module = self
            .modules
            .iter()
            .find(|module| module.is_bundled(LOGGING_CLASSES))?;
        match module.bindings.get(name)? {
            Binding::Definition(def) => Some(*def),
            _ => None,
        }
    }

    pub(crate) fn checked_diagnostic_count(&self, module: ModuleId) -> usize {
        self.checked_diagnostics.borrow()[module].len()
    }

    pub(crate) fn is_unittest_main(&self, id: DefinitionId) -> bool {
        let owner = &self.modules[id.module];
        owner.bundled
            && owner.name == "unittest"
            && owner.bindings.get("main") == Some(&Binding::Definition(id))
    }

    /// Keep module indices stable for the semantic cache. Only after lowering
    /// expose the files a disk build must revalidate and protect from writes.
    pub(crate) fn disk_inputs(
        &mut self,
        profile: Option<&Path>,
    ) -> (crate::SourceInputs, Vec<PathBuf>) {
        let mut inputs = std::mem::take(&mut self.inputs);
        let mut modules = self.modules.iter().enumerate();
        inputs.files.retain(|_| {
            let (id, module) = modules.next().expect("one input per module");
            !module.bundled && !module.is_namespace() && !(id == self.entry && self.virtual_command)
        });
        let paths = inputs
            .files
            .iter()
            .map(|(path, _, _)| path.clone())
            .chain(profile.map(Path::to_path_buf))
            .collect();
        (inputs, paths)
    }

    /// Keep read-only JSON views in programs that never parse or modify a TOML document.
    fn uses_mutable_documents(&self) -> bool {
        use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
        struct Scan<'a> {
            graph: &'a SourceGraph,
            id: usize,
            found: bool,
        }
        impl<'ast> Visitor<'ast> for Scan<'_> {
            fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
                if let ast::Stmt::FunctionDef(method) = stmt
                    && !self.graph.includes_method(self.id, method)
                    && !self.graph.modules[self.id]
                        .bindings
                        .contains_key(method.name.as_str())
                {
                    return;
                }
                walk_stmt(self, stmt);
            }
            fn visit_expr(&mut self, e: &'ast ast::Expr) {
                if matches!(
                    self.graph.resolve(self.id, e),
                    Some(Binding::Intrinsic(
                        Intrinsic::TomliLoads | Intrinsic::TomliLoad | Intrinsic::TomliDump
                    ))
                ) {
                    self.found = true;
                } else {
                    walk_expr(self, e);
                }
            }
        }
        self.modules.iter().enumerate().any(|(id, m)| {
            let mut scan = Scan {
                graph: self,
                id,
                found: false,
            };
            for (statement, node) in m.parsed.syntax().body.iter().enumerate() {
                if self.includes(DefinitionId {
                    module: id,
                    statement,
                }) || !matches!(node, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_))
                {
                    scan.visit_stmt(node);
                }
            }
            scan.found
        })
    }
    /// A write affects bare len calls in every module, including earlier definitions.
    fn writes_builtin_len(&self) -> bool {
        use ruff_python_ast::visitor::{Visitor, walk_expr};
        struct Scan<'a> {
            graph: &'a SourceGraph,
            module: ModuleId,
            writes: bool,
        }
        impl<'a> Visitor<'a> for Scan<'_> {
            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                if let ast::Expr::Attribute(attr) = expr
                    && attr.ctx != ast::ExprContext::Load
                    && self.graph.resolve(self.module, expr)
                        == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen))
                {
                    self.writes = true;
                }
                if !self.writes {
                    walk_expr(self, expr);
                }
            }
        }
        if !self.modules.iter().any(|source| {
            source
                .bindings
                .values()
                .any(|binding| *binding == Binding::IntrinsicModule(IntrinsicModule::Builtins))
        }) {
            return false;
        }
        self.modules.iter().enumerate().any(|(module, source)| {
            let mut scan = Scan {
                graph: self,
                module,
                writes: false,
            };
            for stmt in &source.parsed.syntax().body {
                scan.visit_stmt(stmt);
            }
            scan.writes
        })
    }

    /// Ordinary file builds keep assertions at every native optimizer level.
    pub(crate) fn python_optimization(&self) -> i32 {
        self.command_startup
            .as_ref()
            .map_or(0, |(flags, _)| flags.optimize)
    }

    pub(crate) fn reachable_only(&self) -> bool {
        self.reachable.is_some()
    }

    fn required_definitions(&self, module: ModuleId) -> Option<&reachability::Reachable> {
        if let Some((library, roots)) = &self.required_library
            && *library == module
        {
            return Some(roots);
        }
        self.reachable.as_ref()
    }

    pub(crate) fn includes(&self, definition: DefinitionId) -> bool {
        self.required_definitions(definition.module)
            .is_none_or(|r| r.definitions.contains(&definition))
    }

    pub(crate) fn inherits_class_method(&self, class: DefinitionId, name: &str) -> bool {
        self.reachable
            .as_ref()
            .is_none_or(|r| r.inherited_class_method(class, name))
    }

    pub(crate) fn unused_literal_global(&self, definition: DefinitionId) -> bool {
        self.reachable
            .as_ref()
            .is_some_and(|r| r.unused_literals.contains(&definition))
    }

    pub(crate) fn includes_method(&self, module: ModuleId, method: &ast::StmtFunctionDef) -> bool {
        self.required_definitions(module)
            .is_none_or(|r| r.methods.contains(&(module, method.start())))
    }

    pub(crate) fn module_filename(&self, module: ModuleId) -> &Path {
        if module == self.entry {
            &self.entry_filename
        } else {
            &self.modules[module].path
        }
    }

    /// The import package of a linked source module; direct script entries have no package.
    pub(crate) fn module_package(&self, module: ModuleId) -> Option<&str> {
        let source = &self.modules[module];
        (module != self.entry).then(|| {
            if source.is_package {
                source.name.as_str()
            } else {
                source
                    .name
                    .rsplit_once('.')
                    .map_or("", |(parent, _)| parent)
            }
        })
    }

    /// Record only this call's overload diagnostic. Nested arguments still get
    /// their own diagnostics, including calls with the same method name.
    pub(crate) fn checked_overload(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::Call(call));
    }

    /// Native evaluation of this exact call raises for a known noncallable object.
    pub(crate) fn checked_noncallable(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::NonCallable(call));
    }

    /// A checked runtime overload can legally stand alone: calling the dummy raises.
    pub(crate) fn runtime_overload(&self, module: ModuleId, name: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::RuntimeOverload(name));
    }

    /// Mark the precise call whose behavior for missing arguments is handled by native lowering.
    pub(crate) fn checked_missing_argument(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::MissingArgument(call));
    }

    /// This exact operand is excessive for a resolved native method that raises TypeError.
    pub(crate) fn checked_excess_argument(&self, module: ModuleId, argument: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::ExcessArgument(argument));
    }

    /// Native lowering accepts an inert body at this exact return annotation.
    pub(crate) fn checked_inert_return(&self, module: ModuleId, annotation: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::InertReturn(annotation));
    }

    /// The native positional-only binder already raises at this exact keyword.
    pub(crate) fn checked_positional_keyword(&self, module: ModuleId, keyword: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::PositionalKeyword(keyword));
    }

    /// Lowering checked this exact operand against the native integer protocol.
    pub(crate) fn checked_argument_type(&self, module: ModuleId, argument: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::ArgumentType(argument));
    }

    pub(crate) fn deferred_name(&self, module: ModuleId, name: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::DeferredName(name));
    }

    /// The native binder raises because this keyword's parameter was already bound.
    pub(crate) fn checked_repeated_keyword(&self, module: ModuleId, keyword: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::RepeatedKeyword(keyword));
    }

    /// An optional handler lock either enters the captured lock or raises TypeError on None.
    pub(crate) fn checked_optional_lock_context(&self, module: ModuleId, expr: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::OptionalLockContext(expr));
    }

    /// This operand is evaluated by a tuple display that raises a native TypeError.
    pub(crate) fn checked_non_iterable_spread(&self, module: ModuleId, value: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::NonIterableSpread(value));
    }

    pub(crate) fn checked_keywords(
        &self,
        module: ModuleId,
        keywords: impl Iterator<Item = TextRange>,
    ) {
        self.checked_diagnostics.borrow_mut()[module]
            .extend(keywords.map(CheckedDiagnostic::Keyword));
    }

    /// Target headers decide whether an errno from-import succeeds or raises ImportError.
    pub(crate) fn checked_builtin_import(&self, module: ModuleId, alias: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::BuiltinImport(alias));
    }

    /// A sealed module exports this exact member even when typeshed omits it.
    pub(crate) fn checked_builtin_attribute(&self, module: ModuleId, name: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::BuiltinAttribute(name));
    }

    /// Record an integer access with a proved native indexing implementation.
    /// Other diagnostics at this index or inside the receiver remain visible.
    pub(crate) fn checked_index_read(&self, module: ModuleId, read: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::CheckedIndexRead(read));
    }

    /// The accelerated result of cmp_to_key remains callable after binding an object.
    pub(crate) fn checked_callable_key(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::CallableKey(call));
    }

    /// The native table is an actual dict, including errno.errorcode (a Mapping in typeshed).
    pub(crate) fn checked_dict_deletion(&self, module: ModuleId, target: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::DictDeletion(target));
    }

    /// This read checks a local's assignment flag and raises Python's error if unset.
    pub(crate) fn checked_unbound_local_read(&self, module: ModuleId, read: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::UnboundLocalRead(read));
    }

    /// Native construction raises TypeError at this abstract class call.
    pub(crate) fn checked_abstract_class_call(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::AbstractClassCall(call));
    }

    /// Source ranges where the reachable profile or native lowering handles diagnostics.
    pub(crate) fn diagnostic_scopes(
        &self,
    ) -> (Option<Vec<Vec<TextRange>>>, Vec<Vec<CheckedDiagnostic>>) {
        let overloads: Vec<Vec<_>> = self
            .modules
            .iter()
            .enumerate()
            .map(|(id, module)| {
                module
                    .parsed
                    .syntax()
                    .body
                    .iter()
                    .filter_map(|stmt| {
                        if let ast::Stmt::ClassDef(c) = stmt {
                            Some(&c.body)
                        } else {
                            None
                        }
                    })
                    .flatten()
                    .filter_map(|stmt| {
                        let ast::Stmt::FunctionDef(method) = stmt else {
                            return None;
                        };
                        let [decorator] = method.decorator_list.as_slice() else {
                            return None;
                        };
                        (self.resolve(id, &decorator.expression)
                            == Some(Binding::Intrinsic(Intrinsic::ContextManager)))
                        .then_some(CheckedDiagnostic::Decorator(decorator.range()))
                    })
                    .chain({
                        let mut diagnostics = self.checked_diagnostics.borrow()[id].clone();
                        diagnostics.sort_unstable_by_key(|note| note.sort_key());
                        diagnostics.dedup();
                        diagnostics
                    })
                    .collect()
            })
            .collect();
        let mut skipped = self.reachable.as_ref().map(|r| r.skipped.clone());
        if let Some((library, roots)) = &self.required_library {
            skipped.get_or_insert_with(|| vec![Vec::new(); self.modules.len()])[*library]
                .extend_from_slice(&roots.skipped[*library]);
        }
        // The bundled _simple_enum factories are lowered to immutable member tables.
        // Keep the actual category property bodies and every caller in ty's check.
        if let Some((id, http)) = self
            .modules
            .iter()
            .enumerate()
            .find(|(_, m)| m.is_bundled("http"))
        {
            let ranges =
                &mut skipped.get_or_insert_with(|| vec![Vec::new(); self.modules.len()])[id];
            for stmt in &http.parsed.syntax().body {
                match stmt {
                    ast::Stmt::ImportFrom(import) if import.module.as_deref() == Some("enum") => {
                        ranges.push(import.range());
                    }
                    ast::Stmt::ClassDef(class) => {
                        ranges.extend(class.body.iter().filter_map(|s| {
                            if let ast::Stmt::FunctionDef(f) = s
                                && f.name.as_str() == "__new__"
                            {
                                Some(f.range())
                            } else {
                                None
                            }
                        }))
                    }
                    _ => {}
                }
            }
        }
        if self.python_optimization() > 0 {
            use ast::visitor::{Visitor, walk_stmt};

            struct OmittedAssertions<'a>(&'a mut Vec<TextRange>);
            impl<'ast> Visitor<'ast> for OmittedAssertions<'_> {
                fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
                    if matches!(stmt, ast::Stmt::Assert(_)) {
                        self.0.push(stmt.range());
                    } else {
                        walk_stmt(self, stmt);
                    }
                }

                fn visit_expr(&mut self, _: &'ast ast::Expr) {}
            }
            let ranges = skipped.get_or_insert_with(|| vec![Vec::new(); self.modules.len()]);
            for (module, ranges) in self.modules.iter().zip(ranges) {
                OmittedAssertions(ranges).visit_body(&module.parsed.syntax().body);
            }
        }
        (skipped, overloads)
    }

    /// Entry segments preserve globals, platform import checks, and definition-time effects.
    pub(crate) fn initializers(
        &self,
        entry_declarations: bool,
    ) -> impl Iterator<Item = (ModuleId, Range<usize>)> + '_ {
        self.initialization
            .iter()
            .filter(move |(id, segment)| {
                let module = &self.modules[*id];
                *id != self.entry
                    || !module.snapshots.is_empty()
                    || entry_declarations
                    || !module.entry_globals.is_empty()
                    || !module.overloads.is_empty()
                    || (!segment.is_empty()
                        && errno_import(&module.parsed.syntax().body[segment.end - 1]).is_some())
            })
            .cloned()
    }

    /// An isolated source API supports sealed imports only; first-party imports
    /// require the path API and its filesystem search root.
    pub(crate) fn single(source: &str) -> Result<Self, String> {
        Self::single_with_mode(source, false)
    }

    pub(crate) fn single_with_mode(source: &str, reachable_only: bool) -> Result<Self, String> {
        let mut loader = Loader::new(PathBuf::from("/"), &[], FileSystem::default())?;
        loader.allow_imports = false;
        loader.reachable_only = reachable_only;
        let filename = PathBuf::from("/input.py");
        let entry = loader.add(
            filename.clone(),
            filename.clone(),
            "__main__".into(),
            false,
            source,
        )?;
        loader.scan(entry)?;
        loader.finish(entry, filename, reachable_only, None)
    }

    /// Parse command bytes as __main__; only a non-safe command sees the working directory.
    pub(crate) fn command(
        command: &crate::PythonCommand,
        options: &crate::CompileOptions,
        fs: FileSystem<'_>,
    ) -> Result<Self, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let mut loader = Loader::new(root.clone(), &options.extension_paths, fs)?;
        loader.skip_current_root = command.flags.safe_path;
        loader.reachable_only = options.reachable_only || options.entry_profile.is_some();
        let filename = root.join("<string>.py");
        let entry = loader.add_source(
            filename.clone(),
            filename.clone(),
            "__main__".into(),
            false,
            &command.source,
            crate::SourceKind::Command,
        )?;
        loader.scan(entry)?;
        let reachable_only = loader.reachable_only;
        let mut graph = loader.finish(
            entry,
            filename,
            reachable_only,
            options.entry_profile.as_deref(),
        )?;
        graph.command_startup = Some((command.flags.clone(), command.executable.clone()));
        graph.virtual_command = true;
        Ok(graph)
    }

    /// A directly invoked file is __main__; its enclosing package is not imported.
    pub(crate) fn script(
        path: &Path,
        flags: &crate::CommandFlags,
        executable: &str,
        options: &crate::CompileOptions,
        mut fs: FileSystem<'_>,
    ) -> Result<Self, String> {
        let filename = std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path);
        let entry = fs
            .canonicalize(path)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let bytes = if crate::named_bytecode(path) {
            std::fs::File::open(&entry).and_then(crate::read_bytecode_prefix)
        } else {
            fs.read(&entry)
        }
        .map_err(|err| format!("{}: {err}", path.display()))?;
        if let Some(error) = crate::bytecode_script_error(path, &bytes) {
            return Err(error.into());
        }
        let source = String::from_utf8(bytes)
            .map_err(|_| format!("{}: stream did not contain valid UTF-8", path.display()))?;
        let root = entry
            .parent()
            .ok_or("script has no parent directory")?
            .to_path_buf();
        let mut loader = Loader::new(root, &options.extension_paths, fs)?;
        loader.skip_current_root = flags.safe_path;
        let reachable = options.reachable_only || options.entry_profile.is_some();
        loader.reachable_only = reachable;
        let id = loader.add(entry.clone(), entry, "__main__".into(), false, &source)?;
        loader.scan(id)?;
        let mut graph = loader.finish(id, filename, reachable, options.entry_profile.as_deref())?;
        graph.command_startup = Some((flags.clone(), executable.to_owned()));
        Ok(graph)
    }

    #[cfg(test)]
    pub(crate) fn load(
        entry: &Path,
        module_root: Option<&Path>,
        extension_paths: &[PathBuf],
    ) -> Result<Self, String> {
        Self::load_with(
            entry,
            module_root,
            extension_paths,
            FileSystem::default(),
            false,
            None,
        )
    }

    pub(crate) fn load_with(
        entry: &Path,
        module_root: Option<&Path>,
        extension_paths: &[PathBuf],
        mut fs: FileSystem<'_>,
        reachable_only: bool,
        profile: Option<&Path>,
    ) -> Result<Self, String> {
        // __file__ preserves the invoked spelling, including symlinks and '..'.
        // Use the canonical path below for import resolution and containment.
        let filename = std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(entry);
        let entry = fs
            .canonicalize(entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        if entry.extension().and_then(|value| value.to_str()) != Some("py") {
            return Err("entry source must be a .py file".into());
        }
        if entry.file_name().and_then(|value| value.to_str()) == Some("__init__.py") {
            return Err("use a program module or package __main__.py as the entry source".into());
        }
        let inferred = module_root.is_none();
        let root = if let Some(root) = module_root {
            fs.canonicalize(root)
                .map_err(|error| format!("{}: {error}", root.display()))?
        } else {
            let mut root = entry
                .parent()
                .ok_or("entry source has no parent")?
                .to_path_buf();
            while fs.is_file(&root.join("__init__.py")) {
                root = root
                    .parent()
                    .ok_or("package has no module root")?
                    .to_path_buf();
            }
            root
        };
        if !fs.is_dir(&root) {
            return Err("module root must be a directory".into());
        }
        let relative = entry
            .strip_prefix(&root)
            .map_err(|_| "entry source must be inside the module root")?;
        let name = relative
            .with_extension("")
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .ok_or("module paths must be valid UTF-8")
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(".");
        let source = fs
            .read_to_string(&entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        let mut loader = Loader::new(root, extension_paths, fs)?;
        loader.reachable_only = reachable_only;
        if inferred {
            loader.inputs.missing.push(loader.root.join("__init__.py"));
        }
        let id = loader.add(entry.clone(), entry, name.clone(), false, &source)?;
        // Like `python -m package.program`, package initialization precedes the entry.
        // The entry itself is __main__, and is deliberately not attached as a package child.
        if let Some((package, _)) = name.rsplit_once('.') {
            let package = loader.load_module(package)?;
            if !loader.modules[package].is_package {
                return Err(
                    "entry source is inside a directory without an __init__.py package".into(),
                );
            }
        }
        loader.scan(id)?;
        loader.finish(id, filename, reachable_only, profile)
    }

    /// Resolves namespace paths while leaving local-value attributes to the expression checker.
    pub(crate) fn resolve(&self, module: ModuleId, expression: &ast::Expr) -> Option<Binding> {
        if let Some(import) = self.local_static_binding(module, expression) {
            return self.local_import_value(module, expression, import);
        }
        resolve_binding(&self.modules, module, expression)
    }

    /// Recognize Protocol and Protocol[T] at the point where the class is defined.
    pub(crate) fn is_protocol_base(&self, module: ModuleId, expression: &ast::Expr) -> bool {
        let origin = match expression {
            ast::Expr::Subscript(subscript) => &*subscript.value,
            base => base,
        };
        matches!(
            self.resolve(module, origin),
            Some(Binding::Intrinsic(Intrinsic::TYPING_PROTOCOL))
        ) && self.session.is_typing_protocol(module, origin)
    }

    /// An import binds its name throughout the function, even before the import executes.
    fn local_static_binding(
        &self,
        module: ModuleId,
        expression: &ast::Expr,
    ) -> Option<&LocalStaticImport> {
        let imports = &self.modules[module].local_static_imports;
        if imports.is_empty() {
            return None;
        }
        let mut root = expression;
        while let ast::Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        let name = root.as_name_expr()?.id.as_str();
        imports
            .iter()
            .find(|i| i.name == name && i.body.contains_range(expression.range()))
    }

    fn local_import_value(
        &self,
        module: ModuleId,
        expression: &ast::Expr,
        import: &LocalStaticImport,
    ) -> Option<Binding> {
        if expression.start() < import.initialized {
            return None;
        }
        if import.module_name == "importlib.metadata" {
            let attr = expression.as_attribute_expr()?;
            return self
                .session
                .is_stdlib_module(module, &attr.value, "importlib.metadata")
                .then(|| {
                    stdlib_symbol(
                        &self.modules,
                        IntrinsicModule::ImportlibMetadata,
                        attr.attr.as_str(),
                    )
                })?;
        }
        let (root, attribute) = match expression {
            ast::Expr::Attribute(attr) => (&*attr.value, Some(attr.attr.as_str())),
            ast::Expr::Name(_) => (expression, None),
            _ => return None,
        };
        if !self.session.has_import_binding(
            module,
            root,
            import.alias,
            &self.modules[module].parsed,
        ) {
            return None;
        }
        let owner = IntrinsicModule::named(import.module_name)?;
        match (&import.symbol, attribute) {
            (None, None) => Some(Binding::IntrinsicModule(owner)),
            (None, Some(attr)) => owner.symbol(attr),
            (Some(symbol), None) => owner.symbol(symbol),
            (Some(_), Some(_)) => None,
        }
    }

    /// The loader has already resolved this import and recorded the scope of its binding.
    pub(crate) fn local_static_import(&self, module: ModuleId, stmt: &ast::Stmt) -> bool {
        matches!(stmt, ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_))
            && self.modules[module]
                .local_static_imports
                .iter()
                .any(|i| i.initialized == stmt.end())
    }

    /// A postponed annotation may reference a type imported under TYPE_CHECKING.
    pub(crate) fn resolve_type(&self, module: ModuleId, expression: &ast::Expr) -> Option<Binding> {
        if let Some(import) = self.local_static_binding(module, expression) {
            return self.local_import_value(module, expression, import);
        }
        match expression {
            ast::Expr::Name(name) => self.modules[module]
                .type_bindings
                .get(name.id.as_str())
                .copied()
                .or_else(|| self.resolve(module, expression)),
            ast::Expr::Attribute(attribute) => match self.resolve_type(module, &attribute.value)? {
                Binding::Module(target) => self.modules[target].export(attribute.attr.as_str()),
                Binding::IntrinsicModule(target) => {
                    stdlib_attribute(&self.modules, module, target, attribute)
                }
                _ => None,
            },
            _ => None,
        }
    }
}

struct LocalStaticImport {
    name: String,
    module_name: &'static str,
    symbol: Option<String>,
    alias: TextRange,
    body: TextRange,
    initialized: TextSize,
}

/// An unconditional function-local import dominates the remaining statements in that body.
/// The semantic model checks the receiver at each use, including local rebindings.
fn static_import_scopes(f: &ast::StmtFunctionDef, imports: &mut Vec<LocalStaticImport>) {
    for stmt in &f.body {
        // These modules have no observable initialization or mutable exports.
        // Other local imports still take their ordinary lowering path.
        let (alias, module_name, symbol, default_name) = match stmt {
            ast::Stmt::Import(import) if !import.is_lazy => {
                let [alias] = import.names.as_slice() else {
                    continue;
                };
                match alias.name.as_str() {
                    "importlib.metadata" => (alias, "importlib.metadata", None, "importlib"),
                    "fnmatch" => (alias, "fnmatch", None, "fnmatch"),
                    "re" => (alias, "re", None, "re"),
                    _ => continue,
                }
            }
            ast::Stmt::ImportFrom(import) if !import.is_lazy && import.level == 0 => {
                let [alias] = import.names.as_slice() else {
                    continue;
                };
                let owner = match import.module.as_deref() {
                    Some("fnmatch") => "fnmatch",
                    Some("re") => "re",
                    _ => continue,
                };
                let name = alias.name.as_str();
                if !matches!(IntrinsicModule::named(owner).and_then(|m| m.symbol(name)), Some(Binding::Intrinsic(symbol)) if !matches!(symbol, Intrinsic::Declaration(_)))
                {
                    continue;
                }
                (alias, owner, Some(name.to_owned()), name)
            }
            _ => continue,
        };
        imports.push(LocalStaticImport {
            name: alias
                .asname
                .as_ref()
                .map_or(default_name, |n| n.as_str())
                .into(),
            module_name,
            symbol,
            alias: alias.range(),
            body: TextRange::new(f.body[0].start(), f.end()),
            initialized: stmt.end(),
        });
    }
}

fn importlib_child(name: &str) -> Option<IntrinsicModule> {
    match name {
        "abc" => Some(IntrinsicModule::ImportlibAbc),
        "_abc" => Some(IntrinsicModule::ImportlibLoaderAbc),
        "machinery" => Some(IntrinsicModule::ImportlibMachinery),
        "_bootstrap_external" => Some(IntrinsicModule::ImportlibBootstrapExternal),
        "metadata" => Some(IntrinsicModule::ImportlibMetadata),
        "readers" => Some(IntrinsicModule::ImportlibReaders),
        "resources" => Some(IntrinsicModule::ImportlibResources),
        "util" => Some(IntrinsicModule::ImportlibUtil),
        _ => None,
    }
}

fn importlib_child_name(module: IntrinsicModule) -> Option<&'static str> {
    match module {
        IntrinsicModule::ImportlibAbc => Some("abc"),
        IntrinsicModule::ImportlibLoaderAbc => Some("_abc"),
        IntrinsicModule::ImportlibMachinery => Some("machinery"),
        IntrinsicModule::ImportlibBootstrapExternal => Some("_bootstrap_external"),
        IntrinsicModule::ImportlibMetadata => Some("metadata"),
        IntrinsicModule::ImportlibReaders => Some("readers"),
        IntrinsicModule::ImportlibResources => Some("resources"),
        IntrinsicModule::ImportlibUtil => Some("util"),
        _ => None,
    }
}

fn stdlib_attribute(
    modules: &[SourceModule],
    owner: ModuleId,
    target: IntrinsicModule,
    attr: &ast::ExprAttribute,
) -> Option<Binding> {
    if target == IntrinsicModule::Importlib
        && modules[owner]
            .importlib_children
            .get(attr.attr.as_str())
            .is_some_and(|at| *at <= attr.start())
        && let Some(child) = importlib_child(attr.attr.as_str())
    {
        return Some(Binding::IntrinsicModule(child));
    }
    stdlib_symbol(modules, target, attr.attr.as_str())
}

fn resolve_binding(
    modules: &[SourceModule],
    module: ModuleId,
    expression: &ast::Expr,
) -> Option<Binding> {
    match expression {
        ast::Expr::Name(name) => modules[module].bindings.get(name.id.as_str()).copied(),
        ast::Expr::Attribute(attribute) => {
            match resolve_binding(modules, module, &attribute.value)? {
                Binding::Module(module) => modules[module].export(attribute.attr.as_str()),
                Binding::IntrinsicModule(target) => {
                    stdlib_attribute(modules, module, target, attribute)
                }
                Binding::Definition(_)
                | Binding::ConditionalValue
                | Binding::Value(_)
                | Binding::TypeAlias(..)
                | Binding::Intrinsic(_) => None,
            }
        }
        _ => None,
    }
}

pub(crate) const LOGGING_CLASSES: &str = "__imre_logging_classes__";
const LOGGING_HANDLER_CLASSES: &str = "__imre_logging_handler_classes__";
pub(crate) const CONTEXTLIB_ABSTRACT: &str = "__imre_contextlib_abstract__";
const CONTEXTLIB_EXIT_STACK: &str = "__imre_contextlib_exit_stack__";
const METADATA_VERSION: &str = "__imre_metadata_version__";
const INSPECT_FUNCTIONS: &str = "__imre_inspect_functions__";
const TEMPFILE_SPOOL: &str = "__imre_tempfile_spool__";
const TEXTWRAP_FUNCTIONS: &str = "__imre_textwrap_functions__";
const TEXTWRAP_WRAPPER: &str = "__imre_textwrap_wrapper__";

fn textwrap_implementation(name: &str) -> Option<&'static str> {
    match name {
        "indent" => Some(TEXTWRAP_FUNCTIONS),
        "TextWrapper" | "wrap" | "fill" | "shorten" => Some(TEXTWRAP_WRAPPER),
        _ => None,
    }
}

/// Resolve the public definitions compiled from Python alongside sealed native operations.
fn stdlib_symbol(modules: &[SourceModule], module: IntrinsicModule, name: &str) -> Option<Binding> {
    if module == IntrinsicModule::Dataclasses
        && matches!(name, "MISSING" | "_MISSING_TYPE")
        && let Some(source) = modules
            .iter()
            .find(|source| source.is_bundled("dataclasses"))
    {
        return source.export(name);
    }
    if module == IntrinsicModule::Selectors
        && matches!(
            name,
            "PollSelector" | "DefaultSelector" | "SelectorKey" | "EVENT_READ" | "EVENT_WRITE"
        )
        && let Some(source) = modules.iter().find(|source| source.is_bundled("selectors"))
    {
        return source.export(name);
    }
    module.symbol(name).or_else(|| {
        let implementation = match module {
            IntrinsicModule::Contextlib if name == "AbstractContextManager" => CONTEXTLIB_ABSTRACT,
            IntrinsicModule::Contextlib if name == "ExitStack" => CONTEXTLIB_EXIT_STACK,
            IntrinsicModule::ImportlibMetadata if name == "version" => METADATA_VERSION,
            IntrinsicModule::Inspect if name == "cleandoc" => INSPECT_FUNCTIONS,
            IntrinsicModule::Tempfile if name == "SpooledTemporaryFile" => TEMPFILE_SPOOL,
            IntrinsicModule::Logging if !name.starts_with('_') => LOGGING_CLASSES,
            IntrinsicModule::LoggingHandlers if !name.starts_with('_') => LOGGING_HANDLER_CLASSES,
            IntrinsicModule::Textwrap => textwrap_implementation(name)?,
            _ => return None,
        };
        let (id, source) = modules
            .iter()
            .enumerate()
            .find(|(_, m)| m.bundled && m.name == implementation)?;
        source.export(name).filter(|b| {
            matches!(b, Binding::Definition(d) if d.module == id &&
                matches!((module, &source.parsed.syntax().body[d.statement]),
                    (IntrinsicModule::Contextlib, ast::Stmt::ClassDef(_))
                    | (IntrinsicModule::ImportlibMetadata | IntrinsicModule::Inspect, ast::Stmt::FunctionDef(_))
                    | (IntrinsicModule::Tempfile, ast::Stmt::ClassDef(_))
                    | (IntrinsicModule::Logging, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_))
                    | (IntrinsicModule::LoggingHandlers, ast::Stmt::ClassDef(_))
                    | (IntrinsicModule::Textwrap, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))))
        })
    })
}

struct Loader<'a> {
    fs: FileSystem<'a>,
    /// ty only searches typeshed for builtins; record sealed imports missing from its inventory.
    checked_builtin_imports: Vec<(ModuleId, TextRange)>,
    needs_object_imports: bool,
    wildcard_import: Option<String>,
    inputs: crate::SourceInputs,
    root: PathBuf,
    extension_roots: Vec<PathBuf>,
    session: Session,
    modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    by_path: HashMap<PathBuf, ModuleId>,
    loading: HashSet<ModuleId>,
    /// A from-import of a still-initializing module observes the value at that point.
    partial_value_imports: HashSet<DefinitionId>,
    ordered_imports: bool,
    import_depth: usize,
    allow_imports: bool,
    skip_current_root: bool,
    reachable_only: bool,
    initialization: Vec<(ModuleId, Range<usize>)>,
    pub(crate) errno_names: Vec<String>,
}

impl<'a> Loader<'a> {
    fn new(
        root: PathBuf,
        extension_paths: &[PathBuf],
        mut fs: FileSystem<'a>,
    ) -> Result<Self, String> {
        let mut extension_roots = Vec::new();
        for path in extension_paths {
            let directory = fs
                .canonicalize(path)
                .map_err(|e| format!("invalid extension path {}: {e}", path.display()))?;
            if !fs.is_dir(&directory) {
                return Err(format!(
                    "extension path must be a directory: {}",
                    path.display()
                ));
            }
            if !extension_roots.contains(&directory) {
                extension_roots.push(directory);
            }
        }
        Ok(Self {
            fs,
            session: Session::new(&root, &extension_roots)?,
            extension_roots,
            inputs: crate::SourceInputs::default(),
            checked_builtin_imports: Vec::new(),
            needs_object_imports: false,
            wildcard_import: None,
            root,
            modules: Vec::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            loading: HashSet::new(),
            partial_value_imports: HashSet::new(),
            ordered_imports: false,
            import_depth: 0,
            allow_imports: true,
            skip_current_root: false,
            reachable_only: false,
            initialization: Vec::new(),
            errno_names: Vec::new(),
        })
    }

    fn add(
        &mut self,
        path: PathBuf,
        identity: PathBuf,
        name: String,
        is_package: bool,
        source: &str,
    ) -> Result<ModuleId, String> {
        self.add_source(
            path,
            identity,
            name,
            is_package,
            source,
            crate::SourceKind::File,
        )
    }

    fn add_source(
        &mut self,
        path: PathBuf,
        identity: PathBuf,
        name: String,
        is_package: bool,
        source: &str,
        kind: crate::SourceKind,
    ) -> Result<ModuleId, String> {
        self.inputs.files.push((
            path.clone(),
            identity.clone(),
            blake3::hash(source.as_bytes()).to_hex().to_string(),
        ));
        let source = crate::normalize_source(source, kind)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .into_owned();
        let parsed = self.session.add(&path, &source, kind)?;
        let id = self.modules.len();
        self.by_name.insert(name.clone(), id);
        self.by_path.insert(identity, id);
        self.loading.insert(id);
        self.modules.push(SourceModule {
            path,
            name,
            source,
            bundled: false,
            parsed,
            is_package,
            search_roots: SearchRoots::Single(0),
            archive: None,
            bindings: HashMap::new(),
            type_bindings: HashMap::new(),
            importlib_children: HashMap::new(),
            local_static_imports: Vec::new(),
            aliases: Vec::new(),
            overloads: HashSet::new(),
            type_guards: Vec::new(),
            entry_globals: Vec::new(),
            optional_imports: HashMap::new(),
            rebound_values: HashSet::new(),
            snapshots: HashMap::new(),
            executable: Vec::new(),
        });
        Ok(id)
    }

    fn finish(
        mut self,
        entry: ModuleId,
        entry_filename: PathBuf,
        reachable_only: bool,
        profile: Option<&Path>,
    ) -> Result<SourceGraph, String> {
        // Resolve against the completed graph. Type-only imports neither run a
        // module initializer nor introduce runtime bindings or import cycles.
        for id in 0..self.modules.len() {
            let parsed = self.modules[id].parsed.clone();
            for index in self.modules[id].type_guards.clone() {
                let ast::Stmt::If(guard) = &parsed.syntax().body[index] else {
                    continue; // ordinary runtime-visible type declarations have no guarded imports
                };
                for statement in &guard.body {
                    if let ast::Stmt::Import(import) = statement {
                        for alias in &import.names {
                            let full = alias.name.as_str();
                            let name = if alias.asname.is_some() {
                                full
                            } else {
                                full.split('.').next().unwrap()
                            };
                            let found = self
                                .by_name
                                .get(name)
                                .copied()
                                .map(Binding::Module)
                                .or_else(|| {
                                    IntrinsicModule::named(name).map(Binding::IntrinsicModule)
                                });
                            if let Some(binding) = found {
                                let local = alias.asname.as_ref().map_or(name, |id| id.as_str());
                                self.modules[id]
                                    .type_bindings
                                    .insert(local.to_owned(), binding);
                            }
                        }
                        continue;
                    }
                    let ast::Stmt::ImportFrom(import) = statement else {
                        continue;
                    };
                    let name = self.import_name(id, import)?;
                    for alias in &import.names {
                        let binding = self
                            .by_name
                            .get(&name)
                            .and_then(|&target| {
                                let target = &self.modules[target];
                                target
                                    .type_bindings
                                    .get(alias.name.as_str())
                                    .copied()
                                    .or_else(|| target.export(alias.name.as_str()))
                            })
                            .or_else(|| {
                                stdlib_symbol(
                                    &self.modules,
                                    IntrinsicModule::named(&name)?,
                                    alias.name.as_str(),
                                )
                            });
                        if let Some(binding) = binding {
                            let local = alias.asname.as_ref().unwrap_or(&alias.name);
                            self.modules[id]
                                .type_bindings
                                .insert(local.to_string(), binding);
                        }
                    }
                }
            }
        }
        let doctest_imports = self.doctest_program()?;
        self.inputs.missing.sort_unstable();
        self.inputs.missing.dedup();
        let mut checked_diagnostics = vec![Vec::new(); self.modules.len()];
        for (id, name) in self.checked_builtin_imports {
            checked_diagnostics[id].push(CheckedDiagnostic::BuiltinImport(name));
        }
        let mut graph = SourceGraph {
            doctest_imports,
            command_startup: None,
            virtual_command: false,
            traceback_objects: false,
            handled_tracebacks: self.modules.iter().any(|module| {
                module.bindings.values().any(|binding| {
                    matches!(
                        binding,
                        Binding::IntrinsicModule(IntrinsicModule::Traceback)
                            | Binding::Intrinsic(
                                Intrinsic::TracebackPrintExc | Intrinsic::TracebackFormatter(_)
                            )
                    )
                })
            }),
            empty_keywords: empty_keywords::EmptyKeywords::default(),
            checked_diagnostics: std::cell::RefCell::new(checked_diagnostics),
            needs_object_runtime: std::cell::Cell::new(self.needs_object_imports),
            class_body_error: Default::default(),
            object_builtin_errors: Default::default(),
            deferred_object_error: self.wildcard_import.into(),
            return_errors: Default::default(),
            entry_profile: profile
                .map(|p| entry_profile::Profile::load(p, &self.modules, &mut self.fs))
                .transpose()?
                .unwrap_or_default(),
            mutable_documents: false,
            mutable_builtin_len: false,
            reachable: None,
            required_library: None,
            entry_filename,
            inputs: self.inputs,
            session: self.session,
            ordered_bindings: self
                .ordered_imports
                .then(|| ordered_imports::Bindings::new(&self.modules)),
            modules: self.modules,
            entry,
            initialization: self.initialization,
            errno_names: self.errno_names,
        };
        if reachable_only {
            graph.reachable = Some(reachability::Reachable::collect(&graph, false));
        } else if let Some(library) = graph
            .modules
            .iter()
            .position(|m| m.is_bundled(CONTEXTLIB_EXIT_STACK))
        {
            // A sealed library can own higher-order methods that require object dispatch.
            // Strict builds still visit even an uncalled first-party function using them.
            graph.required_library =
                Some((library, reachability::Reachable::collect(&graph, true)));
        }
        graph.empty_keywords = empty_keywords::EmptyKeywords::collect(&graph);
        graph.mutable_documents = graph.uses_mutable_documents();
        if graph.handled_tracebacks {
            graph.traceback_objects = graph.reachable.as_ref().map_or_else(
                || reachability::Reachable::collect(&graph, false).traceback_objects,
                |roots| roots.traceback_objects,
            );
        }
        graph.mutable_builtin_len = graph.writes_builtin_len();
        Ok(graph)
    }

    fn scan(&mut self, id: ModuleId) -> Result<(), String> {
        let parsed = self.modules[id].parsed.clone();
        let mut imports = Vec::new();
        for stmt in &parsed.syntax().body {
            match stmt {
                ast::Stmt::FunctionDef(f) => static_import_scopes(f, &mut imports),
                ast::Stmt::ClassDef(c) => {
                    for f in c.body.iter().filter_map(ast::Stmt::as_function_def_stmt) {
                        static_import_scopes(f, &mut imports);
                    }
                }
                _ => {}
            }
        }
        if !imports.is_empty() {
            for module_name in imports
                .iter()
                .map(|import| import.module_name)
                .collect::<std::collections::BTreeSet<_>>()
            {
                self.intrinsic_module(module_name, None)?;
            }
            // Typeshed omits CPython's private _translate companion. The import
            // was resolved above against the sealed module's real export.
            self.checked_builtin_imports.extend(
                imports
                    .iter()
                    .filter(|i| {
                        i.module_name == "fnmatch" && i.symbol.as_deref() == Some("_translate")
                    })
                    .map(|i| (id, i.alias)),
            );
            self.modules[id].local_static_imports = imports;
        }
        let last_definition = parsed.syntax().body.iter().rposition(|statement| {
            matches!(
                statement,
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)
            )
        });
        let mut executable = false;
        let mut segment = 0;
        let mut definition_prefix = false;
        let mut field_separators = Vec::new();
        for (index, statement) in parsed.syntax().body.iter().enumerate() {
            if self.modules[id].archive.is_some() && !native_declaration(statement) {
                return Err(self.modules[id].error(statement,
                    "native stubs allow type-only imports and fully annotated, required positional-only methods with an ellipsis body"));
            }
            match statement {
                ast::Stmt::If(guard)
                    if resolve_binding(&self.modules, id, &guard.test)
                        == Some(Binding::Intrinsic(Intrinsic::TypeChecking))
                        && guard.elif_else_clauses.is_empty()
                        && guard.body.iter().all(type_aliases::type_statement) =>
                {
                    self.modules[id].type_guards.push(index);
                    for stmt in &guard.body {
                        if let (Some(name), Some(value)) = (value_name(stmt), module_value(stmt)) {
                            let known = self.add_alias(id, value, false);
                            self.modules[id]
                                .type_bindings
                                .insert(name.to_owned(), known);
                        }
                    }
                }
                ast::Stmt::Try(guard)
                    if id == 0
                        && !executable
                        && self.modules[id].entry_globals.is_empty()
                        && let Some((module, name)) =
                            optional_stdlib_import(&self.modules, id, guard) =>
                {
                    self.reject_stdlib_shadow(module)?;
                    self.bind(id, name, Binding::ConditionalValue, statement)?;
                    self.modules[id]
                        .optional_imports
                        .insert(statement.range(), name.to_owned());
                    self.modules[id].executable.push(index);
                }
                _ if self.alias_declaration(id, statement)? => {
                    self.modules[id].type_guards.push(index);
                }
                ast::Stmt::FunctionDef(function) => {
                    definition_prefix |= self.partial_source(id);
                    if executable && id == 0 {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    let name = function.name.as_str();
                    let current = Binding::Definition(DefinitionId {
                        module: id,
                        statement: index,
                    });
                    let decorated = matches!(function.decorator_list.as_slice(), [d]
                        if resolve_binding(&self.modules, id, &d.expression) == Some(Binding::Intrinsic(Intrinsic::Overload)));
                    let continues = matches!(self.modules[id].bindings.get(name), Some(Binding::Definition(previous))
                        if previous.module == id && previous.statement + 1 == index && self.modules[id].overloads.contains(&previous.statement));
                    if continues {
                        // Nothing can observe the intermediate name between these
                        // consecutive definitions. Intervening statements still go
                        // through the ordinary static rebind rejection.
                        self.modules[id].bindings.insert(name.into(), current);
                    } else {
                        self.bind(id, name, current, statement)?;
                    }
                    if decorated {
                        self.modules[id].overloads.insert(index);
                    }
                }
                ast::Stmt::ClassDef(class) => {
                    definition_prefix |= self.partial_source(id)
                        || class
                            .body
                            .iter()
                            .any(|stmt| matches!(stmt, ast::Stmt::Assign(_)));
                    if executable && id == 0 {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    self.bind(
                        id,
                        class.name.as_str(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => {
                    if executable || !self.modules[id].entry_globals.is_empty() {
                        // Emit accepted prefixes before the dependency starts:
                        // a literal can fail to allocate, and a guard can raise.
                        let module = &self.modules[id];
                        let prefix = module
                            .executable
                            .iter()
                            .rev()
                            .take_while(|&&at| at >= segment)
                            .map(|&at| &parsed.syntax().body[at]);
                        let simple = prefix.clone().all(|stmt| {
                            module_name_guard(stmt)
                                || conditional_import(&self.modules, id, stmt).is_some()
                                || (module.is_package
                                    && value_name(stmt).is_some()
                                    && module_value(stmt).is_some_and(literal_export))
                        });
                        let nested_source = module.name.contains('.') && !module.bundled;
                        let ordered = !module.is_package
                            && module.archive.is_none()
                            && prefix.clone().all(|stmt| {
                                module_name_guard(stmt)
                                    || value_name(stmt).is_some()
                                        && module_value(stmt).is_some_and(|value| {
                                            nested_source
                                                || literal_export(value)
                                                    && module.rebound_target(id, stmt).is_none()
                                        })
                                    || nested_source && matches!(stmt, ast::Stmt::Expr(_))
                            });
                        if id == 0 || !simple && !ordered {
                            return Err(self.modules[id].error(
                                statement,
                                "imports must precede executable module statements",
                            ));
                        }
                        self.ordered_imports |= !simple;
                        self.initialization.push((id, segment..index));
                        segment = index;
                        executable = false;
                    } else if definition_prefix {
                        // A child can call a definition that already exists in this module.
                        self.initialization.push((id, segment..index));
                        segment = index;
                    }
                    definition_prefix = false;
                    if self.modules[id]
                        .snapshots
                        .keys()
                        .any(|at| (segment..index).contains(at))
                    {
                        self.initialization.push((id, segment..index));
                        segment = index;
                    }
                    self.import(id, index, statement)
                        .map_err(|error| self.modules[id].error(statement, &error))?;
                    if errno_import(statement).is_some() {
                        // The target decides which forward symbols exist. A failed from-import
                        // must precede the next imported module, even if its alias is unused.
                        self.initialization.push((id, segment..index + 1));
                        segment = index + 1;
                    }
                }
                ast::Stmt::Expr(value)
                    if matches!(&*value.value, ast::Expr::StringLiteral(_)) && !executable => {}
                ast::Stmt::Assign(_)
                    if value_name(statement).is_some()
                        && matches!(module_value(statement), Some(ast::Expr::Call(call))
                        if resolve_binding(&self.modules, id, &call.func) == Some(Binding::Intrinsic(Intrinsic::NamedTuple))) =>
                {
                    if let Some(ast::Expr::Call(call)) = module_value(statement)
                        && let [_, fields] = call.arguments.args.as_ref()
                    {
                        namedtuple_fields::separators(
                            fields,
                            &self.modules[id].source,
                            &mut field_separators,
                        );
                    }
                    if executable && id == 0 {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    self.bind(
                        id,
                        value_name(statement).unwrap(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                _ => {
                    if let Some(name) = conditional_import(&self.modules, id, statement) {
                        self.bind(id, name, Binding::ConditionalValue, statement)?;
                        self.needs_object_imports = true;
                    }
                    if let ast::Stmt::If(guard) = statement
                        && resolve_binding(&self.modules, id, &guard.test)
                            == Some(Binding::Intrinsic(Intrinsic::TypeChecking))
                        && let [otherwise] = guard.elif_else_clauses.as_slice()
                        && otherwise.test.is_none()
                    {
                        for runtime_statement in &otherwise.body {
                            if let Some(name) = value_name(runtime_statement)
                                && module_value(runtime_statement).is_some()
                            {
                                self.bind(id, name, Binding::ConditionalValue, runtime_statement)?;
                            }
                        }
                    }
                    if let ast::Stmt::If(guard) = statement
                        && guard
                            .elif_else_clauses
                            .last()
                            .is_some_and(|clause| clause.test.is_none())
                    {
                        for statement in &guard.body {
                            let Some(name) = conditional_export_name(statement) else {
                                continue;
                            };
                            // Assignments to existing globals retain their storage and write tracking.
                            if module_value(statement).is_some()
                                && self.modules[id].bindings.contains_key(name)
                            {
                                continue;
                            }
                            if guard.elif_else_clauses.iter().all(|clause| {
                                clause
                                    .body
                                    .iter()
                                    .any(|other| conditional_export_name(other) == Some(name))
                            }) {
                                self.bind(id, name, Binding::ConditionalValue, statement)?;
                                self.needs_object_imports = true;
                            }
                        }
                    }
                    let entry_global = id == 0
                        && !executable
                        && (last_definition.is_some_and(|last| index < last)
                            || matches!(
                                module_value(statement),
                                Some(ast::Expr::Call(call)) if resolve_binding(&self.modules, id, &call.func) == Some(Binding::Intrinsic(Intrinsic::RegexCompile))
                            ));
                    if entry_global && module_attribute_guard(&self.modules, id, statement) {
                        // Run the guard in the initializer before subsequent declarations.
                        self.modules[id].entry_globals.push(index);
                        continue;
                    }
                    self.scan_value_writes(id, statement)?;
                    if let Some(name) = value_name(statement) {
                        if (id != 0 || entry_global)
                            && self.modules[id].rebound_target(id, statement).is_none()
                        {
                            self.bind(
                                id,
                                name,
                                Binding::Value(DefinitionId {
                                    module: id,
                                    statement: index,
                                }),
                                statement,
                            )?;
                        }
                        if entry_global {
                            self.modules[id].entry_globals.push(index);
                            continue;
                        }
                    }
                    executable = true;
                    self.modules[id].executable.push(index);
                }
            }
        }
        if !field_separators.is_empty() {
            let module = &mut self.modules[id];
            let mut source = module.source.clone();
            for range in field_separators {
                let spaces = " ".repeat(range.len());
                source.replace_range(range, &spaces);
            }
            module.parsed = self.session.replace_source(id, source);
        }
        self.loading.remove(&id);
        self.initialization
            .push((id, segment..parsed.syntax().body.len()));
        Ok(())
    }

    /// Record writes to established values, including those inside module-level control flow.
    fn scan_value_writes(&mut self, id: ModuleId, statement: &ast::Stmt) -> Result<(), String> {
        use ast::statement_visitor::{StatementVisitor, walk_stmt};

        #[derive(Default)]
        struct Writes<'a>(Vec<&'a ast::Stmt>);
        impl<'a> StatementVisitor<'a> for Writes<'a> {
            fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
                if value_name(stmt).is_some() {
                    self.0.push(stmt);
                } else if !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                    walk_stmt(self, stmt);
                }
            }
        }
        let mut writes = Writes::default();
        writes.visit_stmt(statement);
        let module = &mut self.modules[id];
        for stmt in writes.0 {
            if let Some(first) = module.local_value(id, stmt) {
                if self.partial_value_imports.contains(&first) {
                    return Err(module.error(stmt,
                        "rebinding a value already imported during cyclic initialization requires a runtime namespace"));
                }
                module.rebound_values.insert(first.statement);
            }
        }
        Ok(())
    }

    /// Preserve the original spelling of target-dependent symbols even when imported under an alias.
    fn named_import(&mut self, binding: Binding, name: &str) -> Binding {
        if binding != Binding::Intrinsic(Intrinsic::ErrnoAttribute) {
            return binding;
        }
        let slot = self
            .errno_names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| {
                self.errno_names.push(name.to_owned());
                self.errno_names.len() - 1
            });
        Binding::Intrinsic(Intrinsic::ErrnoFrom(slot))
    }

    fn bind(
        &mut self,
        module: ModuleId,
        name: &str,
        binding: Binding,
        node: &impl Ranged,
    ) -> Result<(), String> {
        if self.modules[module].bindings.get(name) == Some(&Binding::ConditionalValue) {
            self.modules[module].bindings.insert(name.into(), binding);
            return Ok(());
        }
        if let Some(previous) = self.modules[module].bindings.get(name) {
            if *previous != binding {
                return Err(self.modules[module]
                    .error(node, &format!("static binding {name:?} cannot be rebound")));
            }
        } else {
            self.modules[module].bindings.insert(name.into(), binding);
        }
        Ok(())
    }

    fn import(&mut self, id: ModuleId, index: usize, statement: &ast::Stmt) -> Result<(), String> {
        match statement {
            ast::Stmt::Import(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                for alias in &import.names {
                    let name = alias.name.as_str();
                    if let Some(module) = self.intrinsic_module(name, None)? {
                        if let Some(child) = importlib_child_name(module) {
                            self.modules[id]
                                .importlib_children
                                .entry(child)
                                .or_insert(statement.end());
                        }
                        if alias.asname.is_none()
                            && let Some((local, _)) = name.split_once('.')
                            && let Some(namespace) = IntrinsicModule::named(local)
                        {
                            self.bind(id, local, Binding::IntrinsicModule(namespace), alias)?;
                            continue;
                        }
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), Binding::IntrinsicModule(module), alias)?;
                        if module.lacks_builtin_stub() {
                            self.checked_builtin_imports.push((id, alias.range()));
                        }
                        continue;
                    }
                    let leaf = self.load_module(name)?;
                    let (name, target) = if let Some(alias) = &alias.asname {
                        (alias.as_str(), leaf)
                    } else {
                        let first = name.split('.').next().unwrap();
                        (first, self.by_name[first])
                    };
                    self.bind(id, name, Binding::Module(target), alias)?;
                }
            }
            ast::Stmt::ImportFrom(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                let name = self.import_name(id, import)?;
                if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                    // The module initializer chooses its exports. Still resolve the
                    // dependency so checked files retain their ordinary diagnostics.
                    if import.level != 0 || self.intrinsic_module(&name, None)?.is_none() {
                        self.load_module(&name)?;
                    }
                    self.needs_object_imports = true;
                    self.wildcard_import.get_or_insert_with(|| {
                        self.modules[id].error(import, "wildcard imports require runtime binding")
                    });
                    return Ok(());
                }
                if import.level == 0 && name == "imre._logging" {
                    if !self.modules[id].is_bundled(LOGGING_CLASSES) {
                        return Err("the private logging implementation is not importable".into());
                    }
                    for alias in &import.names {
                        let method = match alias.name.as_str() {
                            "_rename_handler" => Intrinsic::HandlerRename,
                            "_Lock" => Intrinsic::HandlerLock,
                            "_lookup_handler" => Intrinsic::HandlerLookup,
                            "_file_encoding_uses_utf8" => Intrinsic::FileEncodingUsesUtf8,
                            "_print_active_exception" => Intrinsic::PrintActiveException,
                            "_Level" => Intrinsic::HandlerLevelType,
                            "_TextOutput" => Intrinsic::TextOutputType,
                            "_fallback" => Intrinsic::HandlerFallback,
                            _ => {
                                return Err(format!(
                                    "unknown private logging symbol: {}",
                                    alias.name
                                ));
                            }
                        };
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), Binding::Intrinsic(method), alias)?;
                    }
                    return Ok(());
                }
                if import.level == 0
                    && let Some(module) = self.intrinsic_module(&name, Some(&import.names))?
                {
                    if let Some(child) = importlib_child_name(module) {
                        self.modules[id]
                            .importlib_children
                            .entry(child)
                            .or_insert(statement.end());
                    }
                    for alias in &import.names {
                        if module == IntrinsicModule::Logging && alias.name.as_str() == "handlers" {
                            self.intrinsic_module("logging.handlers", None)?;
                        }
                        // A from-import loads the child before reading the package attribute.
                        let binding = if module == IntrinsicModule::Importlib
                            && let Some(child) = importlib_child(alias.name.as_str())
                        {
                            self.intrinsic_module(&format!("importlib.{}", alias.name), None)?;
                            self.modules[id].importlib_children.entry(importlib_child_name(child).unwrap()).or_insert(statement.end());
                            Some(Binding::IntrinsicModule(child))
                        } else if module == IntrinsicModule::Sys && alias.name.as_str() == "pycache_prefix" {
                            return Err("from-importing mutable sys.pycache_prefix is unsupported; use qualified access".into());
                        } else {
                            stdlib_symbol(&self.modules, module, alias.name.as_str())
                        }
                        .filter(|binding| {
                            self.reachable_only
                                || !matches!(
                                    binding,
                                    Binding::Intrinsic(Intrinsic::Declaration(_))
                                        | Binding::IntrinsicModule(IntrinsicModule::Declaration(_))
                                )
                        })
                        .ok_or_else(|| {
                            format!(
                                "unsupported sealed standard-library export {name}.{}",
                                alias.name
                            )
                        })?;
                        if binding
                            == Binding::Intrinsic(Intrinsic::Fnmatch(FnmatchOperation::Parts))
                            || module == IntrinsicModule::EmailHeaderValueParser
                        {
                            self.checked_builtin_imports.push((id, alias.range()));
                        }
                        if binding == Binding::Intrinsic(Intrinsic::BuiltinsLen) {
                            return Err("from builtins import len needs an import-time snapshot; use a qualified lookup".into());
                        }
                        let binding = if let Binding::Intrinsic(
                            symbol @ (Intrinsic::BuiltinsLast | Intrinsic::Stdout),
                        ) = binding
                        {
                            // Typeshed omits the interactive name; this import
                            // performs the real lookup and may raise ImportError.
                            if symbol == Intrinsic::BuiltinsLast {
                                self.checked_builtin_imports.push((id, alias.range()));
                            }
                            self.modules[id].snapshots.insert(index, symbol);
                            Binding::Value(DefinitionId {
                                module: id,
                                statement: index,
                            })
                        } else {
                            binding
                        };
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        let binding = self.named_import(binding, alias.name.as_str());
                        // A Sequence annotation keeps its typed list contract; value uses request boxing.
                        self.needs_object_imports |= !matches!(
                            module,
                            IntrinsicModule::EmailHeaderValueParser
                                | IntrinsicModule::Marshal
                                | IntrinsicModule::Pickle
                                | IntrinsicModule::Struct
                                | IntrinsicModule::Warnings
                                | IntrinsicModule::ImportlibReaders
                                | IntrinsicModule::ImportlibResourceReaders
                                | IntrinsicModule::ImportlibResources
                        ) && matches!(
                            binding,
                            Binding::Intrinsic(
                                export @ (Intrinsic::ObjectRuntimeExport(_) | Intrinsic::Awaitable)
                            ) if export != Intrinsic::TYPING_SEQUENCE
                        );
                        if module == IntrinsicModule::Collections
                            && binding == Binding::Intrinsic(Intrinsic::CountElements)
                        {
                            self.checked_builtin_imports.push((id, alias.range()));
                        }
                        self.bind(id, local.as_str(), binding, alias)?;
                    }
                    if module.lacks_builtin_stub()
                        && let Some(name) = &import.module
                    {
                        self.checked_builtin_imports.push((id, name.range()));
                    }
                    return Ok(());
                }
                // Only a from-import may read an explicitly ready binding from a loading module.
                let target = match self.by_name.get(&name).copied() {
                    Some(target)
                        if self.loading.contains(&target)
                            && (self.modules[target].is_package || self.partial_source(target)) =>
                    {
                        target
                    }
                    _ => self.load_module(&name)?,
                };
                for alias in &import.names {
                    let binding = if let Some(binding) =
                        self.modules[target].export(alias.name.as_str())
                    {
                        if self.loading.contains(&target) {
                            let package = self.modules[target].is_package;
                            let available = match binding {
                                Binding::Definition(def) => {
                                    let statement = &self.modules[def.module].parsed.syntax().body
                                        [def.statement];
                                    let ready = def.module == target
                                        && self.partial_source(target)
                                        && self.initialized(def)
                                        && matches!(statement, ast::Stmt::FunctionDef(f) if f.decorator_list.is_empty());
                                    self.ordered_imports |= ready;
                                    ready
                                }
                                Binding::Value(def) => package && self.initialized(def),
                                Binding::ConditionalValue => false,
                                _ => package,
                            };
                            if !available {
                                return Err(format!("cyclic import through {name:?}"));
                            }
                        }
                        if let Binding::Module(child) = binding
                            && self.loading.contains(&child)
                        {
                            return Err(format!(
                                "cyclic import through {:?}",
                                self.modules[child].name
                            ));
                        }
                        binding
                    } else if self.modules[target].is_package {
                        let child_name = format!("{name}.{}", alias.name);
                        if let Some(&child) = self.by_name.get(&child_name)
                            && self.loading.contains(&child)
                            && self.partial_source(child)
                            && !self.modules[target].bindings.contains_key("__getattr__")
                        {
                            // A fromlist can find the loading child before its parent publishes it.
                            self.ordered_imports = true;
                            Binding::Module(child)
                        } else {
                            Binding::Module(self.load_module(&child_name)?)
                        }
                    } else if self.modules[target].is_bundled("http.client")
                        || self.modules[target].is_bundled("dis")
                    {
                        // http.client supplies names beyond its checked header types.
                        // dis imports opcode's public names with a wildcard.
                        Binding::ConditionalValue
                    } else {
                        return Err(format!(
                            "module {name:?} has no static binding {:?}",
                            alias.name.as_str()
                        ));
                    };
                    if let Binding::Value(def) = binding
                        && self.loading.contains(&def.module)
                    {
                        self.partial_value_imports.insert(def);
                    }
                    if binding == Binding::Intrinsic(Intrinsic::SOCKETSERVER_CLASS) {
                        self.checked_builtin_imports.push((id, alias.range()));
                    }
                    self.needs_object_imports |= binding == Binding::ConditionalValue;
                    let local = alias.asname.as_ref().unwrap_or(&alias.name);
                    self.bind(id, local.as_str(), binding, alias)?;
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Intrinsic imports use fixed identities, but never silently accept a
    /// first-party file that attempts to shadow the standard-library namespace.
    fn intrinsic_module(
        &mut self,
        name: &str,
        imports: Option<&[ast::Alias]>,
    ) -> Result<Option<IntrinsicModule>, String> {
        let module = IntrinsicModule::named(name).filter(|module| {
            self.reachable_only || !matches!(module, IntrinsicModule::Declaration(_))
        });
        if let Some(module) = module {
            self.reject_stdlib_shadow(name)?;
            self.needs_object_imports |= matches!(
                module,
                IntrinsicModule::Ast
                    | IntrinsicModule::Blake2
                    | IntrinsicModule::Sha3
                    | IntrinsicModule::Md5
                    | IntrinsicModule::Atexit
                    | IntrinsicModule::SocketNative
                    | IntrinsicModule::Imp
                    | IntrinsicModule::ImportlibAbc
                    | IntrinsicModule::ImportlibLoaderAbc
                    | IntrinsicModule::ImportlibBootstrapExternal
                    | IntrinsicModule::ImportlibResourcesAbc
            );
        }
        if module == Some(IntrinsicModule::Dataclasses)
            && imports.is_none_or(|names| {
                names
                    .iter()
                    .any(|a| matches!(a.name.as_str(), "MISSING" | "_MISSING_TYPE"))
            })
        {
            self.load_module(name)?;
        }
        if module == Some(IntrinsicModule::Selectors)
            && cfg!(any(target_os = "linux", target_os = "macos"))
        {
            self.load_module(name)?;
        }
        let needs_textwrap = |implementation| {
            matches!(module, Some(IntrinsicModule::Textwrap))
                && imports.is_none_or(|names| {
                    names.iter().any(|alias| {
                        textwrap_implementation(alias.name.as_str()) == Some(implementation)
                    })
                })
        };
        let metadata = if module == Some(IntrinsicModule::ImportlibMetadata)
            && !self.by_name.contains_key(METADATA_VERSION)
            && imports.is_none_or(|names| names.iter().any(|a| a.name.as_str() == "version"))
        {
            let source = concat!(
                include_str!("../stdlib/importlib/metadata_typed.py"),
                "\n",
                include_str!("../stdlib/importlib/metadata_version.py"),
            );
            let paths = self.filesystem_search_path()?;
            Some(format!("{source}\n_search_path: list[str] = {paths}\n"))
        } else {
            None
        };
        // A named import loads only the bundled definitions it can call.
        let bundles = [
            metadata.as_deref().map(|source| (METADATA_VERSION, source)),
            (module == Some(IntrinsicModule::Contextlib)
                && imports.is_none_or(|names| {
                    names
                        .iter()
                        .any(|a| a.name.as_str() == "AbstractContextManager")
                }))
            .then_some((CONTEXTLIB_ABSTRACT, include_str!("../stdlib/contextlib.py"))),
            (module == Some(IntrinsicModule::Contextlib)
                && imports
                    .is_none_or(|names| names.iter().any(|a| a.name.as_str() == "ExitStack")))
            .then_some((
                CONTEXTLIB_EXIT_STACK,
                concat!(
                    "from contextlib import AbstractContextManager\n",
                    include_str!("../stdlib/contextlib_exitstack.py")
                ),
            )),
            (module == Some(IntrinsicModule::Tempfile)
                && imports.is_none_or(|names| {
                    names
                        .iter()
                        .any(|a| a.name.as_str() == "SpooledTemporaryFile")
                }))
            .then_some((TEMPFILE_SPOOL, include_str!("../stdlib/tempfile/spool.py"))),
            (module == Some(IntrinsicModule::Inspect)
                && imports.is_none_or(|names| names.iter().any(|a| a.name.as_str() == "cleandoc")))
            .then_some((INSPECT_FUNCTIONS, include_str!("../stdlib/inspect.py"))),
            (name.split('.').next() == Some("logging") && module.is_some()).then_some((
                LOGGING_CLASSES,
                concat!(
                    include_str!("../stdlib/logging/typed_handler.py"),
                    "\n",
                    include_str!("../stdlib/logging/stream.py.inc"),
                    "\n",
                    include_str!("../stdlib/logging/classes.py")
                ),
            )),
            (module == Some(IntrinsicModule::LoggingHandlers)).then_some((
                LOGGING_HANDLER_CLASSES,
                concat!(
                    include_str!("../stdlib/logging/handlers.py"),
                    "\n",
                    include_str!("../stdlib/logging/buffering.py"),
                ),
            )),
            needs_textwrap(TEXTWRAP_FUNCTIONS).then_some((
                TEXTWRAP_FUNCTIONS,
                include_str!("../stdlib/textwrap/functions.py"),
            )),
            needs_textwrap(TEXTWRAP_WRAPPER).then_some((
                TEXTWRAP_WRAPPER,
                include_str!("../stdlib/textwrap/wrapper.py"),
            )),
        ];
        for (implementation, source) in bundles.into_iter().flatten() {
            if self.by_name.contains_key(implementation) {
                continue;
            }
            let path = self.root.join(format!("{implementation}.py"));
            if self.fs.exists(&path) {
                return Err(format!(
                    "standard-library shadowing is unsupported: {implementation:?}"
                ));
            }
            self.inputs.missing.push(path.clone());
            let id = self.add(path.clone(), path, implementation.into(), false, source)?;
            self.modules[id].bundled = true;
            self.scan(id)?;
        }
        Ok(module)
    }

    fn reject_stdlib_shadow(&mut self, name: &str) -> Result<(), String> {
        if self.allow_imports && !self.skip_current_root {
            for base in [
                self.root.join(name.split('.').next().unwrap()),
                self.root.join(name.replace('.', "/")),
            ] {
                for candidate in [base.with_extension("py"), base.join("__init__.py")] {
                    if self.fs.exists(&candidate) {
                        return Err(format!(
                            "standard-library shadowing is unsupported: {name:?}"
                        ));
                    }
                    self.inputs.missing.push(candidate);
                }
            }
        }
        Ok(())
    }

    fn import_name(&self, id: ModuleId, import: &ast::StmtImportFrom) -> Result<String, String> {
        let suffix = import.module.as_ref().map_or("", |name| name.as_str());
        if import.level == 0 {
            return Ok(suffix.to_owned());
        }
        let module = &self.modules[id];
        let package = if module.is_package {
            module.name.as_str()
        } else {
            module
                .name
                .rsplit_once('.')
                .map_or("", |(parent, _)| parent)
        };
        let mut parts: Vec<_> = package.split('.').filter(|part| !part.is_empty()).collect();
        if import.level as usize > parts.len() {
            return Err("relative import goes beyond the first-party package root".into());
        }
        parts.truncate(parts.len() + 1 - import.level as usize);
        if !suffix.is_empty() {
            parts.push(suffix);
        }
        Ok(parts.join("."))
    }

    fn partial_source(&self, id: ModuleId) -> bool {
        let module = &self.modules[id];
        id != 0 && !module.bundled && module.archive.is_none()
    }

    fn initialized(&self, def: DefinitionId) -> bool {
        self.initialization.iter().any(|(module, statements)| {
            *module == def.module && statements.contains(&def.statement)
        })
    }

    /// Embed the same ordered roots that resolve imports, including Python's safe-path exclusion.
    fn filesystem_search_path(&self) -> Result<String, String> {
        let roots = std::iter::once(&self.root)
            .skip(usize::from(self.skip_current_root))
            .chain(&self.extension_roots)
            .collect::<Vec<_>>();
        serde_json::to_string(&roots).map_err(|e| e.to_string())
    }

    fn load_module(&mut self, name: &str) -> Result<ModuleId, String> {
        if name == METADATA_VERSION {
            return Err(
                "the private importlib.metadata implementation is not an importable module".into(),
            );
        }
        if name == TEMPFILE_SPOOL {
            return Err("the private tempfile implementation is not an importable module".into());
        }
        if name == INSPECT_FUNCTIONS {
            return Err("the private inspect implementation is not an importable module".into());
        }
        if matches!(name, LOGGING_CLASSES | LOGGING_HANDLER_CLASSES) {
            return Err("the private logging implementation is not an importable module".into());
        }
        if matches!(name, TEXTWRAP_FUNCTIONS | TEXTWRAP_WRAPPER) {
            return Err("the private textwrap implementation is not an importable module".into());
        }
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!(
                    "cyclic import through {name:?} (the entry module cannot import itself)"
                ));
            }
            return Ok(id);
        }
        let bundled = match name {
            "stat" => stat_mode_source().map(|source| ("stat.py", source)),
            // Keep the public typeshed decorator signatures available alongside this singleton.
            "dataclasses" => Some((
                "<imre stdlib>/dataclasses.py",
                include_str!("../stdlib/dataclasses_missing.py"),
            )),
            "socketserver" => Some(("socketserver.py", include_str!("../stdlib/socketserver.py"))),
            "_imre_socketserver_reader" => Some((
                "_imre_socketserver_reader.py",
                include_str!("../stdlib/_socketserver_reader_typed.py"),
            )),
            "selectors" if cfg!(any(target_os = "linux", target_os = "macos")) => {
                Some(("selectors.py", include_str!("../stdlib/selectors.py")))
            }
            "doctest" => Some((
                "doctest.py",
                concat!(
                    include_str!("../stdlib/doctest.py"),
                    "\n",
                    include_str!("../stdlib/doctest_typed.py"),
                ),
            )),
            "keyword" => Some(("keyword.py", KEYWORD_SOURCE)),
            "pkgutil" => Some(("pkgutil.py", include_str!("../stdlib/pkgutil.py"))),
            "zipimport" if cfg!(unix) => Some((
                "zipimport.py",
                concat!(
                    include_str!("../stdlib/zipimport.py"),
                    "\n",
                    include_str!("../stdlib/zipimport_typed.py"),
                ),
            )),
            "platform" => Some((
                "platform.py",
                concat!(
                    include_str!("../stdlib/platform/version.py"),
                    "\n",
                    include_str!("../stdlib/platform/typed.py"),
                ),
            )),
            "mimetypes" if cfg!(unix) => {
                Some(("mimetypes.py", include_str!("../stdlib/mimetypes.py")))
            }
            "string" => Some((
                "string/__init__.py",
                include_str!("../stdlib/string/__init__.py"),
            )),
            // The boxed email package is intrinsic. Typed child modules need a package root.
            "email" => Some(("email/__init__.py", "")),
            "email.message" => Some((
                "email/message.py",
                include_str!("../stdlib/email/message.py"),
            )),
            "unittest" => Some((
                "unittest/__init__.py",
                include_str!("../stdlib/unittest/__init__.py"),
            )),
            "http.client" => Some(("http/client.py", include_str!("../stdlib/http/client.py"))),
            _ => bundled_runtime_source(name),
        };
        if let Some((filename, source)) = bundled {
            self.reject_stdlib_shadow(name)?;
            // token initializes its public tables from the live module globals.
            self.needs_object_imports |= matches!(name, "token" | "http.client");
            let pkgutil_source;
            let source = if name == "pkgutil" {
                let paths = self.filesystem_search_path()?;
                let suffixes = pkgutil_suffixes();
                let discovery = include_str!("../stdlib/pkgutil/iter_modules.py");
                pkgutil_source = format!(
                    "{source}\n{discovery}\n_search_path: list[str] = {paths}\n_module_suffixes: list[str] = {suffixes}\n"
                );
                &pkgutil_source
            } else {
                source
            };
            let parent = if let Some((parent, child)) = name.rsplit_once('.') {
                Some((self.load_module(parent)?, child))
            } else {
                None
            };
            // ty resolves this immutable source at its Python import path in the
            // in-memory project. It is part of the compiler, not a disk input.
            let path = self.root.join(filename);
            let id = self.add(
                path.clone(),
                path,
                name.into(),
                filename.ends_with("/__init__.py"),
                source,
            )?;
            self.modules[id].bundled = true;
            // codeop sets compiler flags on Python instances.
            self.needs_object_imports |= name == "codeop";
            self.scan(id)?;
            self.bind_child(name, id, parent)?;
            return Ok(id);
        }
        if !self.allow_imports {
            return Err("imports require a source file and a first-party module root".into());
        }
        // Both scanning an import and resolving a dotted package parent can
        // re-enter the loader. Count active calls, including unresolved parents.
        if self.import_depth >= 96 {
            return Err("first-party import nesting exceeds 96 levels".into());
        }
        self.import_depth += 1;
        let result = self.load_new_module(name);
        self.import_depth -= 1;
        result
    }

    fn load_new_module(&mut self, name: &str) -> Result<ModuleId, String> {
        let first = name.split('.').next().unwrap_or("");
        if stdlib_top_level(first) {
            return Err(format!(
                "standard-library imports and shadowing are unsupported: {first:?}"
            ));
        }
        let parent = if let Some((parent, child)) = name.rsplit_once('.') {
            let id = match self.by_name.get(parent).copied() {
                Some(id) if self.loading.contains(&id) && self.modules[id].is_package => id,
                _ => self.load_module(parent)?,
            };
            if !self.modules[id].is_package {
                return Err(format!("{parent:?} is not a package"));
            }
            Some((id, child))
        } else {
            None
        };
        // Parent initialization can have loaded this child while exposing re-exports.
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!("cyclic import through {name:?}"));
            }
            return Ok(id);
        }
        // A regular package owns its subtree. A namespace searches its portions in path order.
        let relative = name.replace('.', "/");
        let parent_roots = parent.map(|(id, _)| &self.modules[id].search_roots);
        let roots = std::iter::once(&self.root).chain(&self.extension_roots);
        let mut resolved = None;
        let mut portions = Vec::new();
        for (root_index, root) in roots.enumerate() {
            if root_index == 0 && self.skip_current_root {
                continue;
            }
            if parent_roots.is_some_and(|roots| !roots.contains(root_index)) {
                continue;
            }
            let base = root.join(&relative);
            let package = base.join("__init__.py");
            let source = base.with_extension("py");
            let stub = base.with_extension("pyi");
            let (path, is_package, archive) = if self.fs.is_file(&package) {
                (package, true, None)
            } else {
                self.inputs.missing.push(package);
                let archive = base.with_extension(crate::NATIVE_ARCHIVE_EXTENSION);
                let installed = self.extension_roots.contains(root);
                if installed && self.fs.is_file(&archive) {
                    if !self.fs.is_file(&stub) {
                        return Err(format!(
                            "native module {name:?} requires a typed declaration at {}",
                            stub.display()
                        ));
                    }
                    let archive = self.fs.canonicalize(&archive).map_err(|e| e.to_string())?;
                    if !archive.starts_with(root) {
                        return Err(format!(
                            "archive for native module {name:?} resolves outside its extension path"
                        ));
                    }
                    (stub, false, Some(archive))
                } else if self.fs.is_file(&source) {
                    (source, false, None)
                } else if installed && self.fs.is_file(&stub) {
                    return Err(format!(
                        "native module {name:?} requires a rebuilt archive at {}",
                        archive.display()
                    ));
                } else {
                    self.inputs.missing.push(source);
                    if installed {
                        self.inputs.missing.extend([archive, stub]);
                    }
                    if self.fs.is_dir(&base) {
                        portions.push((root_index, root.clone(), base));
                    } else {
                        self.inputs.missing.push(base);
                    }
                    continue;
                }
            };
            let identity = self
                .fs
                .canonicalize(&path)
                .map_err(|error| error.to_string())?;
            if !identity.starts_with(root) {
                return Err(format!(
                    "module {name:?} resolves outside the first-party module root or extension path"
                ));
            }
            resolved = Some((path, identity, is_package, root_index, archive));
            break;
        }
        let Some((path, identity, is_package, root_index, archive)) = resolved else {
            if portions.is_empty() {
                return Err(format!(
                    "cannot resolve first-party module {name:?} under {}",
                    self.root.display()
                ));
            }
            let path = portions[0].2.clone();
            let mut indices = Vec::with_capacity(portions.len());
            for (index, root, dir) in portions {
                let identity = self.fs.canonicalize(&dir).map_err(|e| e.to_string())?;
                if !identity.starts_with(&root) {
                    return Err(format!(
                        "namespace {name:?} resolves outside its module root"
                    ));
                }
                self.inputs.directories.push((dir, identity));
                indices.push(index);
            }
            // Give the semantic database an empty module without creating an initializer
            // that would hide portions from later roots.
            let id = self.add(path.join("<namespace>.py"), path, name.into(), true, "")?;
            self.modules[id].search_roots = SearchRoots::Namespace(indices);
            self.scan(id)?;
            self.bind_child(name, id, parent)?;
            return Ok(id);
        };
        if archive.is_some() {
            let leaf = name.rsplit('.').next().unwrap();
            if !leaf.is_ascii() {
                return Err(format!(
                    "native module {name:?} requires an ASCII initialization symbol"
                ));
            }
            if self.modules.iter().any(|module| {
                module.archive.is_some() && module.name.rsplit('.').next() == Some(leaf)
            }) {
                return Err(format!(
                    "native modules share the initialization symbol PyInit_{leaf}"
                ));
            }
        }
        if let Some(&previous) = self.by_path.get(&identity) {
            return Err(format!(
                "{} is already loaded as {:?}; a source file cannot have multiple module identities",
                path.display(),
                self.modules[previous].name
            ));
        }
        let source = self
            .fs
            .read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        // ty resolves logical import paths; canonical identities enforce containment
        // and uniqueness without breaking a single internal symlink's import name.
        let id = self.add(path, identity, name.to_owned(), is_package, &source)?;
        self.modules[id].search_roots = SearchRoots::Single(root_index);
        self.modules[id].archive = archive;
        self.scan(id)?;
        self.bind_child(name, id, parent)?;
        Ok(id)
    }

    fn bind_child(
        &mut self,
        name: &str,
        id: ModuleId,
        parent: Option<(ModuleId, &str)>,
    ) -> Result<(), String> {
        if let Some((parent, child)) = parent {
            let binding = Binding::Module(id);
            if let Some(previous) = self.modules[parent].bindings.get(child)
                && *previous != binding
            {
                return Err(format!(
                    "importing {name:?} would overwrite a fixed package binding"
                ));
            }
            self.modules[parent]
                .bindings
                .insert(child.to_owned(), binding);
        }
        Ok(())
    }
}

pub(crate) fn errno_import(statement: &ast::Stmt) -> Option<&ast::StmtImportFrom> {
    let ast::Stmt::ImportFrom(import) = statement else {
        return None;
    };
    (import.level == 0
        && import
            .module
            .as_ref()
            .is_some_and(|name| name.as_str() == "errno"))
    .then_some(import)
}

fn stdlib_top_level(name: &str) -> bool {
    let vendored = ty_vendored::file_system();
    INTRINSIC_MODULES.contains(&name)
        || vendored.is_file(format!("stdlib/{name}.pyi").as_str())
        || vendored.is_file(format!("stdlib/{name}/__init__.pyi").as_str())
}

/// An optional module import may occur between ordinary imports in the entry prefix.
fn optional_stdlib_import<'a>(
    modules: &[SourceModule],
    module: ModuleId,
    guard: &'a ast::StmtTry,
) -> Option<(&'a str, &'a str)> {
    let [ast::Stmt::Import(import)] = guard.body.as_slice() else {
        return None;
    };
    let [alias] = import.names.as_slice() else {
        return None;
    };
    let [fallback] = import_error_fallback(modules, module, guard)? else {
        return None;
    };
    let name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();
    (!alias.name.contains('.')
        && stdlib_top_level(alias.name.as_str())
        && matches!(fallback, ast::Stmt::Assign(_))
        && value_name(fallback) == Some(name)
        && matches!(module_value(fallback), Some(ast::Expr::NoneLiteral(_))))
    .then_some((alias.name.as_str(), name))
}

fn import_error_fallback<'a>(
    modules: &[SourceModule],
    module: ModuleId,
    guard: &'a ast::StmtTry,
) -> Option<&'a [ast::Stmt]> {
    let [ast::ExceptHandler::ExceptHandler(handler)] = guard.handlers.as_slice() else {
        return None;
    };
    let exception = handler.type_.as_deref()?;
    (!guard.is_star
        && guard.orelse.is_empty()
        && guard.finalbody.is_empty()
        && handler.name.is_none()
        && matches!(exception, ast::Expr::Name(n) if n.id.as_str() == "ImportError")
        && resolve_binding(modules, module, exception).is_none())
    .then_some(&handler.body)
}

/// Both arms bind one name. The source importer runs the selected imports and
/// their module initializers before continuing to the next statement.
fn conditional_import<'a>(
    modules: &[SourceModule],
    module: ModuleId,
    statement: &'a ast::Stmt,
) -> Option<&'a str> {
    if let ast::Stmt::Try(guard) = statement {
        let fallback = import_error_fallback(modules, module, guard)?;
        let name = single_import_binding(&guard.body)?;
        return (single_import_binding(fallback)? == name).then_some(name);
    }
    let guard = statement.as_if_stmt()?;
    let test = guard.test.as_compare_expr()?;
    let [left, right] = test.operands.as_ref() else {
        return None;
    };
    if resolve_binding(modules, module, left) != Some(Binding::Intrinsic(Intrinsic::SysPlatform)) {
        return None;
    }
    let strings = |items: &[ast::Expr]| {
        items
            .iter()
            .all(|item| matches!(item, ast::Expr::StringLiteral(_)))
    };
    match (test.ops.as_ref(), right) {
        ([ast::CmpOp::Eq | ast::CmpOp::NotEq], ast::Expr::StringLiteral(_)) => {}
        ([ast::CmpOp::In | ast::CmpOp::NotIn], ast::Expr::Set(set)) if strings(&set.elts) => {}
        ([ast::CmpOp::In | ast::CmpOp::NotIn], ast::Expr::Tuple(tuple)) if strings(&tuple.elts) => {
        }
        _ => return None,
    }
    let [ast::Stmt::ImportFrom(import)] = guard.body.as_slice() else {
        return None;
    };
    let [alias] = import.names.as_slice() else {
        return None;
    };
    let [otherwise] = guard.elif_else_clauses.as_slice() else {
        return None;
    };
    let [fallback] = otherwise.body.as_slice() else {
        return None;
    };
    let name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();
    (otherwise.test.is_none()
        && value_name(fallback) == Some(name)
        && module_value(fallback).is_some_and(literal_export))
    .then_some(name)
}

fn single_import_binding(body: &[ast::Stmt]) -> Option<&str> {
    let aliases = match body {
        [ast::Stmt::Import(import)] => &import.names,
        [ast::Stmt::ImportFrom(import)] => &import.names,
        _ => return None,
    };
    let [alias] = aliases.as_slice() else {
        return None;
    };
    if let Some(name) = &alias.asname {
        return Some(name.as_str());
    }
    // An unaliased dotted import binds only the first component.
    let name = alias.name.as_str().split('.').next()?;
    (name != "*").then_some(name)
}

/// Recognize a module attribute test whose only branch raises.
fn module_attribute_guard(modules: &[SourceModule], id: ModuleId, statement: &ast::Stmt) -> bool {
    let ast::Stmt::If(guard) = statement else {
        return false;
    };
    if !guard.elif_else_clauses.is_empty()
        || !matches!(guard.body.as_slice(), [ast::Stmt::Raise(_)])
    {
        return false;
    }
    let test = match &*guard.test {
        ast::Expr::UnaryOp(op) if op.op == ast::UnaryOp::Not => &*op.operand,
        other => other,
    };
    matches!(test, ast::Expr::Attribute(attr) if matches!(
        resolve_binding(modules, id, &attr.value),
        Some(Binding::Module(_) | Binding::IntrinsicModule(_))
    ))
}

/// Allow an identity check with a closed builtin raise before importing a dependency.
/// Keep it in the initializer so even a failing guard runs in source order.
fn module_name_guard(statement: &ast::Stmt) -> bool {
    let ast::Stmt::If(guard) = statement else {
        return false;
    };
    let ast::Expr::Compare(test) = &*guard.test else {
        return false;
    };
    let [ast::Stmt::Raise(raise)] = guard.body.as_slice() else {
        return false;
    };
    if !guard.elif_else_clauses.is_empty()
        || !matches!(test.ops.as_ref(), [ast::CmpOp::Eq | ast::CmpOp::NotEq])
        || !matches!(test.operands.as_ref(),
            [ast::Expr::Name(n), ast::Expr::StringLiteral(_)] |
            [ast::Expr::StringLiteral(_), ast::Expr::Name(n)] if n.id == "__name__")
        || raise.cause.is_some()
    {
        return false;
    }
    let exception = match raise.exc.as_deref() {
        Some(ast::Expr::Name(name)) => name,
        Some(ast::Expr::Call(call))
            if call.arguments.keywords.is_empty()
                && call
                    .arguments
                    .args
                    .iter()
                    .all(|a| matches!(a, ast::Expr::StringLiteral(_))) =>
        {
            let ast::Expr::Name(name) = &*call.func else {
                return false;
            };
            name
        }
        _ => return false,
    };
    crate::frontend::exception_kind(exception.id.as_str()).is_some()
}

/// A closed literal tree cannot read a name introduced by a later import.
fn literal_export(expression: &ast::Expr) -> bool {
    match expression {
        ast::Expr::NoneLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_) => true,
        ast::Expr::List(seq) => seq.elts.iter().all(literal_export),
        ast::Expr::Tuple(seq) => seq.elts.iter().all(literal_export),
        _ => false,
    }
}

/// The rebuilt C interface supports METH_VARARGS. Its stub contains signatures
/// and their type imports; no Python statements in the stub run at import time.
fn native_declaration(statement: &ast::Stmt) -> bool {
    match statement {
        ast::Stmt::Import(import) => import
            .names
            .iter()
            .all(|a| matches!(a.name.as_str(), "typing" | "collections.abc")),
        ast::Stmt::ImportFrom(import) => {
            import.level == 0
                && import.names.iter().all(|a| {
                    matches!(
                        (import.module.as_ref().map(|m| m.as_str()), a.name.as_str()),
                        (Some("typing" | "collections.abc"), "Callable")
                            | (Some("__future__"), "annotations")
                    )
                })
        }
        ast::Stmt::Expr(doc) => matches!(&*doc.value, ast::Expr::StringLiteral(_)),
        ast::Stmt::FunctionDef(f) => {
            !f.is_async
                && f.type_params.is_none()
                && f.decorator_list.is_empty()
                && f.returns.is_some()
                && f.parameters.args.is_empty()
                && f.parameters.kwonlyargs.is_empty()
                && f.parameters.vararg.is_none()
                && f.parameters.kwarg.is_none()
                && f.parameters
                    .posonlyargs
                    .iter()
                    .all(|p| p.annotation().is_some() && p.default.is_none())
                && matches!(f.body.as_slice(), [ast::Stmt::Expr(s)] if matches!(&*s.value, ast::Expr::EllipsisLiteral(_)))
        }
        _ => false,
    }
}

/// A single named assignment can initialize or replace a module value.
pub(crate) fn value_name(statement: &ast::Stmt) -> Option<&str> {
    let target = match statement {
        ast::Stmt::Assign(assignment) if assignment.targets.len() == 1 => &assignment.targets[0],
        ast::Stmt::AnnAssign(assignment) => &assignment.target,
        _ => return None,
    };
    if let ast::Expr::Name(name) = target {
        Some(name.id.as_str())
    } else {
        None
    }
}

fn module_value(statement: &ast::Stmt) -> Option<&ast::Expr> {
    match statement {
        ast::Stmt::Assign(assignment) => Some(&assignment.value),
        ast::Stmt::AnnAssign(assignment) => assignment.value.as_deref(),
        _ => None,
    }
}

/// A runtime binding introduced directly in a module-level conditional branch.
fn conditional_export_name(statement: &ast::Stmt) -> Option<&str> {
    if let ast::Stmt::FunctionDef(function) = statement {
        Some(function.name.as_str())
    } else {
        module_value(statement)?;
        value_name(statement)
    }
}

#[cfg(test)]
mod native_tests {
    use super::SourceGraph;
    use std::fs;

    #[test]
    fn native_declarations_follow_package_ownership_and_preserve_initializers() {
        let workspace = tempfile::tempdir().unwrap();
        let app = workspace.path().join("app");
        let wheel = workspace.path().join("wheel");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(wheel.join("pkg")).unwrap();
        let entry = app.join("program.py");
        fs::write(&entry, "from pkg import combine\nprint(combine(b'a'))\n").unwrap();
        fs::write(
            wheel.join("pkg/__init__.py"),
            "from .native import combine\n",
        )
        .unwrap();
        let stub = wheel.join("pkg/native.pyi");
        let archive = wheel.join("pkg/native.imre.a");
        fs::write(&stub, "def combine(data: bytes, /) -> int: ...\n").unwrap();
        let options = crate::CompileOptions {
            extension_paths: vec![wheel.clone()],
            ..Default::default()
        };
        assert!(
            crate::compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("requires a rebuilt archive")
        );
        fs::write(&archive, []).unwrap();
        let compiled = crate::compile_file(&entry, None, &options).unwrap();
        assert_eq!(
            compiled.extension_archives,
            [archive.canonicalize().unwrap()]
        );
        assert_eq!(
            compiled
                .llvm_ir
                .matches("call void @imre_extension_import(ptr @PyInit_native)")
                .count(),
            1
        );
        assert_eq!(
            compiled
                .llvm_ir
                .matches("call void @imre_extension_clear()")
                .count(),
            2
        );

        fs::write(&stub, "def combine(data: str, /) -> int: ...\n").unwrap();
        assert!(
            crate::compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("native declarations currently support")
        );

        for invalid in [
            "def combine(data: bytes) -> int: ...",
            "def combine(data: bytes = b'', /) -> int: ...",
            "def combine(*data: bytes) -> int: ...",
            "async def combine(data: bytes, /) -> int: ...",
            "@staticmethod\ndef combine(data: bytes, /) -> int: ...",
            "from elsewhere import value as combine",
        ] {
            fs::write(&stub, invalid).unwrap();
            let error =
                crate::check_file_with_extensions(&entry, None, std::slice::from_ref(&wheel))
                    .unwrap_err();
            assert!(
                error.contains("native stubs allow type-only imports"),
                "{error}"
            );
        }
        fs::write(
            &stub,
            "from typing import Callable as Callback\ndef combine(data: bytes, /) -> int: ...\n",
        )
        .unwrap();
        fs::write(&entry, "from pkg.native import Callback\n").unwrap();
        let error = crate::check_file_with_extensions(&entry, None, std::slice::from_ref(&wheel))
            .unwrap_err();
        assert!(error.contains("no static binding"), "{error}");

        fs::create_dir(app.join("pkg")).unwrap();
        fs::write(app.join("pkg/__init__.py"), "").unwrap();
        fs::write(&entry, "from pkg.native import combine\n").unwrap();
        assert!(
            crate::compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("cannot resolve first-party module")
        );
    }

    #[test]
    fn namespace_portions_track_directory_changes_and_concrete_module_precedence() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let extension = root.join("extension");
        let namespace = root.join("space");
        let other_portion = extension.join("space");
        fs::create_dir(&namespace).unwrap();
        fs::create_dir_all(&other_portion).unwrap();
        let entry = root.join("program.py");
        fs::write(&entry, "import space\n").unwrap();
        let load =
            || SourceGraph::load(&entry, Some(&root), std::slice::from_ref(&extension)).unwrap();

        let mut graph = load();
        let (inputs, _) = graph.disk_inputs(None);
        assert!(inputs.fresh());
        fs::remove_dir(&other_portion).unwrap();
        assert!(!inputs.fresh());
        fs::create_dir(&other_portion).unwrap();
        assert!(inputs.fresh());

        let stub = extension.join("space.pyi");
        fs::write(&stub, "def read() -> int: ...\n").unwrap();
        assert!(!inputs.fresh());
        fs::remove_file(stub).unwrap();
        assert!(inputs.fresh());

        let module = extension.join("space.py");
        fs::write(&module, "value = 42\n").unwrap();
        assert!(!inputs.fresh());
        let mut graph = load();
        let (_, paths) = graph.disk_inputs(None);
        assert!(paths.contains(&module));
        assert!(!paths.contains(&namespace.join("__init__.py")));
    }

    #[test]
    fn distinct_packages_cannot_supply_the_same_static_module_initializer() {
        let workspace = tempfile::tempdir().unwrap();
        let wheel = workspace.path().join("wheel");
        for package in ["left", "right"] {
            let path = wheel.join(package);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("__init__.py"), "").unwrap();
            fs::write(path.join("_core.pyi"), "def read() -> int: ...\n").unwrap();
            fs::write(path.join("_core.imre.a"), []).unwrap();
        }
        let entry = workspace.path().join("program.py");
        fs::write(&entry, "import left._core\nimport right._core\n").unwrap();
        let error = crate::check_file_with_extensions(&entry, None, &[wheel]).unwrap_err();
        assert!(
            error.contains("share the initialization symbol PyInit__core"),
            "{error}"
        );
    }
}
