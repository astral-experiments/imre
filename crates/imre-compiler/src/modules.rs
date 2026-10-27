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
pub(crate) use reachability::http_command_key;
mod type_aliases;

pub(crate) type ModuleId = usize;

/// Python source modules bundled for runtime imports.
pub(crate) fn bundled_runtime_source(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "abc" => Some(("abc.py", include_str!("../stdlib/abc.py"))),
        "contextlib" => Some((
            "contextlib.py",
            concat!(
                include_str!("../stdlib/contextlib.py"),
                "\n",
                include_str!("../stdlib/contextlib_exitstack.py"),
                "\n",
                include_str!("../stdlib/contextlib_redirect.py")
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
        "logging" => Some((
            "logging/__init__.py",
            concat!(
                include_str!("../stdlib/logging/stream.py"),
                "\n",
                include_str!("../stdlib/logging/boxed.py"),
            ),
        )),
        "mimetypes" if cfg!(unix) => Some((
            "mimetypes.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/mimetypes/upstream/Lib/mimetypes.py"
            ),
        )),
        "socketserver" => Some(("socketserver.py", include_str!("../stdlib/socketserver.py"))),
        // CPython 3.14.5 Lib/string/__init__.py, blob eab5067c9b133ea1210efacd92eb5f69c2336236.
        "string" => Some((
            "string/__init__.py",
            include_str!("../stdlib/string/cpython.py"),
        )),
        "http" => Some((
            "http/__init__.py",
            include_str!("../stdlib/http/__init__.py"),
        )),
        "http.client" => Some(("http/client.py", include_str!("../stdlib/http/client.py"))),
        "http.server" => Some(("http/server.py", include_str!("../stdlib/http/server.py"))),
        "_imre_gettext_locale" => Some((
            "_imre_gettext_locale.py",
            include_str!("../stdlib/_imre_gettext_locale.py"),
        )),
        "gettext" => Some(("gettext.py", include_str!("../stdlib/gettext.py"))),
        "uuid" => Some(("uuid.py", include_str!("../stdlib/uuid.py"))),
        _ => None,
    }
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
    /// A module guard selects the function object when the module executes.
    ConditionalFunction,
    Value(DefinitionId),
    TypeAlias(ModuleId, usize),
    IntrinsicModule(IntrinsicModule),
    Intrinsic(Intrinsic),
}

/// Sealed namespaces expose only compiler-known symbols, never module values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicModule {
    ContextVars,
    Importlib,
    ImportlibMachinery,
    ImportlibUtil,
    Array,
    AnnotationLib,
    AnnotationFormat,
    Declaration(&'static str),
    Argparse,
    Errno,
    Future,
    Builtins,
    Sys,
    SysJit,
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
    Abc,
    Asyncio,
    Contextlib,
    Inspect,
    Linecache,
    Functools,
    Fnmatch,
    Time,
    Warnings,
    Tomli,
    TomliW,
    Shutil,
    Random,
    Secrets,
    Sysconfig,
    Signal,
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
    PosixPath,
    Urllib,
    UrllibParse,
    Email,
    EmailUtils,
    Collections,
    CollectionsNative,
    CollectionsAbc,
    Copy,
    Itertools,
    Math,
    Operator,
    Textwrap,
    Io,
    Gzip,
    UnicodeData,
    WeakRef,
    Threading,
    Thread,
    Codecs,
    Socket,
    Hashlib,
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
    CachePath { reverse: bool },
    NominalClass(&'static str),
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
    SysDisplayHook,
    ArgumentParser,
    ArgparseNamespace,
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
    HandlerLevelType,
    TextOutputType,
    HandlerFallback,
    LevelMap { names: bool },
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
    InspectCoroutine { mark: bool },
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
    ShellOutput { status: bool },
    NullContext,
    IntConstant(i64),
    WallTime,
    TimeSleep,
    WarningFilter { simple: bool },
    WarningReset,
    WarningDisplay { show: bool },
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
    SysIntern { query: bool },
    SysException,
    SysGetFrameModuleName,
    SysGetRecursionLimit,
    SysSetRecursionLimit,
    SysJitPredicate(&'static str),
    Stdin,
    OsName,
    OsSeparator { alternate: bool, posix: bool },
    ErrnoAttribute,
    ErrnoFrom(usize),
    ModuleType,
    GetSysconfigPath,
    OsPathExists,
    OsDevnull,
    SysExit,
    ShlexSplit,
    ShlexQuote,
    ShlexJoin,
    Ndiff,
    DocCaptureStart,
    DocCaptureTake,
    DocDisplay,
    DocAscii,
    MockCallList { mark: bool },
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
    Sequence,
    Mapping { typing_alias: bool },
    Iterable,
    Iterator,
    UnsupportedOperation,
    GzipOpen,
    UrlsafeB64encode,
    Base64Encode,
    Base64Decode,
    BinasciiDecode,
    BinasciiEncode,
    BinasciiError,
    BinasciiIncomplete,
    Socket,
    GaiError,
    SocketGetFqdn,
    SocketConstant(&'static str),
    Sha1,
    Sha256,
    CodecInfoClass,
    UnicodeNormalize { check: bool },
    ObjectRuntimeExport(&'static str),
    WeakProxy,
    WeakProxyClass { callable: bool },
    CodecLookup,
    CodecGetWriter,
    CodecBom(&'static [u8]),
    HashNew,
    HmacCompareDigest,
    SimpleNamespace,
    HashAvailable,
    ZipFile,
    ZipInfo,
    BytesIo,
    IoBase(crate::ir::IoBase),
    StringIo,
    BadZipFile,
    BadGzipFile,
    CsvReader,
    FormatterParser,
    FormatterFieldNameSplit,
    CsvError,
    RegexCompile,
    RegexSearch,
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
    RandomChoice { secure: bool },
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
    GetEnv,
    GetCwd,
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
    SplitResult { bytes: bool },
}

impl Intrinsic {
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
            Self::Opcode | Self::StringFormatting | Self::CollectionsNative
        )
    }

    fn named(name: &str) -> Option<Self> {
        match name {
            "array" => Some(Self::Array),
            "annotationlib" => Some(Self::AnnotationLib),
            "__future__" => Some(Self::Future),
            "logging" => Some(Self::Logging),
            "logging.config" => Some(Self::LoggingConfig),
            "logging.handlers" => Some(Self::LoggingHandlers),
            "datetime" => Some(Self::Datetime),
            "abc" => Some(Self::Abc),
            "asyncio" => Some(Self::Asyncio),
            "contextvars" => Some(Self::ContextVars),
            "inspect" => Some(Self::Inspect),
            "linecache" => Some(Self::Linecache),
            "contextlib" => Some(Self::Contextlib),
            "functools" => Some(Self::Functools),
            "fnmatch" => Some(Self::Fnmatch),
            "time" => Some(Self::Time),
            "warnings" => Some(Self::Warnings),
            "tomli" => Some(Self::Tomli),
            "tomli_w" => Some(Self::TomliW),
            "shutil" => Some(Self::Shutil),
            "random" => Some(Self::Random),
            "secrets" => Some(Self::Secrets),
            "sysconfig" => Some(Self::Sysconfig),
            "signal" => Some(Self::Signal),
            "_opcode" => Some(Self::Opcode),
            "unidiff" => Some(Self::Unidiff),
            "argparse" => Some(Self::Argparse),
            "builtins" => Some(Self::Builtins),
            "sys" => Some(Self::Sys),
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
            "posixpath" => Some(Self::PosixPath),
            "email" => Some(Self::Email),
            "email.utils" => Some(Self::EmailUtils),
            "urllib" => Some(Self::Urllib),
            "urllib.parse" => Some(Self::UrllibParse),
            "importlib" => Some(Self::Importlib),
            "importlib.machinery" => Some(Self::ImportlibMachinery),
            "importlib.util" => Some(Self::ImportlibUtil),
            "collections" => Some(Self::Collections),
            "_collections" => Some(Self::CollectionsNative),
            "collections.abc" => Some(Self::CollectionsAbc),
            "copy" => Some(Self::Copy),
            "itertools" => Some(Self::Itertools),
            "math" => Some(Self::Math),
            "operator" => Some(Self::Operator),
            "textwrap" => Some(Self::Textwrap),
            "io" => Some(Self::Io),
            "gzip" => Some(Self::Gzip),
            "types" => Some(Self::Types),
            "unicodedata" => Some(Self::UnicodeData),
            "weakref" => Some(Self::WeakRef),
            "threading" => Some(Self::Threading),
            "_thread" => Some(Self::Thread),
            "codecs" => Some(Self::Codecs),
            "socket" => Some(Self::Socket),
            "hashlib" => Some(Self::Hashlib),
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
        if self == Self::Email && name == "utils" {
            return Some(Binding::IntrinsicModule(Self::EmailUtils));
        }
        if self == Self::Urllib && name == "parse" {
            return Some(Binding::IntrinsicModule(Self::UrllibParse));
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
            (Self::AnnotationLib, "get_annotations") => Intrinsic::GetAnnotations,
            (Self::AnnotationLib, "type_repr") => Intrinsic::AnnotationTypeRepr,
            (Self::AnnotationFormat, "VALUE") => Intrinsic::AnnotationFormat(1),
            (Self::AnnotationFormat, "VALUE_WITH_FAKE_GLOBALS") => Intrinsic::AnnotationFormat(2),
            (Self::AnnotationFormat, "FORWARDREF") => Intrinsic::AnnotationFormat(3),
            (Self::AnnotationFormat, "STRING") => Intrinsic::AnnotationFormat(4),
            (Self::ImportlibMachinery, "ModuleSpec") => Intrinsic::ModuleSpec,
            (Self::ImportlibUtil, "cache_from_source") => Intrinsic::CachePath { reverse: false },
            (Self::ImportlibUtil, "source_from_cache") => Intrinsic::CachePath { reverse: true },
            (Self::Future, "annotations") => Intrinsic::FutureAnnotations,
            (Self::Logging, "Formatter") => Intrinsic::LogFormatter,
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
            (Self::Abc, "abstractmethod") => Intrinsic::AbstractMethod,
            (Self::Asyncio, "create_subprocess_exec") => Intrinsic::CreateSubprocessExec,
            (Self::Asyncio, "run") => Intrinsic::RunLoop,
            (Self::ContextVars, "Context") => Intrinsic::Context,
            (Self::ContextVars, "ContextVar") => Intrinsic::ContextVar,
            (Self::ContextVars, "Token") => Intrinsic::ContextToken,
            (Self::ContextVars, "copy_context") => Intrinsic::CopyContext,
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
            (Self::Contextlib, "nullcontext") => Intrinsic::NullContext,
            (Self::Warnings, "simplefilter") => Intrinsic::WarningFilter { simple: true },
            (Self::Warnings, "filterwarnings") => Intrinsic::WarningFilter { simple: false },
            (Self::Warnings, "resetwarnings") => Intrinsic::WarningReset,
            (Self::Warnings, "formatwarning") => Intrinsic::WarningDisplay { show: false },
            (Self::Warnings, "showwarning") => Intrinsic::WarningDisplay { show: true },
            (Self::Time, "sleep") => Intrinsic::TimeSleep,
            (Self::Time, "time") => Intrinsic::WallTime,
            (Self::Signal, "SIGINT") => Intrinsic::IntConstant(2),
            (Self::Signal, "SIGTERM") => Intrinsic::IntConstant(15),
            // Native executables have no adaptive bytecode interpreter.
            (Self::Opcode, "ENABLE_SPECIALIZATION" | "ENABLE_SPECIALIZATION_FT") => {
                Intrinsic::IntConstant(0)
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
            (Self::Functools, "Placeholder") => Intrinsic::Placeholder,
            (Self::Functools, "reduce") => Intrinsic::Reduce,
            (Self::Functools, "cmp_to_key") => Intrinsic::CmpToKey,
            (Self::TomliW, "dumps") => Intrinsic::TomlDumps,
            (Self::Tomli, "loads") => Intrinsic::TomliLoads,
            (Self::Tomli, "load") => Intrinsic::TomliLoad,
            (Self::TomliW, "dump") => Intrinsic::TomliDump,
            (Self::Enum, "Enum") => Intrinsic::StandardEnum,
            (Self::Dataclasses, "is_dataclass") => Intrinsic::IsDataclass,
            (Self::Subprocess, "PIPE") => Intrinsic::Pipe,
            (Self::Subprocess, "DEVNULL") => Intrinsic::Devnull,
            (Self::Subprocess, "check_output") => Intrinsic::CheckOutput,
            (Self::Subprocess, "getoutput") => Intrinsic::ShellOutput { status: false },
            (Self::Subprocess, "getstatusoutput") => Intrinsic::ShellOutput { status: true },
            (Self::Typing, "TYPE_CHECKING") => Intrinsic::TypeChecking,
            (Self::Sys, "stdin" | "__stdin__") => Intrinsic::Stdin,
            (Self::Sys, "stdout") => Intrinsic::Stdout,
            (Self::Sys, "__stdout__") => Intrinsic::OriginalStdout,
            (Self::Sys, "stderr" | "__stderr__") => Intrinsic::Stderr,
            (Self::Sys, "exception") => Intrinsic::SysException,
            (Self::Sys, "_getframemodulename") => Intrinsic::SysGetFrameModuleName,
            (Self::Sys, "getrecursionlimit") => Intrinsic::SysGetRecursionLimit,
            (Self::Sys, "setrecursionlimit") => Intrinsic::SysSetRecursionLimit,
            (Self::SysJit, "is_available") => Intrinsic::SysJitPredicate("_jit.is_available"),
            (Self::SysJit, "is_enabled") => Intrinsic::SysJitPredicate("_jit.is_enabled"),
            (Self::SysJit, "is_active") => Intrinsic::SysJitPredicate("_jit.is_active"),
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
            (Self::Errno, _) => Intrinsic::ErrnoAttribute,
            (Self::Argparse, "ArgumentParser") => Intrinsic::ArgumentParser,
            (Self::Argparse, "Namespace") => Intrinsic::ArgparseNamespace,
            (Self::Argparse, "BooleanOptionalAction") => Intrinsic::BooleanOptionalAction,
            (Self::Argparse, "RawDescriptionHelpFormatter") => {
                Intrinsic::RawDescriptionHelpFormatter
            }
            (Self::Builtins, "_") => Intrinsic::BuiltinsLast,
            (Self::Builtins, "len") => Intrinsic::BuiltinsLen,
            (Self::Builtins, "ascii") => Intrinsic::BuiltinsAscii,
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
            (Self::Typing, "Final") => Intrinsic::Final,
            (Self::Typing, "TextIO") => Intrinsic::TextIO,
            (Self::Typing, "BinaryIO") => Intrinsic::BinaryIO,
            (Self::Typing, "Union") => Intrinsic::Union,
            (Self::Typing, "Literal") => Intrinsic::Literal,
            (Self::Typing, "Self") => Intrinsic::SelfType,
            (Self::Typing, "ClassVar") => Intrinsic::ClassVar,
            (Self::Typing, "Any") => Intrinsic::Any,
            (Self::Typing, "assert_never") => Intrinsic::AssertNever,
            (Self::Typing, "cast") => Intrinsic::Cast,
            (Self::Typing | Self::CollectionsAbc, "Callable") => Intrinsic::Callable,
            (Self::Typing, "Sequence") => Intrinsic::Sequence,
            (Self::Typing | Self::CollectionsAbc, "Mapping") => Intrinsic::Mapping {
                typing_alias: self == Self::Typing,
            },
            (Self::Typing | Self::CollectionsAbc, "Iterable") => Intrinsic::Iterable,
            (Self::Typing | Self::CollectionsAbc, "Iterator") => Intrinsic::Iterator,
            (Self::Io, "UnsupportedOperation") => Intrinsic::UnsupportedOperation,
            (Self::Gzip, "open") => Intrinsic::GzipOpen,
            (Self::Types, "ModuleType") => Intrinsic::ModuleType,
            (Self::Types, "SimpleNamespace") => Intrinsic::SimpleNamespace,
            (Self::Types, "TracebackType") => Intrinsic::NominalClass("traceback"),
            (Self::Types, "FrameType") => Intrinsic::NominalClass("frame"),
            (Self::Types, "CodeType") => Intrinsic::NominalClass("code"),
            (Self::Codecs, "CodecInfo") => Intrinsic::CodecInfoClass,
            (Self::Threading, "Lock") | (Self::Thread, "LockType") => {
                Intrinsic::ObjectRuntimeExport("_thread.lock")
            }
            (Self::Threading, "local") | (Self::Thread, "_local") => {
                Intrinsic::ObjectRuntimeExport("_thread._local")
            }
            (Self::WeakRef, "proxy") => Intrinsic::WeakProxy,
            (Self::WeakRef, "ProxyType") => Intrinsic::WeakProxyClass { callable: false },
            (Self::WeakRef, "CallableProxyType") => Intrinsic::WeakProxyClass { callable: true },
            (Self::UnicodeData, "normalize") => Intrinsic::UnicodeNormalize { check: false },
            (Self::UnicodeData, "is_normalized") => Intrinsic::UnicodeNormalize { check: true },
            (Self::Codecs, "lookup") => Intrinsic::CodecLookup,
            (Self::Codecs, "BOM_UTF8") => Intrinsic::CodecBom(b"\xef\xbb\xbf"),
            (Self::Codecs, "BOM_UTF16_LE") => Intrinsic::CodecBom(b"\xff\xfe"),
            (Self::Codecs, "BOM_UTF16_BE") => Intrinsic::CodecBom(b"\xfe\xff"),
            (Self::Codecs, "BOM_UTF32_LE") => Intrinsic::CodecBom(b"\xff\xfe\0\0"),
            (Self::Codecs, "BOM_UTF32_BE") => Intrinsic::CodecBom(b"\0\0\xfe\xff"),
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
            (Self::Hashlib, "sha1") => Intrinsic::Sha1,
            (Self::Hmac, "compare_digest") => Intrinsic::HmacCompareDigest,
            (Self::Hashlib, "sha256") => Intrinsic::Sha256,
            (Self::Base64, "urlsafe_b64encode") => Intrinsic::UrlsafeB64encode,
            (Self::Base64, "b64encode") => Intrinsic::Base64Encode,
            (Self::Base64, "b64decode") => Intrinsic::Base64Decode,
            (Self::Binascii, "a2b_base64") => Intrinsic::BinasciiDecode,
            (Self::Binascii, "b2a_base64") => Intrinsic::BinasciiEncode,
            (Self::Binascii, "Error") => Intrinsic::BinasciiError,
            (Self::Binascii, "Incomplete") => Intrinsic::BinasciiIncomplete,
            (Self::Hashlib, "new") => Intrinsic::HashNew,
            (Self::Hashlib, "algorithms_available") => Intrinsic::HashAvailable,
            (Self::Zipfile, "ZipFile") => Intrinsic::ZipFile,
            (Self::Zipfile, "ZipInfo") => Intrinsic::ZipInfo,
            (Self::Io, "BytesIO") => Intrinsic::BytesIo,
            (Self::Io, "IOBase") => Intrinsic::IoBase(crate::ir::IoBase::Any),
            (Self::Io, "RawIOBase") => Intrinsic::IoBase(crate::ir::IoBase::Raw),
            (Self::Io, "BufferedIOBase") => Intrinsic::IoBase(crate::ir::IoBase::Buffered),
            (Self::Io, "TextIOBase") => Intrinsic::IoBase(crate::ir::IoBase::Text),
            (Self::Io, "StringIO") => Intrinsic::StringIo,
            (Self::Zipfile, "BadZipFile") => Intrinsic::BadZipFile,
            (Self::Csv, "reader") => Intrinsic::CsvReader,
            (Self::StringFormatting, "formatter_parser") => Intrinsic::FormatterParser,
            (Self::StringFormatting, "formatter_field_name_split") => {
                Intrinsic::FormatterFieldNameSplit
            }
            (Self::Csv, "Error") => Intrinsic::CsvError,
            (Self::Gzip, "BadGzipFile") => Intrinsic::BadGzipFile,
            (Self::Re, "compile") => Intrinsic::RegexCompile,
            (Self::Re, "search") => Intrinsic::RegexSearch,
            (Self::Re, "sub") => Intrinsic::RegexSub,
            (Self::Re, "VERBOSE") => Intrinsic::RegexVerbose,
            (Self::Re, "UNICODE") => Intrinsic::RegexUnicode,
            (Self::Re, "Pattern") => Intrinsic::RegexPattern,
            (Self::Re, "Match") => Intrinsic::RegexMatch,
            (Self::Enum, "StrEnum") => Intrinsic::StrEnum,
            (Self::Enum, "IntEnum") => Intrinsic::IntEnum,
            (Self::Enum, "_simple_enum") => Intrinsic::SimpleEnum,
            (Self::Enum, "auto") => Intrinsic::EnumAuto,
            (Self::Collections | Self::CollectionsNative, "_count_elements") => {
                Intrinsic::CountElements
            }
            (Self::Collections | Self::CollectionsNative, "defaultdict") => Intrinsic::DefaultDict,
            (Self::Collections | Self::CollectionsNative, "deque") => Intrinsic::Deque,
            (Self::Collections, "ChainMap") => Intrinsic::ChainMap,
            (Self::Collections, "OrderedDict") => Intrinsic::OrderedDict,
            (Self::Collections, "namedtuple") => Intrinsic::NamedTuple,
            (Self::Collections, "Counter") => Intrinsic::Counter,
            (Self::Operator, "attrgetter") => Intrinsic::AttrGetter,
            (Self::Operator, "length_hint") => Intrinsic::LengthHint,
            (Self::Math, "prod") => Intrinsic::MathProd,
            (Self::Copy, "copy") => Intrinsic::ShallowCopy,
            (Self::Copy, "deepcopy") => Intrinsic::DeepCopy,
            (Self::Itertools, "count") => Intrinsic::Count,
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
            (Self::ImreJson, "decode") => Intrinsic::JsonDecode,
            (Self::Json, "loads") => Intrinsic::JsonLoads,
            (Self::Json | Self::JsonDecoder, "JSONDecoder") => Intrinsic::JsonDecoderClass,
            (Self::Json, "load") => Intrinsic::JsonLoad,
            (Self::Json, "dumps") => Intrinsic::JsonDumps,
            (Self::Json, "dump") => Intrinsic::JsonDump,
            (Self::Json, "JSONDecodeError") => Intrinsic::JsonDecodeError,
            (Self::ImreJson, "encode") => Intrinsic::JsonEncode,
            (Self::ImreToml, "decode") => Intrinsic::TomlDecode,
            (Self::Imre, "struct") => Intrinsic::Struct,
            (Self::Dataclasses, "dataclass") => Intrinsic::Dataclass,
            (Self::Dataclasses, "field") => Intrinsic::DataclassField,
            (Self::Dataclasses, "asdict") => Intrinsic::AsDict,
            (Self::Pathlib, "Path" | "PosixPath") => Intrinsic::Path,
            (Self::Pathlib, "PurePath" | "PurePosixPath") => {
                Intrinsic::PurePath(crate::ir::PathFlavor::Posix)
            }
            (Self::Pathlib, "PureWindowsPath") => {
                Intrinsic::PurePath(crate::ir::PathFlavor::Windows)
            }
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
            (Self::Posix, name) => os::symbol(name)?,
            (Self::Os, "F_OK") => Intrinsic::AccessMode(0),
            (Self::Os, "X_OK") => Intrinsic::AccessMode(1),
            (Self::Os, "W_OK") => Intrinsic::AccessMode(2),
            (Self::Os, "R_OK") => Intrinsic::AccessMode(4),
            (Self::Os, "environ") => Intrinsic::Environ,
            (Self::Os, "getenv") => Intrinsic::GetEnv,
            (Self::Os, "getcwd") => Intrinsic::GetCwd,
            (Self::Os, "fspath") => Intrinsic::FsPath,
            (Self::Os, "fsdecode") => Intrinsic::FsDecode,
            (Self::Os, "fsencode") => Intrinsic::FsEncode,
            (Self::PosixPath, "normcase") => Intrinsic::PosixNormcase,
            (Self::Os, "devnull") => Intrinsic::OsDevnull,
            (Self::PosixPath, "exists") => Intrinsic::OsPathExists,
            (Self::PosixPath, "commonpath") => Intrinsic::PosixPath(PosixPathOperation::Commonpath),
            (Self::PosixPath, "join") => Intrinsic::PosixPath(PosixPathOperation::Join),
            (Self::PosixPath, "basename") => Intrinsic::PosixPath(PosixPathOperation::Basename),
            (Self::PosixPath, "dirname") => Intrinsic::PosixPath(PosixPathOperation::Dirname),
            (Self::PosixPath, "splitext") => Intrinsic::PosixPath(PosixPathOperation::Splitext),
            (Self::PosixPath, "normpath") => Intrinsic::PosixPath(PosixPathOperation::Normpath),
            (Self::PosixPath, "abspath") => Intrinsic::PosixPath(PosixPathOperation::Abspath),
            (Self::UrllibParse, "quote") => Intrinsic::Quote,
            (Self::UrllibParse, "quote_plus") => Intrinsic::QuotePlus,
            (Self::UrllibParse, "urlencode") => Intrinsic::UrlEncode,
            (Self::UrllibParse, "unquote") => Intrinsic::Unquote,
            (Self::EmailUtils, "parsedate_to_datetime") => Intrinsic::EmailParseDate,
            (Self::EmailUtils, "format_datetime") => Intrinsic::EmailFormatDateTime,
            (Self::EmailUtils, "formatdate") => Intrinsic::EmailFormatDate,
            (Self::UrllibParse, "urljoin") => Intrinsic::UrlJoin,
            (Self::UrllibParse, "parse_qsl") => Intrinsic::ParseQsl,
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
    pub(crate) aliases: Vec<TypeAlias>,
    /// Decorated declarations register their original function at module initialization.
    pub(crate) overloads: HashSet<usize>,
    pub(crate) type_guards: Vec<usize>,
    /// Fixed pattern declarations in the entry module precede ordinary executable statements.
    pub(crate) entry_globals: Vec<usize>,
    /// Mutable intrinsic exports read at the source from-import, keyed by statement.
    pub(crate) snapshots: HashMap<usize, Intrinsic>,
    /// Executable entry statements, or inactive main guards that still require checking.
    pub(crate) executable: Vec<usize>,
    is_package: bool,
    root_index: usize,
    /// A rebuilt module is declared by a .pyi beside its platform-specific archive.
    pub(crate) archive: Option<PathBuf>,
}

impl SourceModule {
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
    pub(crate) empty_keywords: empty_keywords::EmptyKeywords,
    checked_diagnostics: std::cell::RefCell<Vec<Vec<CheckedDiagnostic>>>,
    pub(crate) needs_object_runtime: std::cell::Cell<bool>,
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
}

impl SourceGraph {
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
            !module.bundled && !(id == self.entry && self.virtual_command)
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
        graph.command_startup = Some((command.flags, command.executable.clone()));
        graph.virtual_command = true;
        Ok(graph)
    }

    /// A directly invoked file is __main__; its enclosing package is not imported.
    pub(crate) fn script(
        path: &Path,
        flags: crate::CommandFlags,
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
        let source = fs
            .read_to_string(&entry)
            .map_err(|err| format!("{}: {err}", path.display()))?;
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
        graph.command_startup = Some((flags, executable.to_owned()));
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
        resolve_binding(&self.modules, module, expression)
    }

    /// A postponed annotation may reference a type imported under TYPE_CHECKING.
    pub(crate) fn resolve_type(&self, module: ModuleId, expression: &ast::Expr) -> Option<Binding> {
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

fn importlib_child(name: &str) -> Option<IntrinsicModule> {
    match name {
        "machinery" => Some(IntrinsicModule::ImportlibMachinery),
        "util" => Some(IntrinsicModule::ImportlibUtil),
        _ => None,
    }
}

fn importlib_child_name(module: IntrinsicModule) -> Option<&'static str> {
    match module {
        IntrinsicModule::ImportlibMachinery => Some("machinery"),
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
                | Binding::ConditionalFunction
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
                    | (IntrinsicModule::Inspect, ast::Stmt::FunctionDef(_))
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
    inputs: crate::SourceInputs,
    root: PathBuf,
    extension_roots: Vec<PathBuf>,
    session: Session,
    modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    by_path: HashMap<PathBuf, ModuleId>,
    loading: HashSet<ModuleId>,
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
            root,
            modules: Vec::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            loading: HashSet::new(),
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
            root_index: 0,
            archive: None,
            bindings: HashMap::new(),
            type_bindings: HashMap::new(),
            importlib_children: HashMap::new(),
            aliases: Vec::new(),
            overloads: HashSet::new(),
            type_guards: Vec::new(),
            entry_globals: Vec::new(),
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
        self.doctest_program()?;
        self.inputs.missing.sort_unstable();
        self.inputs.missing.dedup();
        let mut checked_diagnostics = vec![Vec::new(); self.modules.len()];
        for (id, name) in self.checked_builtin_imports {
            checked_diagnostics[id].push(CheckedDiagnostic::BuiltinImport(name));
        }
        let mut graph = SourceGraph {
            command_startup: None,
            virtual_command: false,
            empty_keywords: empty_keywords::EmptyKeywords::default(),
            checked_diagnostics: std::cell::RefCell::new(checked_diagnostics),
            needs_object_runtime: std::cell::Cell::new(self.needs_object_imports),
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
        graph.mutable_builtin_len = graph.writes_builtin_len();
        Ok(graph)
    }

    fn scan(&mut self, id: ModuleId) -> Result<(), String> {
        let parsed = self.modules[id].parsed.clone();
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
                                || (module.is_package
                                    && value_name(stmt).is_some()
                                    && module_value(stmt).is_some_and(literal_export))
                        });
                        let ordered = !module.is_package
                            && module.name.contains('.')
                            && !module.bundled
                            && module.archive.is_none()
                            && prefix.clone().all(|stmt| {
                                module_name_guard(stmt)
                                    || value_name(stmt).is_some() && module_value(stmt).is_some()
                                    || matches!(stmt, ast::Stmt::Expr(_))
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
                    if let ast::Stmt::If(guard) = statement
                        && guard
                            .elif_else_clauses
                            .last()
                            .is_some_and(|clause| clause.test.is_none())
                    {
                        for function in guard
                            .body
                            .iter()
                            .filter_map(ast::Stmt::as_function_def_stmt)
                        {
                            let name = function.name.as_str();
                            if guard.elif_else_clauses.iter().all(|clause| {
                                clause
                                    .body
                                    .iter()
                                    .filter_map(ast::Stmt::as_function_def_stmt)
                                    .any(|other| other.name.as_str() == name)
                            }) {
                                self.bind(id, name, Binding::ConditionalFunction, function)?;
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
                    if let Some(name) = value_name(statement)
                        && (id != 0 || entry_global)
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
                if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                    return Err("wildcard imports are unsupported".into());
                }
                let name = self.import_name(id, import)?;
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
                        self.needs_object_imports |= matches!(
                            binding,
                            Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(_))
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
                                Binding::ConditionalFunction => false,
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
                    } else {
                        return Err(format!(
                            "module {name:?} has no static binding {:?}",
                            alias.name.as_str()
                        ));
                    };
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
        if module.is_some() {
            self.reject_stdlib_shadow(name)?;
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
        // A named import loads only the bundled definitions it can call.
        let bundles = [
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
                    include_str!("../stdlib/logging/stream.py"),
                    "\n",
                    include_str!("../stdlib/logging/classes.py")
                ),
            )),
            (module == Some(IntrinsicModule::LoggingHandlers)).then_some((
                LOGGING_HANDLER_CLASSES,
                include_str!("../stdlib/logging/handlers.py"),
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

    fn load_module(&mut self, name: &str) -> Result<ModuleId, String> {
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
            "_imre_socketserver_reader" => Some((
                "_imre_socketserver_reader.py",
                include_str!("../stdlib/_socketserver_reader_typed.py"),
            )),
            "selectors" if cfg!(any(target_os = "linux", target_os = "macos")) => {
                Some(("selectors.py", include_str!("../stdlib/selectors.py")))
            }
            "decimal" => Some(("decimal.py", include_str!("../stdlib/decimal.py"))),
            "doctest" => Some(("doctest.py", include_str!("../stdlib/doctest.py"))),
            "pkgutil" => Some(("pkgutil.py", include_str!("../stdlib/pkgutil.py"))),
            "mimetypes" if cfg!(unix) => {
                Some(("mimetypes.py", include_str!("../stdlib/mimetypes.py")))
            }
            "string" => Some((
                "string/__init__.py",
                include_str!("../stdlib/string/__init__.py"),
            )),
            "unittest" => Some((
                "unittest/__init__.py",
                include_str!("../stdlib/unittest/__init__.py"),
            )),
            "unittest.mock" => Some((
                "unittest/mock.py",
                include_str!("../stdlib/unittest/mock.py"),
            )),
            _ => bundled_runtime_source(name),
        };
        if let Some((filename, source)) = bundled {
            self.reject_stdlib_shadow(name)?;
            let pkgutil_source;
            let source = if name == "pkgutil" {
                let roots = std::iter::once(&self.root)
                    .skip(usize::from(self.skip_current_root))
                    .chain(&self.extension_roots)
                    .collect::<Vec<_>>();
                let paths = serde_json::to_string(&roots).map_err(|e| e.to_string())?;
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
                let suffixes = serde_json::to_string(&[
                    tagged.as_str(),
                    ".abi3.so",
                    &archive,
                    ".so",
                    ".py",
                    ".pyc",
                ])
                .unwrap();
                pkgutil_source = format!(
                    "{source}\n_search_path: list[str] = {paths}\n_module_suffixes: list[str] = {suffixes}\n"
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
            self.scan(id)?;
            self.bind_child(name, id, parent)?;
            return Ok(id);
        }
        if name == "stat" {
            self.reject_stdlib_shadow(name)?;
            let prefix = if cfg!(target_os = "macos") {
                include_str!("../stdlib/stat/darwin.py")
            } else {
                include_str!("../stdlib/stat/linux.py")
            };
            let source = format!("{prefix}\n{}", include_str!("../stdlib/stat/common.py"));
            let path = self.root.join("stat.py");
            let id = self.add(path.clone(), path, name.into(), false, &source)?;
            self.modules[id].bundled = true;
            self.scan(id)?;
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
        let vendored = ty_vendored::file_system();
        if INTRINSIC_MODULES.contains(&first)
            || vendored.is_file(format!("stdlib/{first}.pyi").as_str())
            || vendored.is_file(format!("stdlib/{first}/__init__.pyi").as_str())
        {
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
        // A package owns its entire subtree, even when another search root contains
        // a directory with the same name. Only top-level imports search all roots.
        let relative = name.replace('.', "/");
        let parent_root = parent.map(|(id, _)| self.modules[id].root_index);
        let roots = std::iter::once(&self.root).chain(&self.extension_roots);
        let mut resolved = None;
        for (root_index, root) in roots.enumerate() {
            if root_index == 0 && self.skip_current_root {
                continue;
            }
            if parent_root.is_some_and(|index| index != root_index) {
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
            return Err(format!(
                "cannot resolve first-party module {name:?} under {}; external and namespace-package imports are unsupported",
                self.root.display()
            ));
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
        self.modules[id].root_index = root_index;
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

/// Module values use one unconditional, fixed binding; their contents may mutate.
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

#[cfg(test)]
mod native_tests {
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
