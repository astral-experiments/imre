//! Closed, representation-checked IR. Lowering preserves owned results and may
//! borrow rooted locals for the duration of operator and call evaluation.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Type {
    Document(DocumentShape),
    /// Read-only nodes from the sealed unified-diff parser.
    Patch(PatchKind),
    Int,
    Float,
    Bool,
    /// A builtin class object. Its name is interned; the handle is never a Python string.
    BuiltinClass,
    /// An immutable annotation reference with a known layout for its owner.
    ForwardRef(usize),
    /// One of the exact values returned by a heterogeneous annotation dictionary.
    AnnotationValue(usize),
    Str,
    /// A closed domain of strings with ordinary string storage and ownership.
    StrLiteral(usize),
    /// A nominal enum with an unboxed, nonzero canonical member tag.
    StrEnum(usize),
    /// An ordinary Enum with a closed set of string-valued members.
    Enum(usize),
    Bytes,
    ListInt,
    /// Native storage for a private append-only integer list consumed by bytes().
    ByteOutput,
    /// Private, owning accumulator for a tuple display with runtime arity.
    TupleBuffer,
    Class(usize),
    List(usize),
    Dict(usize),
    /// A ChainMap owner, parameterized by the exact dictionary shape of its maps.
    ChainMap(usize),
    Set(usize),
    Deque(usize),
    /// The immutable top-level string keys of a private TOML document.
    StringKeys,
    Tuple(usize),
    VarTuple(usize),
    /// A functools key retains its comparator and, for a bound key, its object.
    CmpKey(usize, bool),
    /// An exact positional signature carried by an owned callable record.
    Callable(usize),
    /// Private state retained by an independently constructed function cache.
    CacheState(usize),
    Optional(usize),
    /// A closed str/bytes union with one owned tagged-pointer word.
    Union(usize),
    /// An owned tagged reference to a string list or a fixed tuple of strings.
    StringIterable,
    /// An internal container or string cursor, never available in user annotations.
    Iterator,
    /// A first-class cursor whose next operation transfers one exact element type.
    Cursor(usize),
    /// A sealed builtin exception with an immutable category and owned payload.
    Exception,
    /// An exception object known to own the given fixed payload layout.
    NamedException(usize),
    /// An owned view of a concrete Python scalar, tuple, or native module attribute.
    ObjectValue,
    ReadOnlyObjectMapping,
    /// A builtin ImportError (including its ModuleNotFoundError and marker subclasses).
    ImportException,
    /// A proved catch-only timeout with readonly float and nullable bytes fields.
    TimeoutExpired,
    /// A sealed UTF-8 text stream; aliases share its open/closed state.
    TextIO,
    StringIO,
    HandlerLock,
    /// A sealed byte stream; aliases share its open/closed state.
    BinaryIO,
    /// One live imported module from the closed source graph.
    SourceModule,
    /// A sealed, immutable POSIX filesystem path with UTF-8 components.
    Path,
    PurePath(PathFlavor),
    /// The shared, independently retained metadata view of an exact concrete Path.
    PathInfo,
    /// A sealed process result whose capture modes fix each field’s ownership.
    CompletedProcess(ProcessCapture, Option<usize>),
    /// A synchronous child with statically known captured stream types.
    Popen(ProcessCapture, bool, usize),
    /// An original Popen from _active. Its stream and argv types vary by launch.
    ActivePopen,
    /// Async process handle, with bits for the captured stdout and stderr slots.
    AsyncProcess(u8),
    Coroutine(usize),
    EventLoop,
    Task(usize),
    TaskGroup,
    GeneratorContext,
    Context,
    ContextMissing,
    ContextVar(usize),
    ContextToken(usize),
    /// A native temporary-directory or null context with a known enter result.
    DirectoryContext(usize),
    Semaphore,
    /// An immutable compiled text pattern or one successful search and its captures.
    RegexPattern,
    NativeModule,
    ModuleNamespace,
    LogRecord,
    LogFormatter,
    Logger,
    TimedFileHandler,
    ClockTime,
    Calendar(CalendarType),
    /// An exact instance of Python's plain, identity-only object.
    PlainObject,
    /// A source function from a heterogeneous overload list.
    FunctionRef,
    /// The shared failing function returned by typing.overload.
    OverloadStub,
    /// An exact immutable method descriptor on the builtin str class.
    StrDescriptor(StrDescriptor),
    ProcessSelectorClass,
    /// A live namespace wrapper; the interned shape describes its string-keyed backing dict.
    SimpleNamespace(usize),
    /// Nominal ModuleSpec backed by a record whose loader and state keep their exact types.
    ModuleSpec(usize),
    CodecInfo,
    /// A POSIX IPv4 stream socket with shared descriptor ownership.
    Socket,
    CodecWriterClass,
    CodecWriter,
    Sha256,
    Poll,
    JsonDecoder,
    /// An open or closed ZIP archive retains its ordered central directory.
    ZipFile,
    ZipInfo,
    CsvReader,
    RegexMatch,
    /// Immutable selections from decoded JSON. Only the object form has a source annotation.
    JsonRoot,
    JsonValue,
    JsonObject,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CalendarType {
    DateTime,
    Delta,
    Zone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalendarOp {
    DateTime,
    Delta,
    Zone,
    Utc,
    ZoneLimit,
    Now,
    Shift { reverse: bool, subtract: bool },
    Negate,
    Field,
    TzInfo,
    Offset,
    ZoneOffset,
    ZoneName,
    Text { iso: bool },
    NullableEqual,
    TotalSeconds,
}

/// Each captured stream has one exact text or byte representation. An inherited
/// or redirected stream has no returned payload or owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ProcessCapture {
    pub stdout: bool,
    pub stderr: bool,
    pub text: bool,
}

impl ProcessCapture {
    pub(crate) const TEXT: Self = Self {
        stdout: true,
        stderr: true,
        text: true,
    };
    pub(crate) const BYTES: Self = Self {
        stdout: true,
        stderr: true,
        text: false,
    };

    pub(crate) fn stream(self, stdout: bool) -> Type {
        if !(if stdout { self.stdout } else { self.stderr }) {
            Type::None
        } else if self.text {
            Type::Str
        } else {
            Type::Bytes
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PathFlavor {
    Posix,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypingOperation {
    Register,
    Get,
    RegisterBuiltin,
    GetBuiltin,
    Clear,
    CallStub,
    Attr(u32),
    SetAttr(u32),
    RegistryLen,
    Dummy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SocketOp {
    Constant(&'static str),
    New,
    Bind,
    Listen,
    Accept,
    Connect,
    Shutdown,
    SockName,
    Recv,
    Send,
    SendAll,
    Close,
    Fileno,
    SetBlocking,
    GetBlocking,
    Dup,
    GetInheritable,
    SetInheritable,
    SetSockOpt,
    GetSockOpt,
}

impl Type {
    pub(crate) fn cache_state(self) -> Self {
        let Self::Dict(id) = self else {
            unreachable!("a function cache has a dictionary layout")
        };
        Self::CacheState(id)
    }

    pub(crate) fn cache_table(self) -> Option<Self> {
        if let Self::CacheState(id) = self {
            Some(Self::Dict(id))
        } else {
            None
        }
    }

    pub(crate) fn is_path(self) -> bool {
        matches!(self, Self::Path | Self::PurePath(_))
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Class(id) = self {
            return write!(f, "class#{id}");
        }
        match self {
            Self::StrLiteral(id) => return write!(f, "string-literal#{id}"),
            Self::StrEnum(id) => return write!(f, "string-enum#{id}"),
            Self::Enum(id) => return write!(f, "enum#{id}"),
            Self::Cursor(id) => return write!(f, "iterator#{id}"),
            Self::List(id) => return write!(f, "list#{id}"),
            Self::Dict(id) => return write!(f, "dict#{id}"),
            Self::ChainMap(id) => return write!(f, "ChainMap#{id}"),
            Self::Set(id) => return write!(f, "set#{id}"),
            Self::Deque(id) => return write!(f, "deque#{id}"),
            Self::Tuple(id) => return write!(f, "tuple#{id}"),
            Self::VarTuple(id) => return write!(f, "variable-tuple#{id}"),
            Self::Callable(id) => return write!(f, "callable#{id}"),
            Self::Optional(id) => return write!(f, "optional#{id}"),
            Self::Union(id) => return write!(f, "union#{id}"),
            _ => {}
        }
        f.write_str(match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::BuiltinClass => "type",
            Self::ForwardRef(_) => "annotationlib.ForwardRef",
            Self::AnnotationValue(_) => "annotation value",
            Self::CmpKey(..) => "functools.KeyWrapper",
            Self::Str => "str",
            Self::Bytes => "bytes",
            Self::StringKeys => "collections.abc.Set[str]",
            Self::ListInt => "list[int]",
            Self::ByteOutput => "private byte output",
            Self::TupleBuffer => "private tuple buffer",
            Self::CacheState(_) => "private function cache",
            Self::None => "None",
            Self::Iterator => "iterator",
            Self::Exception | Self::NamedException(_) => "Exception",
            Self::ObjectValue => "object value",
            Self::ReadOnlyObjectMapping => "Mapping[str, object]",
            Self::ImportException => "ImportError",
            Self::TimeoutExpired => "subprocess.TimeoutExpired",
            Self::StringIterable => "Iterable[str]",
            Self::TextIO => "typing.TextIO",
            Self::StringIO => "io.StringIO",
            Self::HandlerLock => "_thread.RLock",
            Self::BinaryIO => "typing.BinaryIO",
            Self::SourceModule => "module",
            Self::Path => "pathlib.Path",
            Self::PathInfo => "pathlib.types.PathInfo",
            Self::PurePath(PathFlavor::Posix) => "pathlib.PurePosixPath",
            Self::PurePath(PathFlavor::Windows) => "pathlib.PureWindowsPath",
            Self::CompletedProcess(capture, _) if capture.text => {
                "subprocess.CompletedProcess[str]"
            }
            Self::CompletedProcess(..) => "subprocess.CompletedProcess[bytes]",
            Self::Popen(..) | Self::ActivePopen => "subprocess.Popen",
            Self::AsyncProcess(_) => "asyncio.subprocess.Process",
            Self::Coroutine(_) => "Coroutine",
            Self::EventLoop => "asyncio.AbstractEventLoop",
            Self::Task(_) => "asyncio.Task",
            Self::TaskGroup => "asyncio.TaskGroup",
            Self::GeneratorContext => "@contextmanager",
            Self::Context => "contextvars.Context",
            Self::ContextMissing => "Token.MISSING",
            Self::ContextVar(_) => "contextvars.ContextVar",
            Self::ContextToken(_) => "contextvars.Token",
            Self::DirectoryContext(_) => "directory context",
            Self::Semaphore => "asyncio.Semaphore",
            Self::Patch(PatchKind::Set) => "unidiff.PatchSet",
            Self::Patch(PatchKind::File) => "unidiff.PatchedFile",
            Self::Patch(PatchKind::Hunk) => "unidiff.Hunk",
            Self::RegexPattern => "re.Pattern[str]",
            Self::NativeModule => "builtin module",
            Self::ModuleNamespace => "module namespace",
            Self::LogFormatter => "logging.Formatter",
            Self::LogRecord => "logging.LogRecord",
            Self::Logger => "logging.Logger",
            Self::TimedFileHandler => "logging.handlers.TimedRotatingFileHandler",
            Self::ClockTime => "datetime.time",
            Self::Calendar(CalendarType::DateTime) => "datetime.datetime",
            Self::Calendar(CalendarType::Delta) => "datetime.timedelta",
            Self::Calendar(CalendarType::Zone) => "datetime.timezone",
            Self::PlainObject => "object",
            Self::FunctionRef | Self::OverloadStub => "function",
            Self::StrDescriptor(_) => "method_descriptor",
            Self::ProcessSelectorClass => "subprocess selector class",
            Self::SimpleNamespace(_) => "types.SimpleNamespace",
            Self::ModuleSpec(_) => "importlib.machinery.ModuleSpec",
            Self::CodecInfo => "codecs.CodecInfo",
            Self::Poll => "select.poll",
            Self::Socket => "socket.socket",
            Self::CodecWriterClass => "type[codecs.StreamWriter]",
            Self::CodecWriter => "codecs.StreamWriter",
            Self::Sha256 => "SHA-256 hash",
            Self::JsonDecoder => "json.decoder.JSONDecoder",
            Self::ZipFile => "zipfile.ZipFile",
            Self::ZipInfo => "zipfile.ZipInfo",
            Self::CsvReader => "CSV reader",
            Self::RegexMatch => "re.Match[str]",
            Self::JsonRoot => "JSON document",
            Self::JsonValue => "JSON value",
            Self::JsonObject => "dict[str, Any]",
            Self::Document(DocumentShape::Any) => "TOML or JSON value",
            Self::Document(DocumentShape::Scalar) => "None, bool, int or str",
            Self::Document(DocumentShape::Map) => "mutable dict[str, Any]",
            Self::Document(DocumentShape::List) => "mutable list[Any]",
            Self::StrLiteral(_)
            | Self::StrEnum(_)
            | Self::Enum(_)
            | Self::Class(_)
            | Self::Cursor(_)
            | Self::List(_)
            | Self::Dict(_)
            | Self::ChainMap(_)
            | Self::Set(_)
            | Self::Deque(_)
            | Self::Tuple(_)
            | Self::VarTuple(_)
            | Self::Callable(_)
            | Self::Optional(_)
            | Self::Union(_) => {
                unreachable!()
            }
        })
    }
}

/// The information an actual Python call must keep until it returns or unwinds.
pub(crate) enum SourceFrame {
    Depth,
    Module(String),
}

pub(crate) struct Module {
    /// Interpreter-style -I commands ignore Python process environment variables at startup.
    pub ignore_python_environment: bool,
    /// Exact storage shapes survive checking so lowering need not erase them to
    /// generic tagged slots. Class identities index `class_fields`; container
    /// identities refer to this compilation's interned type universe.
    pub types: crate::types::Types,
    pub class_fields: Vec<Vec<Type>>,
    /// Static names and generated value behavior; instance layouts stay unchanged.
    pub class_metadata: Vec<ClassMetadata>,
    /// Single-generator list builds, keyed by the outlined helper they describe.
    pub list_builds: std::collections::HashMap<usize, ListBuild>,
    /// Concrete JSON field schemas; decoding additionally checks constructors.
    pub json_records: Vec<JsonRecordSchema>,
    /// Fixed module bindings own their exact storage until process cleanup.
    pub globals: Vec<Type>,
    /// Source functions and entry in emission order. Synthetic helpers have no frame.
    pub source_frames: Vec<Option<SourceFrame>>,
    pub functions: Vec<Function>,
    pub entry: Function,
}

pub(crate) struct ClassMetadata {
    pub name: String,
    pub qualified_name: String,
    /// Canonical members of a string enum, in declaration order.
    pub enum_members: Vec<(String, String)>,
    /// A closed frozen layout whose generated constructor only stores its fields.
    pub frozen_initializer: Option<usize>,
    pub value_struct: bool,
    pub dataclass_eq: bool,
    pub dataclass_repr: bool,
    pub toml_hash: bool,
    /// Field index for each parameter of a generated, field-only initializer.
    pub initializer_fields: Option<Vec<usize>>,
    pub fields: Vec<ClassFieldMetadata>,
}

/// A helper appends one element to its private result for each item of the
/// iterable parameter. The element stays in the checked append expression.
pub(crate) struct ListBuild {
    pub iterable: usize,
    pub item: usize,
    pub result: usize,
}

pub(crate) struct ClassFieldMetadata {
    pub name: String,
    pub compare: bool,
    pub repr: bool,
}

pub(crate) struct JsonRecordSchema {
    pub class: usize,
    /// Declaration order is also the exact native record field order.
    pub fields: Vec<JsonField>,
}

pub(crate) struct JsonField {
    pub name: String,
    /// Absence requires a field; defaults apply only when its key is missing.
    pub default: Option<JsonDefault>,
}

pub(crate) enum JsonDefault {
    Int(String),
    Float(f64),
    Bool(bool),
    Str(String),
    None,
    EmptyList,
}

impl Module {
    /// Release the rooted namespace bindings at shutdown if this program uses them.
    pub(crate) fn uses_builtins_last(&self) -> bool {
        let mut used = false;
        for function in self.functions.iter().chain([&self.entry]) {
            walk_expressions(&function.body, &mut |value| {
                used |= matches!(
                    value.kind,
                    ExprKind::Builtin(
                        Builtin::BuiltinsSetLast
                            | Builtin::SysDisplayHook
                            | Builtin::BuiltinsLen
                            | Builtin::BuiltinsSetLen,
                        _
                    )
                );
            });
        }
        used
    }

    /// Capture arguments, the original invocation, and source search paths only
    /// for modules that use them; a bare import needs no runtime state.
    pub(crate) fn startup_usage(&self) -> (bool, bool, bool, bool) {
        let mut linecache = false;
        let mut cache_paths = false;
        let mut argv = false;
        let mut original = false;
        for function in self.functions.iter().chain([&self.entry]) {
            walk_expressions(&function.body, &mut |value| match &value.kind {
                ExprKind::Builtin(Builtin::SysArgv, _) => argv = true,
                ExprKind::Builtin(Builtin::SysOrigArgv, _) => original = true,
                ExprKind::Builtin(Builtin::Linecache(..), _) => linecache = true,
                ExprKind::Builtin(Builtin::CachePath(_) | Builtin::ModuleCacheFromOrigin, _) => {
                    cache_paths = true
                }
                ExprKind::Builtin(Builtin::WarningFormat { .. }, args)
                    if args[5].ty != Type::Str =>
                {
                    linecache = true
                }
                _ => {}
            });
        }
        (argv || original, original, linecache, cache_paths)
    }
}

#[derive(Clone)]
pub(crate) struct Function {
    pub params: Vec<usize>,
    pub returns: Type,
    pub locals: Vec<Type>,
    pub body: Vec<Statement>,
}

impl Function {
    /// These slots can change while an earlier operand is still being used.
    /// Comprehension targets stay private; only explicit binding expressions
    /// write into the enclosing source scope.
    pub(crate) fn expression_rebound(&self) -> Vec<bool> {
        let mut rebound = vec![false; self.locals.len()];
        walk_expressions(&self.body, &mut |value| {
            if let ExprKind::Bind(slot, _) = value.kind {
                rebound[slot] = true;
            }
        });
        rebound
    }
}

#[derive(Clone)]
pub(crate) struct Expr {
    pub ty: Type,
    pub kind: ExprKind,
}

impl Expr {
    /// Arguments of an explicit right split with a single split and final-field access.
    pub(crate) fn last_rsplit_args(&self, index: &Self) -> Option<&[Expr]> {
        let ExprKind::Builtin(Builtin::Text(TextOperation::Split(true)), args) = &self.kind else {
            return None;
        };
        let [_, separator, limit] = args.as_slice() else {
            return None;
        };
        (matches!(separator.kind, ExprKind::Present(_))
            && matches!(&limit.kind, ExprKind::Int(n) if n == "1")
            && matches!(&index.kind, ExprKind::Negate(one) if matches!(&one.kind, ExprKind::Int(n) if n == "1")))
            .then_some(args)
    }
}

/// Values are evaluated and rooted in source order. `order` is a permutation of
/// their indices that supplies the native call's parameter order without
/// reevaluation. Methods include their receiver first; constructors omit the
/// implicit receiver, which is allocated only after these values are evaluated.
#[derive(Clone)]
pub(crate) struct CallArgs {
    /// The callee's nominal tag feeds a complete member dispatch at this site.
    pub exhaustive_enum_match: bool,
    pub values: Vec<Expr>,
    pub order: Vec<usize>,
}

impl CallArgs {
    /// Uses the existing positional order for calls that need no argument binding.
    pub(crate) fn positional(values: Vec<Expr>) -> Self {
        Self {
            exhaustive_enum_match: false,
            order: (0..values.len()).collect(),
            values,
        }
    }
}

#[derive(Clone)]
pub(crate) enum ExprKind {
    Int(String),
    Float(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    None,
    Local(usize),
    /// Stores the value, promoting its storage type if needed, and yields the
    /// original value with an independent owned reference.
    Bind(usize, Box<Expr>),
    /// An eager comprehension with writes to its enclosing source scope.
    /// Private slots are cleared on success and on every escaping error.
    Scope {
        body: Vec<Statement>,
        value: Box<Expr>,
        clear: Vec<usize>,
    },
    List(Vec<Expr>),
    Dict {
        key: Type,
        value: Type,
        entries: Vec<(Expr, Expr)>,
    },
    Set {
        element: Type,
        values: Vec<Expr>,
    },
    Tuple(Vec<Expr>),
    Present(Box<Expr>),
    Absent(Box<Expr>),
    Unwrap(Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Compare(Vec<Expr>, Vec<Compare>),
    Boolean(bool, Vec<Expr>),
    /// Returns the short-circuit operand, narrowed by its selected truth value,
    /// or the RHS. The operands are evaluated once and retain their identities.
    BooleanValue(bool, Box<Expr>, Box<Expr>),
    /// Only the selected arm runs; both arms have the expression's storage type.
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Call(usize, CallArgs),
    /// Captures a fixed target and, for a bound method, one owned receiver.
    Callable {
        function: usize,
        receiver: Option<Box<Expr>>,
    },
    /// Evaluates and roots the callable before its source-ordered arguments.
    Invoke {
        callable: Box<Expr>,
        args: CallArgs,
    },
    Construct {
        initializer: usize,
        args: CallArgs,
    },
    Exception {
        kind: u32,
        /// None distinguishes a zero-argument exception from an empty string.
        message: Option<Box<Expr>>,
    },
    Field(Box<Expr>, usize),
    Builtin(Builtin, Vec<Expr>),
}

#[derive(Clone, Copy)]
pub(crate) enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    LeftShift,
    RightShift,
    Repeat,
    Concat,
}

#[derive(Clone, Copy)]
pub(crate) enum Compare {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

/// Native multiset arithmetic over integer counts. Values match the runtime ABI.
#[repr(u32)]
#[derive(Clone, Copy)]
pub(crate) enum CounterOp {
    Add,
    Subtract,
    Union,
    Intersection,
    Positive,
    Negative,
}

/// Public ZIP entry metadata read from the central directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZipField {
    Name,
    Size,
    CompressedSize,
    Crc,
    Method,
    ExternalAttr,
    IsDir,
}

/// The immutable attribute lists exported by functools and used as decorator defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WrapperAttributes {
    Assigned,
    Updated,
}

impl WrapperAttributes {
    pub(crate) fn names(self) -> &'static [&'static str] {
        match self {
            Self::Assigned => &[
                "__module__",
                "__name__",
                "__qualname__",
                "__doc__",
                "__annotate__",
                "__type_params__",
            ],
            Self::Updated => &["__dict__"],
        }
    }

    pub(crate) fn root(self) -> &'static str {
        match self {
            Self::Assigned => "functools_wrapper_assignments",
            Self::Updated => "functools_wrapper_updates",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub(crate) enum ArgparseOutput {
    Return,
    Stdout,
    Stderr,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CachePathOperation {
    Get(bool),
    Set(bool),
    Tag,
    Forward,
    Reverse,
    Debug,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum LinecacheOperation {
    Getline,
    Getlines,
    Update,
    Clear,
    Check,
    Lazy,
    Cache,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RandomOperation {
    Seed,
    Float,
    Bits,
    ChoiceIndex,
    Randint,
    Randrange,
    RandrangeOne,
    ChoiceRange,
}

#[derive(Clone)]
pub(crate) enum Builtin {
    Random(RandomOperation),
    Linecache(LinecacheOperation, Type, bool),
    Repeat,
    WrapperAttributes(WrapperAttributes),
    CursorLengthHint,
    Progression {
        bounded: bool,
    },
    Islice,
    Tee,
    TupleIndex,
    TupleBufferNew,
    TupleBufferPush,
    TupleBufferFinish,
    Cycle,
    SysDisplayHook,
    BuiltinsLast,
    BuiltinsLen,
    BuiltinsSetLen,
    BuiltinsHasLast,
    BuiltinsSetLast,
    BuiltinsDeleteLast,
    DirectoryContextView,
    TextStreamView,
    Document(DocumentOp),
    /// Runs the static archive's module initializer in ordinary import order.
    ExtensionImport(String),
    /// Calls a checked declaration in a rebuilt extension's method table.
    ExtensionCall {
        module: String,
        method: String,
        kinds: Vec<u8>,
        result_kind: u8,
    },
    StrEnumMember {
        class: usize,
        member: usize,
    },
    StrEnumName,
    ClassNameView,
    ClassNamePart {
        module: bool,
    },
    NewForwardRef,
    ForwardRefField(usize),
    BoxAnnotation(usize),
    AnnotationMember(usize),
    AnnotationRef,
    AnnotationIs,
    StaticFunction(usize),
    FunctionWrappedSet,
    FunctionWrappedGet,
    StrEnumValue,
    EnumLookup(usize),
    EnumString,
    NominalIdentity,
    /// Intern a source module by its import name, retaining its source filename.
    SourceModule,
    /// Read __name__ or __file__ from the evaluated module.
    SourceModuleAttribute {
        filename: bool,
    },
    /// Match an exception against the category list retained by an assertion context.
    ExceptionMatchesAny,
    LiteralStringView,
    IntListContains,
    StringListContains,
    StructListContains,
    StringContains,
    /// Immutable byte subsequence; operands are evaluated needle first.
    BytesContains,
    /// Integer byte membership validates 0..=255 before searching.
    BytesContainsInt,
    /// Reads an initialized module binding with an independent owned reference.
    GlobalGet(usize),
    /// Publishes one fixed module binding after its initializer succeeds.
    GlobalSet(usize),
    Print,
    StderrLine,
    SysStreamWrite {
        stderr: bool,
    },
    SysStreamFlush {
        stderr: bool,
    },
    SysPlatform,
    SysThreadInfo,
    SysThreadText {
        version: bool,
    },
    SysVersionInfo {
        implementation: bool,
    },
    SysIntern,
    SysIsInterned,
    OsName,
    DocCaptureStart,
    DocCaptureTake,
    AsciiEscape,
    MockCallList {
        mark: bool,
    },
    SystemExitNew {
        omit_none: bool,
    },
    SystemExitCode,
    SystemExitCodeEqual {
        reversed: bool,
    },
    ExceptionFieldIdentity {
        reversed: bool,
    },
    /// Evaluates and roots all arguments, then raises the last (an exception).
    RaiseException,
    WallTime,
    TimeSleep,
    WarningFilter {
        category: u32,
        invalid: u32,
    },
    WarningReset,
    WarningFormat {
        entry: Type,
        environment: bool,
    },
    WarningWrite {
        stdout: bool,
    },
    PercentFormat,
    Logging(LogOperation),
    Timed(TimedOperation),
    Calendar(CalendarOp),
    LogRecord(LogRecordOperation),
    Formatter(FormatterOperation),
    Input,
    Chr,
    Ord,
    IntegerPower,
    ModularPower,
    NegativeIntegerPower,
    Int,
    Float,
    /// Represents an exact integer by its sign for a proved zero-comparison parameter.
    IntSignAsFloat,
    Bool,
    Str,
    /// Wrap a fixed record after constructor/post-init, preserving exception identity.
    NamedException {
        kind: u32,
        message_field: usize,
        tuple_args: bool,
        text: Option<usize>,
        repr: Option<usize>,
        dataclass_repr: bool,
    },
    /// Borrowing a payload from its exception returns one owned record reference.
    ExceptionRecord,
    /// A single non-string Python argument, retained for str/repr.
    ExceptionArgument(u32),
    ImportErrorNew {
        kind: u32,
        fields: [Option<usize>; 3],
    },
    ImportErrorField(u32),
    ImportErrorReset,
    ImportErrorRejectReset,
    BoxValue,
    ObjectRecordProject(usize),
    ObjectRecordIdentity {
        class: usize,
        reversed: bool,
    },
    MappingObjectGet,
    ExceptionTupleView,
    ExceptionKeywordSuggestion,
    /// A new, completely initialized record for a proven forwarding constructor.
    ExceptionRecordFields,
    /// Check the return type of __str__ or __repr__ before crossing the formatter ABI.
    ExceptionFormatterResult(bool),
    ExceptionArgs,
    ExceptionTupleIdentity,
    SystemExitFromTuple,
    ExceptionFieldRepr,
    ExceptionFieldLen,
    ExceptionFieldIndex,
    ExceptionRepr,
    /// Python representation of a value with a fully known storage shape.
    Repr,
    Len,
    LengthResult,
    Abs,
    /// Stable permutation of a fully typed list by precomputed scalar keys.
    SortByKeys,
    ListExtendUnhinted,
    SortDetach,
    SortRestore,
    StringJoin,
    JoinBufferNew,
    JoinBufferAppend,
    JoinBufferFinish,
    StringFormat,
    StringReplace,
    Text(TextOperation),
    HexFormat {
        width: u32,
        zero: bool,
        upper: bool,
    },
    FloatFormat {
        precision: u32,
        percent: bool,
        positive_sign: u8,
    },
    TupleGet,
    ListGet,
    /// A live-length guard proves a private cursor or constant index is in
    /// bounds, with no intervening mutation or user code. Returns an owned
    /// element of a generic typed list; the index is canonical and nonnegative.
    SequenceGetInBounds,
    /// Transfers the next slot of a fresh append-only list to its sole consumer.
    SequenceTakeInBounds,
    /// Reduces an exact integer list after evaluating its source and start.
    /// Integer addition cannot invoke callbacks or raise a recoverable error.
    ListSum,
    /// Sums a private integer slice at the slice's original evaluation point.
    /// Nullable bounds and zero-step errors match ordinary slicing.
    ListSliceSum,
    /// Internal loop guard over [list, start, stop, step]. Scanning is limited
    /// to four elements per planned iteration; true proves every value fits i32.
    ListAllI32,
    /// Best-effort bounded capacity hint for one empty loop output list.
    ListReserveRange,
    /// The guarded loop proves all elements fit i32 and cannot be replaced.
    ListGetI32,
    /// Every call site and intervening operation preserve these exact element
    /// bounds. The access still checks its index and returns a tagged-small Int.
    ListGetBounded {
        min: i64,
        max: i64,
    },
    /// A canonical reduction loop proves its direct source/cursor access is
    /// in bounds before advancing. The rooted list cannot change during the
    /// loop; the borrowed result also has these exact tagged-small bounds.
    ListGetBoundedInBounds {
        min: i64,
        max: i64,
    },
    /// The left operand of a guarded loop's only sum update. Its local starts
    /// at zero and stays within these bounds before each update.
    IntBounded {
        min: i64,
        max: i64,
    },
    /// Internal identity guard between a read source and a mutated list.
    ListDistinct,
    StringGet,
    StringIter,
    /// Iterates Python line boundaries on a rooted immutable string.
    SplitLinesIter,
    /// Linear Unicode iteration from the final code point toward the first.
    StringReverseIter,
    /// Private, initially zeroed tuple storage for iterator result caching.
    TupleCacheNew,
    /// Reuses an exclusively owned iterator cache or allocates a separate result.
    /// Its argument is a private stable local, borrowed without a temporary owner.
    TupleCacheResult,
    /// Eagerly converts immutable string code points into an integer list,
    /// preserving `list(map(int, text))` errors without character objects.
    StringIntList,
    Bytes,
    BytesGet,
    BytesIter,
    Encode,
    Decode,
    DecodeAscii,
    UrlsafeB64encode,
    CodecLookup,
    CodecName,
    CodecEncode,
    CodecDecode,
    Socket(SocketOp),
    CodecWriter(WriterOperation),
    Sha256Hex,
    Sha256New,
    HashNew,
    NewPlainObject,
    Typing(TypingOperation),
    StrDescriptor(StrDescriptor),
    InspectSentinel,
    HashAvailable,
    Sha256Update,
    Sha256Digest {
        hex: bool,
    },
    Sha256Copy,
    BytesIo,
    BytesIoValue,
    StringIo(StringIoOperation),
    ContextVar(ContextOperation),
    BinarySeek,
    BinaryTell,
    ZipOpenFile,
    ZipGetInfo,
    ZipInfoList,
    ZipMember,
    ZipRead,
    ZipTest,
    ZipFilename,
    ZipMinReadSize,
    ZipField(ZipField),
    ZipOpen,
    ZipNamelist,
    ZipClose,
    /// Validates complete UTF-8 at the decode evaluation point and returns a
    /// retained bytes identity for an internal allocation-free character gather.
    BytesValidateUtf8,
    /// Receiver followed by three nullable integer words for start, stop, step.
    Slice,
    Append,
    Pop,
    ListClear,
    ListReverse,
    IsNone,
    /// Promotes an exact str/bytes owner without allocating or changing identity.
    UnionInject,
    /// An owned list for a readonly iteration: borrows existing list storage or copies tuple words.
    StringIterationList,
    /// Projects a member proved by local control flow, acquiring its own owner.
    UnionProject,
    IsInstance(Vec<InstanceType>),
    IsBuiltinSubclass,
    Split,
    SplitLines,
    /// Only the first line and a joined remainder can be observed.
    JoinedSplitLines,
    DecodeSplitLines,
    Ndiff,
    /// Copy an exact list of immutable strings into a distinct mutable list.
    StringListCopy,
    /// Select by up to two prefixes, optionally exclude one, then slice each match.
    StringListFilter(u32),
    /// Stable sort by the suffix after a fixed number of Unicode code points.
    StringListSortSuffix(u32),
    /// Ask whether a source has an item before evaluating a comprehension predicate.
    StringListNonempty,
    RStrip,
    SysArgv,
    OptionalStrConcat,
    SysOrigArgv,
    SysGetFrameModuleName,
    SysGetRecursionLimit,
    SysSetRecursionLimit,
    /// The child layout descriptors follow the literal subcommand order.
    ParseArgs {
        namespace: usize,
        children: Vec<usize>,
        process_argv: bool,
    },
    /// Format parser text or print it to the requested stream.
    ArgparseText {
        usage: bool,
        output: ArgparseOutput,
    },
    /// Retags a namespace after its immutable command field selects a child layout.
    RecordView,
    FormatCursor {
        split: bool,
    },
    ExactRecord(usize),
    CsvReader {
        delimiter: char,
        quote: Option<char>,
        escape: Option<char>,
        flags: u32,
    },
    CsvNext {
        required: bool,
    },
    CsvLineNumber,
    /// Decodes one string into a checked record, list, or string-key dictionary.
    JsonDecode(Type),
    JsonLoads,
    JsonDecoderNew,
    JsonDecoderMemo,
    JsonDecoderDecode,
    JsonEmptyObject,
    ModuleCache(usize),
    /// Capture and access an individual wrapper's independently owned cache.
    CacheNew,
    CacheTable,
    CacheProbe(i64),
    CacheInsert(i64),
    CacheStat(u32),
    CacheClear,
    /// Read the public maxsize (false) or typed (true) of a constructed cache.
    CacheParameter(bool),
    CallableContext,
    HashStr,
    HashRecord,
    /// A dictionary with a read-only JSON receiver and a literal string key.
    JsonGet {
        key: String,
        empty_list: bool,
    },
    JsonAdd,
    /// Compare two parsed values, observing mutations to an original root list.
    JsonEqual,
    /// Both lookups borrow the same local JSON document and use literal integer defaults.
    JsonAddFields(String, String),
    JsonIsInt,
    /// Bind the JSON object method before an effectful default is evaluated.
    JsonGetReceiver,
    JsonIndex,
    JsonIter,
    JsonLen,
    /// A literal-key path rooted in a readonly JSON owner. The result type selects
    /// an owned view, an exact scalar, or comparison with a supplied string literal.
    JsonSelect(Vec<String>),
    /// Decodes a complete TOML table into a checked record or string-key dictionary.
    TomlDecode(Type),
    TomlKeys,
    /// Encodes rooted concrete storage using immutable, compiler-emitted metadata.
    JsonEncode(Type),
    JsonDumps(Type),
    JsonDump(Type),
    Path(PathOperation),
    PosixPath(PosixPathOperation),
    Errno(ErrnoOperation),
    Fnmatch(FnmatchOperation),
    Patch(PatchOperation),
    Process(ProcessOperation),
    /// Zero reads, one truncates/writes; the frontend accepts literal modes only.
    TextOpen(u32),
    BinaryOpen(u32),
    GzipOpen,
    /// The compile-time validated pattern is independent of the source spelling.
    RegexCompile {
        source: String,
        spelling: String,
    },
    RegexSpelling,
    UnittestCompilePattern,
    RegexSearch,
    RegexSub(Vec<u8>),
    RegexGroupDict,
    RegexFindIter,
    RegexMatchStart,
    /// A proved selector is a valid numeric index for every possible receiver.
    RegexCapture {
        operation: RegexCapture,
        known: bool,
    },
    BinaryRead,
    BinaryReadSize,
    FileReadline,
    BinaryWrite,
    TextEnter,
    TextRead,
    TextWrite,
    TextFlush,
    TextClose,
    TextClosed,
    FileFileno,
    DictGet,
    DictCopy,
    DictFromKeysTarget,
    ModuleParentPrefix,
    ModuleCacheFromOrigin,
    CachePath(CachePathOperation),
    OrderedDict,
    NamespaceWrap,
    NamespaceDict,
    NamespaceGet,
    NamespaceDelete,
    DeepCopy,
    RecordCopy,
    /// Creates a dictionary whose missing subscription stores a fresh typed list.
    DefaultListDict,
    DefaultDictMissing,
    DefaultFactoryEnabled,
    DefaultFactorySet {
        enabled: bool,
    },
    DequeNew,
    DequeReset,
    DequeAppend {
        left: bool,
        extending: bool,
    },
    DequeExtendSource,
    DequeCopy,
    DequePop {
        left: bool,
    },
    DequeClear,
    DequeReverse,
    DequeGet,
    DequeSet,
    DequeDelete,
    DequeContains,
    DequeMaxlen,
    DequeIter {
        reverse: bool,
    },
    CounterNew,
    ChainMapNew,
    ChainMapMaps,
    ChainMapSetMaps,
    ChainMapGetItem,
    ChainMapSetItem,
    ChainMapContains,
    /// The stored type may be promoted to an optional result on a hit.
    ChainMapGet(Type),
    CounterElements,
    CounterTotal,
    CounterMostCommon,
    CounterUpdate,
    CounterMath {
        op: CounterOp,
        inplace: bool,
    },
    DictSumValues,
    /// Widens a defaultdict owner to its ordinary dictionary interface.
    DictAsBase,
    /// Combines the current keys of rooted mappings/sets after both operands run.
    KeyUnion,
    KeySet,
    KeyIntersection,
    KeyDifference,
    /// Produces sorted unique string keys from a small union of local dictionaries.
    SortedStringKeys,
    /// Each generated field reads the sorted key (0) or one input dictionary (1-based).
    SortedRecordRows {
        class: usize,
        columns: Vec<u8>,
    },
    /// Maps each Unicode scalar through an exact string-to-integer dictionary.
    /// The text is evaluated first; the mapping remains mutable between calls.
    DictGatherStrInt,
    /// Reads immutable bytes already proved valid by BytesValidateUtf8.
    DictGatherBytesInt,
    DictGetDefault,
    DictSetDefault,
    /// The stored value type determines optional promotion on successful lookup.
    /// A third argument, when present, already has the flattened result type.
    DictTryGet(Type),
    /// Removes a key or raises KeyError; an optional third operand is an exact default.
    DictPop,
    /// Transfers the removed value into an optional result, or returns the supplied default.
    DictTryPop(Type),
    /// Arguments are key then dictionary, matching membership evaluation order.
    DictContains,
    DictDelete,
    DictClear,
    DictUpdate,
    /// False visits keys; true visits key/value pairs without an intermediate tuple.
    DictIter(bool),
    /// Arguments are the element then set, matching membership evaluation order.
    SetContains,
    SetAdd,
    SetDiscard,
    SetIter,
    FreezeStrings,
    ExceptionMessage,
    JsonErrorField(u32),
    /// Attaches an explicit cause to a fresh, unescaped builtin exception.
    ExceptionWithCause,
    ExceptionCause,
    ActiveException,
    ExceptionSuppressContext,
}

impl Builtin {
    /// Position of a read-only key that can borrow a stack tuple for the call.
    pub(crate) fn tuple_probe_index(&self) -> Option<usize> {
        match self {
            Self::DictGetDefault | Self::DictTryGet(_) => Some(1),
            Self::DictContains | Self::SetContains => Some(0),
            _ => None,
        }
    }
}

pub(crate) const TUPLE_PROBE_ARITY: std::ops::RangeInclusive<usize> = 1..=16;

/// Reads from the active context or an explicit shallow snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextOperation {
    New,
    CopyCurrent,
    Copy,
    Var,
    Set,
    Reset,
    ExitToken,
    Read,
    Contains,
    ContextRead,
    ContextContains,
    Name,
    TokenVar,
    Old,
    Missing,
    WasMissing,
    Enter,
    Leave,
    Len,
}

/// Operations specific to Unicode text held in native memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StringIoOperation {
    New,
    Newlines,
    Capability,
    Readlines,
    Value,
    Read,
    Seek,
    Position,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum DocumentShape {
    Any,
    /// A tagged Python scalar whose container equality does not depend on object identity.
    Scalar,
    Map,
    List,
}

/// The opcode also selects the exact representation for each native argument and result.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum DocumentOp {
    Null,
    EmptyMap,
    EmptyList,
    FromStr,
    FromBool,
    FromInt,
    FromFloat,
    FromJson,
    WrapList,
    WrapMap,
    MapView,
    ListView,
    AsStr,
    RequireMap,
    RequireList,
    IsNull,
    IsMap,
    IsList,
    IsStr,
    Repr,
    Truth,
    ParseToml,
    DumpToml,
    UpdateMap,
    PopDefault,
    ReplaceItems,
    SetItem,
    WriteToml,
    BoxNullMap,
    FreezeMap,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RegexCapture {
    Group,
    Absent,
    Start,
    End,
}

/// These descriptor values call the same native operation as their bound string methods.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u32)]
pub(crate) enum StrDescriptor {
    Lower,
    Strip,
    LStrip,
    RStrip,
    IsAlnum,
}
impl StrDescriptor {
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "lower" => Self::Lower,
            "strip" => Self::Strip,
            "lstrip" => Self::LStrip,
            "rstrip" => Self::RStrip,
            "isalnum" => Self::IsAlnum,
            _ => return None,
        })
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Strip => "strip",
            Self::LStrip => "lstrip",
            Self::RStrip => "rstrip",
            Self::IsAlnum => "isalnum",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PatchKind {
    Set,
    File,
    Hunk,
}

#[derive(Clone, Copy)]
pub(crate) enum PatchOperation {
    Parse,
    Len,
    Files,
    ModifiedFiles,
    ModifiedCount,
    Hunks,
    Path,
    SourceStart,
    HunkText,
}

/// Closed string operations preserve exact argument and storage types.
#[derive(Clone, Copy)]
pub(crate) enum TextOperation {
    Predicate(TextPredicate),
    SplitLineEnds,
    ExpandTabs,
    Lower,
    Upper,
    Capitalize,
    /// True splits from the right.
    Split(bool),
    /// 0 strips both ends, 1 the leading end, and 2 the trailing end.
    Strip(u32),
    StartsWith,
    EndsWith,
    Find,
    Partition,
    Translate,
    Unquote,
    Dedent,
    ShlexSplit,
    ShlexQuote,
}

#[derive(Clone, Copy)]
pub(crate) enum TextPredicate {
    Alnum,
    Space,
    Upper,
}

/// Numeric selectors for the runtime's imre_path_query ABI.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum PathQuery {
    Exists = 0,
    Directory = 1,
    File = 2,
    Symlink = 3,
    Fifo = 4,
    Socket = 5,
    BlockDevice = 6,
    CharDevice = 7,
}

/// Operations on a link's stored target name and its destination (POSIX link ABI).
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum PathLink {
    Symlink = 0,
    CheckSymlink = 1,
    Hardlink = 2,
}

/// Native path operations keep their exact result types in the checked IR.
#[derive(Clone, Copy)]
pub(crate) enum PathOperation {
    Scripts,
    OsExists,
    New,
    PureNew(PathFlavor),
    JoinObject,
    Drive,
    Root,
    Anchor,
    CountPart,
    AsPosix,
    Join,
    Match {
        full: bool,
    },
    WithName,
    WithStem,
    WithSuffix,
    Suffixes,
    IsRelativeTo,
    RelativeTo,
    Text,
    Name,
    Stem,
    Suffix,
    Parent,
    IsAbsolute,
    Absolute,
    Resolve,
    Info,
    InfoRepr,
    CheckStatPath,
    Stat {
        lstat: bool,
    },
    StatTime(u32),
    StatExtraInt(u32),
    Query(PathQuery),
    ReadText,
    ReadBytes,
    Readlink,
    Link(PathLink),
    WriteText,
    WriteBytes,
    WriteTypeError,
    WriteUnion {
        binary: bool,
    },
    Mkdir,
    Unlink,
    Rmdir,
    Replace,
    /// Scoped creation returns a hidden owned pathname, not an escaping manager.
    TemporaryDirectory,
    CleanupTemporaryDirectory,
    ContextNew {
        temporary: bool,
    },
    ContextEnter,
    ContextExit,
    UnionText,
    Iterdir,
    Walk,
    Glob {
        recursive: bool,
    },
}

/// Shell filename operations do not interpret path separators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FnmatchOperation {
    Match { normcase: bool },
    Filter { inverse: bool },
    Translate,
    Parts,
}

/// POSIX lexical operations preserve raw str or bytes spellings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PosixPathOperation {
    Join,
    MixedJoin { path_in_tail: bool },
    Commonpath,
    Basename,
    Dirname,
    Splitext,
    Normpath,
    Abspath,
}

/// Operations on a timed file handler and its immutable wall-clock time.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimedOperation {
    New,
    Clock,
    ClockPart,
    Close,
    Flush,
    Stream,
    Filename,
    Interval,
    RolloverAt,
    When,
    Suffix,
    BackupCount,
    Utc,
    Delay,
    AtTime,
    ComputeInt,
    ComputeFloat,
    FilesToDelete,
    ShouldRollover,
    DoRollover,
}

/// Public formatting operations on a retained style, defaults mapping, and clock converter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatterOperation {
    New,
    Text,
    Date,
    UsesTime,
    Format,
    Message,
    Time,
    SetUtc,
    SetDate,
    DefaultTime,
    DefaultMsec,
    SetDefaultTime,
    SetDefaultMsec,
    Stack,
}

/// Operations on a record with individually owned, named Python fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogRecordOperation {
    New,
    Put,
    Update,
    Read,
    Message,
    Repr,
    Flag,
    SetFlag,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterOperation {
    Class,
    New,
    Write,
    Flush,
    Close,
    Reset,
    Encoding,
    SetEncoding,
    Stream,
    Errors,
    SetErrors,
}

/// Operations on the native logging registry and its named loggers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogOperation {
    Get,
    HandlerRename,
    LockNew,
    LockAcquire,
    LockAcquireInt,
    LockRelease,
    LockCount,
    LockOwned,
    LockLocked,
    LockEnter,
    LockExit,
    LockReset,
    HandlerLookup,
    PrintActiveException,
    AddLevel,
    LevelName,
    LevelForName,
    CheckLevel,
    InvalidLevel,
    Disable,
    EnabledCache,
    Names,
    Numbers,
    NamesCopy,
    Configure,
    ConfigureName,
    DictConfig,
    DictConfigTyped,
    FileConfig,
    Disabled,
    Propagate,
    SetPropagate,
    HasHandlers,
    Name,
    Level,
    Root,
    Parent,
    Children,
    EffectiveLevel,
    SetLevel,
    Enabled,
    Emit,
}

/// Kernel readiness for exact integer descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Readiness {
    New,
    Register,
    Modify,
    Unregister,
    Poll,
    Select,
}

/// POSIX descriptor calls use the target's C int and file descriptor flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FdOperation {
    Open,
    Unlink,
    Remove,
    Seek,
    Pipe,
    Pipe2,
    Dup2,
    Close,
    Dup,
    Isatty,
    GetBlocking,
    SetBlocking,
    GetInheritable,
    SetInheritable,
}

/// Filesystem copy operations using the same ordered pair of path operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum CopyOperation {
    File = 0,
    Mode = 1,
    Stat = 2,
    Xattr = 3,
    Copy = 4,
    Copy2 = 5,
}

/// Synchronous process operations cannot invoke language callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrnoOperation {
    DefaultLookup,
    Module,
    Namespace,
    Keys,
    Errorcode,
    Number,
    Text,
    Lookup,
    Has,
    Items,
    MemberContains,
}

#[derive(Clone, Copy)]
pub(crate) enum ProcessOperation {
    SelectorClass(Option<bool>),
    SetSelector,
    Executable,
    RunToStderr,
    ShellOutput {
        status: bool,
    },
    CheckOutput {
        streams: u32,
        validation: u32,
        errors: u8,
        capture: ProcessCapture,
    },
    Run {
        streams: u32,
        validation: u32,
        errors: u8,
        timed: bool,
    },
    Popen {
        streams: u32,
    },
    Call {
        streams: u32,
        check: bool,
    },
    PopenPid,
    PopenActive,
    PopenCleanup,
    PopenIdentity,
    PopenArgs,
    PopenPipe(u8),
    PopenStatus {
        poll: bool,
    },
    PopenSetStatus,
    PopenSetPoll {
        returns_none: bool,
    },
    PopenDeletePoll,
    PopenWait,
    PopenCommunicate {
        text: bool,
    },
    PopenExit,
    PopenSignal,
    AsyncSpawn {
        stdout: u8,
        stderr: u8,
    },
    AsyncStatus {
        wait: bool,
    },
    AsyncCommunicate,
    Coroutine {
        function: usize,
        result: Type,
    },
    ContextNew {
        function: usize,
    },
    ContextEnter,
    ContextYield,
    GeneratorNext,
    GeneratorTake,
    GeneratorYield,
    ContextExit,
    ContextThrow,
    IndirectCoroutine {
        signature: Type,
        result: Type,
    },
    Await,
    RunLoop,
    NewLoop,
    SetLoop,
    EnsureTask,
    RunUntilComplete,
    LoopSignal,
    CloseLoop,
    CancelTask,
    TaskGroup,
    CreateTask,
    TaskResult,
    EnterGroup,
    JoinGroup,
    AbortGroup,
    ExceptionAt,
    MemberCause,
    Gather {
        element: Type,
        tagged: bool,
    },
    Semaphore,
    Acquire,
    Release,
    EnvironCopy,
    EnvironGet,
    GetCwd,
    Fd(FdOperation),
    Credential(&'static str),
    CredentialId {
        group: bool,
    },
    Readiness(Readiness),
    OsErrorField(u32),
    Strerror,
    QuoteCommandArgument,
    DescriptorConstant(&'static str),
    Which,
    RmTree,
    Copy(CopyOperation),
    CopyTree(Type),
    Move(Type),
    DestInSrc,
    IgnorePatterns,
    TreeErrorArgs,
    MakeArchive,
    UnpackArchive,
    ArchiveFormats {
        unpack: bool,
        row: Type,
    },
    UnregisterArchive(bool),
    /// The module and statement identify a single source-level function object.
    SetArchiveRoot((usize, usize)),
    RegisterArchive {
        unpack: bool,
        frame: Option<Type>,
        keyword_value: Option<Type>,
        variadic_value: Option<Type>,
        root_flag: Option<(usize, usize)>,
    },
    CheckReturnCode,
    /// Reinterprets the same owned exception after a category proof or widening.
    ExceptionView,
    /// Fixed guarded exception fields; output has its own alias index.
    ErrorField(u32),
    /// Exact TimeoutExpired fields use a distinct bytes-safe projection.
    TimeoutField(u32),
}

/// Binary pipe handles have one static type but may own either a raw or a
/// buffered stream. The Python class still distinguishes them after close.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IoBase {
    Any,
    Raw,
    Buffered,
    Text,
}

impl IoBase {
    pub(crate) fn matches(self, ty: Type) -> Option<bool> {
        match (self, ty) {
            (Self::Raw | Self::Buffered, Type::BinaryIO) => None,
            (Self::Any, Type::TextIO | Type::StringIO | Type::BinaryIO)
            | (Self::Text, Type::TextIO | Type::StringIO) => Some(true),
            _ => Some(false),
        }
    }
}

/// Sealed Python runtime categories; parameterized container annotations are
/// never runtime classes, and exception categories use their existing hierarchy.
#[derive(Clone, Copy)]
pub(crate) enum InstanceType {
    Exact(Type),
    ContextVar,
    ContextToken,
    IoBase(IoBase),
    ForwardRef,
    SimpleNamespace,
    ModuleSpec,
    StrEnum,
    List,
    Dict,
    Set,
    Tuple,
    Exception(u32),
}

impl InstanceType {
    pub(crate) fn accepts(self, ty: Type) -> bool {
        match self {
            Self::IoBase(base) => base.matches(ty) != Some(false),
            Self::Exact(Type::CompletedProcess(..)) => matches!(ty, Type::CompletedProcess(..)),
            Self::ContextVar => matches!(ty, Type::ContextVar(_)),
            Self::ContextToken => matches!(ty, Type::ContextToken(_)),
            Self::Exact(Type::Int) => matches!(ty, Type::Int | Type::Bool),
            Self::Exact(Type::Str) => {
                matches!(ty, Type::Str | Type::StrLiteral(_) | Type::StrEnum(_))
            }
            Self::Exact(expected) => expected == ty,
            Self::ForwardRef => matches!(ty, Type::ForwardRef(_)),
            Self::StrEnum => matches!(ty, Type::StrEnum(_)),
            Self::List => matches!(ty, Type::ListInt | Type::List(_)),
            Self::Dict => matches!(ty, Type::Dict(_)),
            Self::Set => matches!(ty, Type::Set(_)),
            Self::Tuple => matches!(ty, Type::Tuple(_) | Type::CodecInfo),
            Self::SimpleNamespace => matches!(ty, Type::SimpleNamespace(_)),
            Self::ModuleSpec => matches!(ty, Type::ModuleSpec(_)),
            Self::Exception(_) => matches!(
                ty,
                Type::Exception
                    | Type::ImportException
                    | Type::TimeoutExpired
                    | Type::NamedException(_)
            ),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ExceptHandler {
    /// Empty for a bare handler; otherwise fixed builtin exception categories.
    pub kinds: Vec<u32>,
    pub name: Option<usize>,
    pub body: Vec<Statement>,
}

#[derive(Clone)]
pub(crate) enum Statement {
    Assign(usize, Expr),
    /// Transfers an owned local to another local, resetting its source.
    Move(usize, usize),
    /// Releases an owned temporary and restores its cleanup-safe initial value.
    Clear(usize),
    /// Compiler-owned roots are released on normal and abrupt scope exits.
    Scope {
        body: Vec<Statement>,
        clear: Vec<usize>,
    },
    SetItem(Expr, Expr, Expr),
    AugmentItem(Expr, Expr, Binary, Expr),
    SetField(Expr, usize, Expr),
    AugmentField(Expr, usize, Binary, Expr),
    Expr(Expr),
    If(Expr, Vec<Statement>, Vec<Statement>),
    /// The final suite runs only on condition failure or iterator exhaustion.
    While(Expr, Vec<Statement>, Vec<Statement>),
    For {
        target: usize,
        cursor: usize,
        stop: usize,
        step: usize,
        args: Vec<Expr>,
        body: Vec<Statement>,
        orelse: Vec<Statement>,
    },
    /// Transfers iterator results into owned output locals only on success.
    /// `ready` is always written; exhausted cursors preserve both output locals.
    IterNext {
        iterator: usize,
        key: usize,
        value: Option<usize>,
        ready: usize,
    },
    Return(Expr),
    Break,
    Continue,
    /// A compiler-owned iterator reached its end; unlike break, this runs else.
    Exhaust,
    Assert(Expr),
    /// Handlers protect only `body`; `finalbody` runs for every exit reason.
    Try {
        body: Vec<Statement>,
        handlers: Vec<ExceptHandler>,
        orelse: Vec<Statement>,
        finalbody: Vec<Statement>,
    },
    /// None re-raises the dynamically active exception, including across calls.
    Raise(Option<Expr>),
}

/// Visits expression trees, including statements embedded in eager expressions.
/// Analyses must include every checked branch, even when it never executes.
pub(crate) fn walk_expressions<'a>(body: &'a [Statement], visit: &mut impl FnMut(&'a Expr)) {
    fn expression<'a>(value: &'a Expr, visit: &mut impl FnMut(&'a Expr)) {
        visit(value);
        match &value.kind {
            ExprKind::List(values)
            | ExprKind::Tuple(values)
            | ExprKind::Set { values, .. }
            | ExprKind::Compare(values, _)
            | ExprKind::Boolean(_, values)
            | ExprKind::Builtin(_, values) => {
                for value in values {
                    expression(value, visit);
                }
            }
            ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
                for value in &args.values {
                    expression(value, visit);
                }
            }
            ExprKind::Callable { receiver, .. } => {
                if let Some(receiver) = receiver {
                    expression(receiver, visit);
                }
            }
            ExprKind::Invoke { callable, args } => {
                expression(callable, visit);
                for value in &args.values {
                    expression(value, visit);
                }
            }
            ExprKind::Dict { entries, .. } => {
                for (key, value) in entries {
                    expression(key, visit);
                    expression(value, visit);
                }
            }
            ExprKind::Bind(_, value)
            | ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Not(value)
            | ExprKind::Negate(value)
            | ExprKind::Field(value, _) => expression(value, visit),
            ExprKind::Scope { body, value, .. } => {
                walk_expressions(body, visit);
                expression(value, visit);
            }
            ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
                expression(left, visit);
                expression(right, visit);
            }
            ExprKind::Conditional(test, yes, no) => {
                expression(test, visit);
                expression(yes, visit);
                expression(no, visit);
            }
            ExprKind::Exception { message, .. } => {
                if let Some(message) = message {
                    expression(message, visit);
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::None
            | ExprKind::Local(_) => {}
        }
    }
    for statement in body {
        match statement {
            Statement::Assign(_, value)
            | Statement::Expr(value)
            | Statement::Return(value)
            | Statement::Assert(value) => expression(value, visit),
            Statement::SetItem(receiver, index, value)
            | Statement::AugmentItem(receiver, index, _, value) => {
                expression(receiver, visit);
                expression(index, visit);
                expression(value, visit);
            }
            Statement::SetField(receiver, _, value)
            | Statement::AugmentField(receiver, _, _, value) => {
                expression(receiver, visit);
                expression(value, visit);
            }
            Statement::Scope { body, .. } => walk_expressions(body, visit),
            Statement::If(test, yes, no) => {
                expression(test, visit);
                walk_expressions(yes, visit);
                walk_expressions(no, visit);
            }
            Statement::While(test, body, orelse) => {
                expression(test, visit);
                walk_expressions(body, visit);
                walk_expressions(orelse, visit);
            }
            Statement::For {
                args, body, orelse, ..
            } => {
                for arg in args {
                    expression(arg, visit);
                }
                walk_expressions(body, visit);
                walk_expressions(orelse, visit);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                walk_expressions(body, visit);
                for handler in handlers {
                    walk_expressions(&handler.body, visit);
                }
                walk_expressions(orelse, visit);
                walk_expressions(finalbody, visit);
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    expression(value, visit);
                }
            }
            Statement::Move(..)
            | Statement::Clear(_)
            | Statement::IterNext { .. }
            | Statement::Break
            | Statement::Continue
            | Statement::Exhaust => {}
        }
    }
}

/// Visits one expression's operands. Scoped statements are handled by the caller.
pub(crate) fn children_mut(expression: &mut Expr, mut visit: impl FnMut(&mut Expr)) {
    match &mut expression.kind {
        ExprKind::Exception { message, .. } => {
            if let Some(message) = message {
                visit(message);
            }
        }
        ExprKind::Dict { entries, .. } => {
            for (key, value) in entries {
                visit(key);
                visit(value);
            }
        }
        ExprKind::List(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Tuple(values)
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Builtin(_, values) => {
            for value in values {
                visit(value);
            }
        }
        ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
            for value in &mut args.values {
                visit(value);
            }
        }
        ExprKind::Callable { receiver, .. } => {
            if let Some(receiver) = receiver {
                visit(receiver);
            }
        }
        ExprKind::Invoke { callable, args } => {
            visit(callable);
            for value in &mut args.values {
                visit(value);
            }
        }
        ExprKind::Scope { .. } => {
            unreachable!("scoped statements require the full inspection or rewrite pass")
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => visit(value),
        ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
            visit(left);
            visit(right);
        }
        ExprKind::Conditional(test, yes, no) => {
            visit(test);
            visit(yes);
            visit(no);
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::None
        | ExprKind::Local(_) => {}
    }
}

/// Visits the immediate expressions and suites of one statement.
pub(crate) enum ChildMut<'a> {
    Expression(&'a mut Expr),
    Suite(&'a mut [Statement]),
}

pub(crate) fn statement_children_mut(
    statement: &mut Statement,
    mut visit: impl FnMut(ChildMut<'_>),
) {
    match statement {
        Statement::Assign(_, value)
        | Statement::Expr(value)
        | Statement::Return(value)
        | Statement::Assert(value) => visit(ChildMut::Expression(value)),
        Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
            for value in [a, b, c] {
                visit(ChildMut::Expression(value));
            }
        }
        Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
            visit(ChildMut::Expression(a));
            visit(ChildMut::Expression(b));
        }
        Statement::Scope { body, .. } => visit(ChildMut::Suite(body)),
        Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
            visit(ChildMut::Expression(test));
            visit(ChildMut::Suite(yes));
            visit(ChildMut::Suite(no));
        }
        Statement::For {
            args, body, orelse, ..
        } => {
            for arg in args {
                visit(ChildMut::Expression(arg));
            }
            visit(ChildMut::Suite(body));
            visit(ChildMut::Suite(orelse));
        }
        Statement::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            visit(ChildMut::Suite(body));
            for handler in handlers {
                visit(ChildMut::Suite(&mut handler.body));
            }
            visit(ChildMut::Suite(orelse));
            visit(ChildMut::Suite(finalbody));
        }
        Statement::Raise(Some(value)) => visit(ChildMut::Expression(value)),
        _ => {}
    }
}

/// Rewrites every checked expression, including expressions inside eager scopes.
pub(crate) fn walk_expressions_mut(body: &mut [Statement], visit: &mut impl FnMut(&mut Expr)) {
    fn expression(value: &mut Expr, visit: &mut impl FnMut(&mut Expr)) {
        visit(value);
        if let ExprKind::Scope { body, value, .. } = &mut value.kind {
            walk_expressions_mut(body, visit);
            expression(value, visit);
        } else {
            children_mut(value, |child| expression(child, visit));
        }
    }
    for statement in body {
        statement_children_mut(statement, |child| match child {
            ChildMut::Expression(value) => expression(value, visit),
            ChildMut::Suite(body) => walk_expressions_mut(body, visit),
        });
    }
}
