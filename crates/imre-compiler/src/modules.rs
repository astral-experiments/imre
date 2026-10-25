//! Loads a closed graph of first-party modules and orders their initialization.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use crate::analysis::Session;
use crate::ir::PosixPathOperation;
use crate::watch::FileSystem;

mod empty_keywords;
mod entry_profile;
mod reachability;

pub(crate) type ModuleId = usize;

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

/// A source definition's identity, independent of its eventual runtime representation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DefinitionId {
    pub(crate) module: ModuleId,
    pub(crate) statement: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Binding {
    Module(ModuleId),
    Definition(DefinitionId),
    Value(DefinitionId),
    IntrinsicModule(IntrinsicModule),
    Intrinsic(Intrinsic),
}

/// Sealed namespaces expose only compiler-known symbols, never module values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicModule {
    Argparse,
    Future,
    Sys,
    Typing,
    Dataclasses,
    Difflib,
    Logging,
    LoggingHandlers,
    Datetime,
    Abc,
    Asyncio,
    Contextlib,
    Functools,
    Time,
    Tomli,
    TomliW,
    Shutil,
    Sysconfig,
    Signal,
    Unidiff,
    Pathlib,
    Shlex,
    Tempfile,
    Subprocess,
    Os,
    PosixPath,
    Urllib,
    UrllibParse,
    Collections,
    CollectionsAbc,
    Itertools,
    Operator,
    Textwrap,
    Io,
    Gzip,
    Hashlib,
    Zipfile,
    Re,
    Enum,
    Zlib,
    Json,
    Imre,
    ImreJson,
    ImreToml,
    Tomllib,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic {
    ArgumentParser,
    ArgparseNamespace,
    RawDescriptionHelpFormatter,
    FutureAnnotations,
    TypeChecking,
    GetLogger,
    GetLevelName,
    AddLevelName,
    LevelNamesMapping,
    LevelMap { names: bool },
    BasicConfig,
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
    FunctionCache,
    LruCache,
    Reduce,
    TomlDumps,
    TomliLoads,
    TomliLoad,
    TomliDump,
    StandardEnum,
    IsDataclass,
    Pipe,
    Devnull,
    CheckOutput,
    NullContext,
    SignalNumber(i64),
    WallTime,
    PatchSet,
    Awaitable,
    TypeVar,
    Stderr,
    Stdout,
    SysExecutable,
    SysPlatform,
    SysEncoding(&'static str),
    SysException,
    Stdin,
    OsName,
    GetSysconfigPath,
    OsPathExists,
    OsDevnull,
    SysExit,
    ShlexSplit,
    Ndiff,
    Final,
    Argv,
    Logger,
    LogRoot,
    LogEmit(Option<u8>),
    TimedFileHandler,
    ClockTime,
    TextIO,
    BinaryIO,
    Union,
    Literal,
    Callable,
    Sequence,
    Iterable,
    Iterator,
    UnsupportedOperation,
    GzipOpen,
    Sha256,
    HashNew,
    HashAvailable,
    ZipFile,
    ZipInfo,
    BytesIo,
    BadZipFile,
    BadGzipFile,
    RegexCompile,
    RegexSearch,
    RegexSub,
    RegexVerbose,
    RegexUnicode,
    RegexPattern,
    RegexMatch,
    StrEnum,
    EnumAuto,
    DefaultDict,
    Counter,
    Repeat,
    LengthHint,
    GroupBy,
    Chain,
    AbstractSet,
    TomllibLoad,
    TomlDecodeError,
    Dedent,
    ZlibError,
    JsonDecode,
    JsonEncode,
    JsonDumps,
    JsonDump,
    JsonLoads,
    JsonLoad,
    JsonDecodeError,
    Any,
    TomlDecode,
    Dataclass,
    DataclassField,
    AsDict,
    SelfType,
    ClassVar,
    AssertNever,
    Cast,
    Path,
    PurePath(crate::ir::PathFlavor),
    Which,
    TemporaryDirectory,
    SubprocessRun,
    SubprocessStdout,
    CompletedProcess,
    SubprocessError,
    CalledProcessError,
    TimeoutExpired,
    AccessMode(u8),
    Environ,
    GetEnv,
    GetCwd,
    FsPath,
    PosixPath(PosixPathOperation),
    Unquote,
}

impl IntrinsicModule {
    fn named(name: &str) -> Option<Self> {
        match name {
            "__future__" => Some(Self::Future),
            "logging" => Some(Self::Logging),
            "logging.handlers" => Some(Self::LoggingHandlers),
            "datetime" => Some(Self::Datetime),
            "abc" => Some(Self::Abc),
            "asyncio" => Some(Self::Asyncio),
            "contextlib" => Some(Self::Contextlib),
            "functools" => Some(Self::Functools),
            "time" => Some(Self::Time),
            "tomli" => Some(Self::Tomli),
            "tomli_w" => Some(Self::TomliW),
            "shutil" => Some(Self::Shutil),
            "sysconfig" => Some(Self::Sysconfig),
            "signal" => Some(Self::Signal),
            "unidiff" => Some(Self::Unidiff),
            "argparse" => Some(Self::Argparse),
            "sys" => Some(Self::Sys),
            "typing" => Some(Self::Typing),
            "dataclasses" => Some(Self::Dataclasses),
            "difflib" => Some(Self::Difflib),
            "pathlib" => Some(Self::Pathlib),
            "shlex" => Some(Self::Shlex),
            "tempfile" => Some(Self::Tempfile),
            "subprocess" => Some(Self::Subprocess),
            "os" => Some(Self::Os),
            "os.path" | "posixpath" => Some(Self::PosixPath),
            "urllib" => Some(Self::Urllib),
            "urllib.parse" => Some(Self::UrllibParse),
            "collections" => Some(Self::Collections),
            "collections.abc" => Some(Self::CollectionsAbc),
            "itertools" => Some(Self::Itertools),
            "operator" => Some(Self::Operator),
            "textwrap" => Some(Self::Textwrap),
            "io" => Some(Self::Io),
            "gzip" => Some(Self::Gzip),
            "hashlib" => Some(Self::Hashlib),
            "zipfile" => Some(Self::Zipfile),
            "re" => Some(Self::Re),
            "enum" => Some(Self::Enum),
            "zlib" => Some(Self::Zlib),
            "json" => Some(Self::Json),
            "imre" => Some(Self::Imre),
            "imre.json" => Some(Self::ImreJson),
            "imre.toml" => Some(Self::ImreToml),
            "tomllib" => Some(Self::Tomllib),
            _ => None,
        }
    }

    fn symbol(self, name: &str) -> Option<Binding> {
        if self == Self::Logging && name == "handlers" {
            return Some(Binding::IntrinsicModule(Self::LoggingHandlers));
        }
        if self == Self::Os && name == "path" {
            return Some(Binding::IntrinsicModule(Self::PosixPath));
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
            (Self::Future, "annotations") => Intrinsic::FutureAnnotations,
            (Self::Logging, "getLogger") => Intrinsic::GetLogger,
            (Self::Logging, "getLevelName") => Intrinsic::GetLevelName,
            (Self::Logging, "addLevelName") => Intrinsic::AddLevelName,
            (Self::Logging, "getLevelNamesMapping") => Intrinsic::LevelNamesMapping,
            (Self::Logging, "_nameToLevel") => Intrinsic::LevelMap { names: true },
            (Self::Logging, "_levelToName") => Intrinsic::LevelMap { names: false },
            (Self::Logging, "basicConfig") => Intrinsic::BasicConfig,
            (Self::Logging, "DEBUG") => Intrinsic::LogLevel(10),
            (Self::Logging, "INFO") => Intrinsic::LogLevel(20),
            (Self::Abc, "ABC") => Intrinsic::Abc,
            (Self::Abc, "abstractmethod") => Intrinsic::AbstractMethod,
            (Self::Asyncio, "create_subprocess_exec") => Intrinsic::CreateSubprocessExec,
            (Self::Asyncio, "run") => Intrinsic::RunLoop,
            (Self::Asyncio, "new_event_loop") => Intrinsic::NewLoop,
            (Self::Asyncio, "set_event_loop") => Intrinsic::SetLoop,
            (Self::Asyncio, "ensure_future") => Intrinsic::EnsureTask,
            (Self::Asyncio, "CancelledError") => Intrinsic::CancelledError,
            (Self::Asyncio, "gather") => Intrinsic::Gather,
            (Self::Asyncio, "TaskGroup") => Intrinsic::TaskGroup,
            (Self::Asyncio, "Semaphore") => Intrinsic::Semaphore,
            (Self::Contextlib, "contextmanager") => Intrinsic::ContextManager,
            (Self::Contextlib, "nullcontext") => Intrinsic::NullContext,
            (Self::Time, "time") => Intrinsic::WallTime,
            (Self::Signal, "SIGINT") => Intrinsic::SignalNumber(2),
            (Self::Signal, "SIGTERM") => Intrinsic::SignalNumber(15),
            (Self::Unidiff, "PatchSet") => Intrinsic::PatchSet,
            (Self::CollectionsAbc, "Awaitable") => Intrinsic::Awaitable,
            (Self::Typing, "TypeVar") => Intrinsic::TypeVar,
            (Self::Functools, "cache") => Intrinsic::FunctionCache,
            (Self::Functools, "lru_cache") => Intrinsic::LruCache,
            (Self::Functools, "reduce") => Intrinsic::Reduce,
            (Self::TomliW, "dumps") => Intrinsic::TomlDumps,
            (Self::Tomli, "loads") => Intrinsic::TomliLoads,
            (Self::Tomli, "load") => Intrinsic::TomliLoad,
            (Self::TomliW, "dump") => Intrinsic::TomliDump,
            (Self::Enum, "Enum") => Intrinsic::StandardEnum,
            (Self::Dataclasses, "is_dataclass") => Intrinsic::IsDataclass,
            (Self::Subprocess, "PIPE") => Intrinsic::Pipe,
            (Self::Subprocess, "DEVNULL") => Intrinsic::Devnull,
            (Self::Subprocess, "check_output") => Intrinsic::CheckOutput,
            (Self::Typing, "TYPE_CHECKING") => Intrinsic::TypeChecking,
            (Self::Sys, "stdin" | "__stdin__") => Intrinsic::Stdin,
            (Self::Sys, "stdout" | "__stdout__") => Intrinsic::Stdout,
            (Self::Sys, "stderr" | "__stderr__") => Intrinsic::Stderr,
            (Self::Sys, "exception") => Intrinsic::SysException,
            (Self::Sys, "getdefaultencoding") => Intrinsic::SysEncoding("getdefaultencoding"),
            (Self::Sys, "getfilesystemencoding") => Intrinsic::SysEncoding("getfilesystemencoding"),
            (Self::Sys, "platform") => Intrinsic::SysPlatform,
            (Self::Os, "name") => Intrinsic::OsName,
            (Self::Argparse, "ArgumentParser") => Intrinsic::ArgumentParser,
            (Self::Argparse, "Namespace") => Intrinsic::ArgparseNamespace,
            (Self::Argparse, "RawDescriptionHelpFormatter") => {
                Intrinsic::RawDescriptionHelpFormatter
            }
            (Self::Sys, "argv") => Intrinsic::Argv,
            (Self::Sys, "executable") => Intrinsic::SysExecutable,
            (Self::Sysconfig, "get_path") => Intrinsic::GetSysconfigPath,
            (Self::Sys, "exit") => Intrinsic::SysExit,
            (Self::LoggingHandlers, "TimedRotatingFileHandler") => Intrinsic::TimedFileHandler,
            (Self::Datetime, "time") => Intrinsic::ClockTime,
            (Self::Logging, "Logger") => Intrinsic::Logger,
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
            (Self::Difflib, "ndiff") => Intrinsic::Ndiff,
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
            (Self::Typing | Self::CollectionsAbc, "Sequence") => Intrinsic::Sequence,
            (Self::Typing | Self::CollectionsAbc, "Iterable") => Intrinsic::Iterable,
            (Self::Typing | Self::CollectionsAbc, "Iterator") => Intrinsic::Iterator,
            (Self::Io, "UnsupportedOperation") => Intrinsic::UnsupportedOperation,
            (Self::Gzip, "open") => Intrinsic::GzipOpen,
            (Self::Hashlib, "sha256") => Intrinsic::Sha256,
            (Self::Hashlib, "new") => Intrinsic::HashNew,
            (Self::Hashlib, "algorithms_available") => Intrinsic::HashAvailable,
            (Self::Zipfile, "ZipFile") => Intrinsic::ZipFile,
            (Self::Zipfile, "ZipInfo") => Intrinsic::ZipInfo,
            (Self::Io, "BytesIO") => Intrinsic::BytesIo,
            (Self::Zipfile, "BadZipFile") => Intrinsic::BadZipFile,
            (Self::Gzip, "BadGzipFile") => Intrinsic::BadGzipFile,
            (Self::Re, "compile") => Intrinsic::RegexCompile,
            (Self::Re, "search") => Intrinsic::RegexSearch,
            (Self::Re, "sub") => Intrinsic::RegexSub,
            (Self::Re, "VERBOSE") => Intrinsic::RegexVerbose,
            (Self::Re, "UNICODE") => Intrinsic::RegexUnicode,
            (Self::Re, "Pattern") => Intrinsic::RegexPattern,
            (Self::Re, "Match") => Intrinsic::RegexMatch,
            (Self::Enum, "StrEnum") => Intrinsic::StrEnum,
            (Self::Enum, "auto") => Intrinsic::EnumAuto,
            (Self::Collections, "defaultdict") => Intrinsic::DefaultDict,
            (Self::Collections, "Counter") => Intrinsic::Counter,
            (Self::Operator, "length_hint") => Intrinsic::LengthHint,
            (Self::Itertools, "repeat") => Intrinsic::Repeat,
            (Self::Itertools, "groupby") => Intrinsic::GroupBy,
            (Self::Itertools, "chain") => Intrinsic::Chain,
            (Self::CollectionsAbc, "Set") => Intrinsic::AbstractSet,
            (Self::Tomllib, "load") => Intrinsic::TomllibLoad,
            (Self::Tomllib, "TOMLDecodeError") => Intrinsic::TomlDecodeError,
            (Self::Textwrap, "dedent") => Intrinsic::Dedent,
            (Self::Zlib, "error") => Intrinsic::ZlibError,
            (Self::ImreJson, "decode") => Intrinsic::JsonDecode,
            (Self::Json, "loads") => Intrinsic::JsonLoads,
            (Self::Json, "load") => Intrinsic::JsonLoad,
            (Self::Json, "dumps") => Intrinsic::JsonDumps,
            (Self::Json, "dump") => Intrinsic::JsonDump,
            (Self::Json, "JSONDecodeError") => Intrinsic::JsonDecodeError,
            (Self::ImreJson, "encode") => Intrinsic::JsonEncode,
            (Self::ImreToml, "decode") => Intrinsic::TomlDecode,
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
            (Self::Shutil, "which") => Intrinsic::Which,
            (Self::Tempfile, "TemporaryDirectory") => Intrinsic::TemporaryDirectory,
            (Self::Subprocess, "STDOUT") => Intrinsic::SubprocessStdout,
            (Self::Subprocess, "run") => Intrinsic::SubprocessRun,
            (Self::Subprocess, "CompletedProcess") => Intrinsic::CompletedProcess,
            (Self::Subprocess, "SubprocessError") => Intrinsic::SubprocessError,
            (Self::Subprocess, "CalledProcessError") => Intrinsic::CalledProcessError,
            (Self::Subprocess, "TimeoutExpired") => Intrinsic::TimeoutExpired,
            (Self::Os, "F_OK") => Intrinsic::AccessMode(0),
            (Self::Os, "X_OK") => Intrinsic::AccessMode(1),
            (Self::Os, "W_OK") => Intrinsic::AccessMode(2),
            (Self::Os, "R_OK") => Intrinsic::AccessMode(4),
            (Self::Os, "environ") => Intrinsic::Environ,
            (Self::Os, "getenv") => Intrinsic::GetEnv,
            (Self::Os, "getcwd") => Intrinsic::GetCwd,
            (Self::Os, "fspath") => Intrinsic::FsPath,
            (Self::Os, "devnull") => Intrinsic::OsDevnull,
            (Self::PosixPath, "exists") => Intrinsic::OsPathExists,
            (Self::PosixPath, "commonpath") => Intrinsic::PosixPath(PosixPathOperation::Commonpath),
            (Self::PosixPath, "join") => Intrinsic::PosixPath(PosixPathOperation::Join),
            (Self::PosixPath, "basename") => Intrinsic::PosixPath(PosixPathOperation::Basename),
            (Self::PosixPath, "dirname") => Intrinsic::PosixPath(PosixPathOperation::Dirname),
            (Self::PosixPath, "splitext") => Intrinsic::PosixPath(PosixPathOperation::Splitext),
            (Self::PosixPath, "normpath") => Intrinsic::PosixPath(PosixPathOperation::Normpath),
            (Self::PosixPath, "abspath") => Intrinsic::PosixPath(PosixPathOperation::Abspath),
            (Self::UrllibParse, "unquote") => Intrinsic::Unquote,
            _ => return None,
        };
        Some(Binding::Intrinsic(symbol))
    }
}

pub(crate) struct SourceModule {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) source: String,
    bundled: bool,
    pub(crate) parsed: ParsedModuleRef,
    pub(crate) bindings: HashMap<String, Binding>,
    type_bindings: HashMap<String, Binding>,
    pub(crate) type_guards: Vec<usize>,
    /// Fixed pattern declarations in the entry module precede ordinary executable statements.
    pub(crate) entry_globals: Vec<usize>,
    /// Executable entry statements, or inactive main guards that still require checking.
    pub(crate) executable: Vec<usize>,
    is_package: bool,
    root_index: usize,
    /// A rebuilt module is declared by a .pyi beside its platform-specific archive.
    pub(crate) archive: Option<PathBuf>,
}

impl SourceModule {
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
    pub(crate) empty_keywords: empty_keywords::EmptyKeywords,
    pub(crate) entry_profile: entry_profile::Profile,
    pub(crate) mutable_documents: bool,
    reachable: Option<reachability::Reachable>,
    pub(crate) inputs: crate::SourceInputs,
    pub(crate) session: Session,
    pub(crate) modules: Vec<SourceModule>,
    pub(crate) entry: ModuleId,
    /// The absolute entry spelling used for Python's `__file__`.
    entry_filename: PathBuf,
    /// Split literal prefixes at imports so allocations and package exports are
    /// available to the child at the original point in Python's initialization.
    initialization: Vec<(ModuleId, Range<usize>)>,
}

impl SourceGraph {
    /// Keep module indices stable for the semantic cache. Only after lowering
    /// expose the files a disk build must revalidate and protect from writes.
    pub(crate) fn disk_inputs(
        &mut self,
        profile: Option<&Path>,
    ) -> (crate::SourceInputs, Vec<PathBuf>) {
        let mut inputs = std::mem::take(&mut self.inputs);
        let mut modules = self.modules.iter();
        inputs
            .files
            .retain(|_| !modules.next().expect("one input per module").bundled);
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
    pub(crate) fn reachable_only(&self) -> bool {
        self.reachable.is_some()
    }

    pub(crate) fn includes(&self, definition: DefinitionId) -> bool {
        self.reachable
            .as_ref()
            .is_none_or(|r| r.definitions.contains(&definition))
    }

    pub(crate) fn inherits_class_method(&self, class: DefinitionId, name: &str) -> bool {
        self.reachable
            .as_ref()
            .is_none_or(|r| r.inherited_class_method(class, name))
    }

    pub(crate) fn includes_method(&self, module: ModuleId, method: &ast::StmtFunctionDef) -> bool {
        self.reachable
            .as_ref()
            .is_none_or(|r| r.methods.contains(&(module, method.start())))
    }

    pub(crate) fn module_filename(&self, module: ModuleId) -> &Path {
        if module == self.entry {
            &self.entry_filename
        } else {
            &self.modules[module].path
        }
    }

    /// Source ranges where the reachable profile or supported decorators replace ty diagnostics.
    pub(crate) fn diagnostic_scopes(&self) -> (Option<Vec<Vec<TextRange>>>, Vec<Vec<TextRange>>) {
        let context_decorators: Vec<Vec<_>> = self
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
                        .then_some(decorator.range())
                    })
                    .collect()
            })
            .collect();
        (
            self.reachable.as_ref().map(|r| r.skipped.clone()),
            context_decorators,
        )
    }

    /// The entry needs its own initializer when it declares globals for later definitions.
    pub(crate) fn initializers(&self) -> impl Iterator<Item = (ModuleId, Range<usize>)> + '_ {
        self.initialization
            .iter()
            .filter(|(id, _)| *id != self.entry || !self.modules[*id].entry_globals.is_empty())
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
                Binding::IntrinsicModule(target) => target.symbol(attribute.attr.as_str()),
                _ => None,
            },
            _ => None,
        }
    }
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
                Binding::IntrinsicModule(module) => module.symbol(attribute.attr.as_str()),
                Binding::Definition(_) | Binding::Value(_) | Binding::Intrinsic(_) => None,
            }
        }
        _ => None,
    }
}

struct Loader<'a> {
    fs: FileSystem<'a>,
    inputs: crate::SourceInputs,
    root: PathBuf,
    extension_roots: Vec<PathBuf>,
    session: Session,
    modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    by_path: HashMap<PathBuf, ModuleId>,
    loading: HashSet<ModuleId>,
    import_depth: usize,
    allow_imports: bool,
    initialization: Vec<(ModuleId, Range<usize>)>,
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
            root,
            modules: Vec::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            loading: HashSet::new(),
            import_depth: 0,
            allow_imports: true,
            initialization: Vec::new(),
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
        self.inputs.files.push((
            path.clone(),
            identity.clone(),
            blake3::hash(source.as_bytes()).to_hex().to_string(),
        ));
        let source = crate::normalize_source(source)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .into_owned();
        let parsed = self.session.add(&path, &source)?;
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
            type_guards: Vec::new(),
            entry_globals: Vec::new(),
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
                    unreachable!()
                };
                for statement in &guard.body {
                    let ast::Stmt::ImportFrom(import) = statement else {
                        continue;
                    };
                    let name = self.import_name(id, import)?;
                    for alias in &import.names {
                        let binding = self
                            .by_name
                            .get(&name)
                            .and_then(|&target| self.modules[target].export(alias.name.as_str()))
                            .or_else(|| IntrinsicModule::named(&name)?.symbol(alias.name.as_str()));
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
        self.inputs.missing.sort_unstable();
        self.inputs.missing.dedup();
        let mut graph = SourceGraph {
            empty_keywords: empty_keywords::EmptyKeywords::default(),
            entry_profile: profile
                .map(|p| entry_profile::Profile::load(p, &self.modules, &mut self.fs))
                .transpose()?
                .unwrap_or_default(),
            mutable_documents: false,
            reachable: None,
            entry_filename,
            inputs: self.inputs,
            session: self.session,
            modules: self.modules,
            entry,
            initialization: self.initialization,
        };
        graph.reachable = reachable_only.then(|| reachability::Reachable::collect(&graph));
        graph.empty_keywords = empty_keywords::EmptyKeywords::collect(&graph);
        graph.mutable_documents = graph.uses_mutable_documents();
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
                        && guard
                            .body
                            .iter()
                            .all(|s| matches!(s, ast::Stmt::ImportFrom(_))) =>
                {
                    self.modules[id].type_guards.push(index);
                }
                ast::Stmt::FunctionDef(function) => {
                    if executable && id == 0 {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    self.bind(
                        id,
                        function.name.as_str(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                ast::Stmt::ClassDef(class) => {
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
                        // Fixed literal exports can precede a package re-export.
                        // Emit the prefix now: even a list literal can fail to
                        // allocate before the next module's initializer runs.
                        if id == 0
                            || !self.modules[id].is_package
                            || !self.modules[id].executable.iter().all(|&at| {
                                let stmt = &parsed.syntax().body[at];
                                value_name(stmt).is_some()
                                    && module_value(stmt).is_some_and(literal_export)
                            })
                        {
                            return Err(self.modules[id].error(
                                statement,
                                "imports must precede executable module statements",
                            ));
                        }
                        self.initialization.push((id, segment..index));
                        segment = index;
                        executable = false;
                    }
                    self.import(id, statement)
                        .map_err(|error| self.modules[id].error(statement, &error))?;
                }
                ast::Stmt::Expr(value)
                    if matches!(&*value.value, ast::Expr::StringLiteral(_)) && !executable => {}
                _ => {
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
        self.loading.remove(&id);
        self.initialization
            .push((id, segment..parsed.syntax().body.len()));
        Ok(())
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

    fn import(&mut self, id: ModuleId, statement: &ast::Stmt) -> Result<(), String> {
        match statement {
            ast::Stmt::Import(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                for alias in &import.names {
                    let name = alias.name.as_str();
                    if let Some(module) = self.intrinsic_module(name)? {
                        if alias.asname.is_none()
                            && let Some((local, _)) = name.split_once('.')
                            && let Some(namespace) = IntrinsicModule::named(local)
                        {
                            self.bind(id, local, Binding::IntrinsicModule(namespace), alias)?;
                            continue;
                        }
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), Binding::IntrinsicModule(module), alias)?;
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
                if import.level == 0
                    && let Some(module) = self.intrinsic_module(&name)?
                {
                    for alias in &import.names {
                        let binding = module.symbol(alias.name.as_str()).ok_or_else(|| {
                            format!(
                                "unsupported sealed standard-library export {name}.{}",
                                alias.name
                            )
                        })?;
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), binding, alias)?;
                    }
                    return Ok(());
                }
                // A package currently importing its children is a namespace container,
                // not an import cycle, unless an import reads its incomplete definitions.
                let target = match self.by_name.get(&name).copied() {
                    Some(target)
                        if self.loading.contains(&target) && self.modules[target].is_package =>
                    {
                        target
                    }
                    _ => self.load_module(&name)?,
                };
                for alias in &import.names {
                    let binding = if let Some(binding) =
                        self.modules[target].export(alias.name.as_str())
                    {
                        if self.loading.contains(&target)
                            && matches!(binding, Binding::Definition(_) | Binding::Value(_))
                            && !matches!(binding, Binding::Value(def)
                                    if self.initialization.iter().any(|(module, statements)|
                                        *module == def.module && statements.contains(&def.statement)))
                        {
                            return Err(format!("cyclic import through {name:?}"));
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
                        Binding::Module(self.load_module(&format!("{name}.{}", alias.name))?)
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
    fn intrinsic_module(&mut self, name: &str) -> Result<Option<IntrinsicModule>, String> {
        let module = IntrinsicModule::named(name);
        if module.is_some() {
            self.reject_stdlib_shadow(name)?;
        }
        Ok(module)
    }

    fn reject_stdlib_shadow(&mut self, name: &str) -> Result<(), String> {
        if self.allow_imports {
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

    fn load_module(&mut self, name: &str) -> Result<ModuleId, String> {
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!(
                    "cyclic import through {name:?} (the entry module cannot import itself)"
                ));
            }
            return Ok(id);
        }
        if name == "unittest" {
            self.reject_stdlib_shadow(name)?;
            // ty resolves this immutable source at its Python import path in the
            // in-memory project. It is part of the compiler, not a disk input.
            let path = self.root.join("unittest/__init__.py");
            let id = self.add(
                path.clone(),
                path,
                name.into(),
                true,
                include_str!("../stdlib/unittest/__init__.py"),
            )?;
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
                let archive = base.with_extension("imre.a");
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
        Ok(id)
    }
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
