//! Closed, representation-checked IR. Lowering preserves owned results and may
//! borrow rooted locals for the duration of operator and call evaluation.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SysEncoding {
    Locale,
    Default,
    Filesystem,
    FilesystemErrors,
}

impl SysEncoding {
    pub(crate) fn method(self) -> &'static str {
        match self {
            Self::Locale => "_locale.getencoding",
            Self::Default => "sys.getdefaultencoding",
            Self::Filesystem => "sys.getfilesystemencoding",
            Self::FilesystemErrors => "sys.getfilesystemencodeerrors",
        }
    }
}

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
    IntEnum(usize),
    /// An ordinary Enum with a closed set of string-valued members.
    Enum(usize),
    Bytes,
    ListInt,
    /// Native storage for a private append-only integer list consumed by bytes().
    ByteArray,
    ByteOutput,
    /// Private, owning accumulator for a tuple display with runtime arity.
    TupleBuffer,
    Class(usize),
    /// A weak proxy owns its handle independently of the target record.
    WeakProxy(usize),
    List(usize),
    Dict(usize),
    /// A ChainMap owner, parameterized by the exact dictionary shape of its maps.
    ChainMap(usize),
    Set(usize),
    Deque(usize),
    /// The immutable top-level string keys of a private TOML document.
    StringKeys,
    /// A concrete immutable set of strings.
    FrozenStrings,
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
    BytesIO,
    /// One live imported module from the closed source graph.
    SourceModule,
    /// A sealed, immutable POSIX filesystem path with UTF-8 components.
    Path,
    PurePath(PathFlavor),
    /// The shared, independently retained metadata view of an exact concrete Path.
    PathInfo,
    /// Native implementation metadata has one stable, struct-like owner.
    SysImplementation,
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
    LogManager,
    TimedFileHandler,
    ClockTime,
    Calendar(CalendarType),
    /// An exact instance of Python's plain, identity-only object.
    PlainObject,
    /// The functools singleton and its publicly observable constructor are distinct objects.
    Placeholder(PlaceholderKind),
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
    TextOutput,
    Hash,
    Poll,
    JsonDecoder,
    JsonEncoder,
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
    Date,
    DateTime,
    Delta,
    Zone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalendarOp {
    Date,
    DateTime,
    Combine,
    Replace { timezone_supplied: bool },
    Astimezone,
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
    EmailParseDate,
    EmailFormatDateTime,
    EmailFormatDate,
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
    GetFqdn,
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
            Self::IntEnum(id) => return write!(f, "integer-enum#{id}"),
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
            Self::FrozenStrings => "frozenset[str]",
            Self::ListInt => "list[int]",
            Self::ByteArray => "bytearray",
            Self::ByteOutput => "private byte output",
            Self::TupleBuffer => "private tuple buffer",
            Self::CacheState(_) => "private function cache",
            Self::None => "None",
            Self::Iterator => "iterator",
            Self::Exception | Self::NamedException(_) => "Exception",
            Self::ObjectValue => "object value",
            Self::WeakProxy(_) => "weakref.proxy",
            Self::ReadOnlyObjectMapping => "Mapping[str, object]",
            Self::ImportException => "ImportError",
            Self::TimeoutExpired => "subprocess.TimeoutExpired",
            Self::StringIterable => "Iterable[str]",
            Self::TextIO => "typing.TextIO",
            Self::StringIO => "io.StringIO",
            Self::HandlerLock => "_thread.RLock",
            Self::BinaryIO => "typing.BinaryIO",
            Self::BytesIO => "io.BytesIO",
            Self::SourceModule => "module",
            Self::Path => "pathlib.Path",
            Self::PathInfo => "pathlib.types.PathInfo",
            Self::PurePath(PathFlavor::Posix) => "pathlib.PurePosixPath",
            Self::PurePath(PathFlavor::Windows) => "pathlib.PureWindowsPath",
            Self::SysImplementation => "sys.implementation",
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
            Self::LogManager => "logging.Manager",
            Self::TimedFileHandler => "logging.handlers.TimedRotatingFileHandler",
            Self::ClockTime => "datetime.time",
            Self::Calendar(CalendarType::Date) => "datetime.date",
            Self::Calendar(CalendarType::DateTime) => "datetime.datetime",
            Self::Calendar(CalendarType::Delta) => "datetime.timedelta",
            Self::Calendar(CalendarType::Zone) => "datetime.timezone",
            Self::PlainObject => "object",
            Self::FunctionRef | Self::OverloadStub => "function",
            Self::StrDescriptor(_) => "method_descriptor",
            Self::Placeholder(PlaceholderKind::Value) => "functools._PlaceholderType",
            Self::Placeholder(PlaceholderKind::Class) => "type",
            Self::ProcessSelectorClass => "subprocess selector class",
            Self::SimpleNamespace(_) => "types.SimpleNamespace",
            Self::ModuleSpec(_) => "importlib.machinery.ModuleSpec",
            Self::CodecInfo => "codecs.CodecInfo",
            Self::Poll => "select.poll",
            Self::Socket => "socket.socket",
            Self::CodecWriterClass => "type[codecs.StreamWriter]",
            Self::CodecWriter => "codecs.StreamWriter",
            Self::TextOutput => "writable text stream",
            Self::Hash => "hashlib hash",
            Self::JsonDecoder => "json.decoder.JSONDecoder",
            Self::JsonEncoder => "json.encoder.JSONEncoder",
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
            | Self::IntEnum(_)
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
    /// Interpreter settings already selected by the Python-style launcher.
    pub command_flags: Option<crate::CommandFlags>,
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

/// Fields initialized by the sealed enum constructors in the bundled http module.
#[derive(Clone)]
pub(crate) struct HttpMember {
    pub code: Option<u16>,
    pub phrase: String,
    pub description: String,
}

pub(crate) struct ClassMetadata {
    pub name: String,
    pub qualified_name: String,
    /// Canonical members of a string enum, in declaration order.
    pub enum_members: Vec<(String, String)>,
    /// Plain Enum names its class and member; StrEnum displays the value.
    pub plain_enum: bool,
    pub http_members: Vec<HttpMember>,
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
    /// A trusted bundled field containing the complete, unquoted record representation.
    pub repr_text: bool,
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
    /// Read a local slot with its type at the use site.
    pub(crate) fn local(slot: usize, ty: Type) -> Self {
        Self {
            ty,
            kind: ExprKind::Local(slot),
        }
    }

    pub(crate) fn none() -> Self {
        Self {
            ty: Type::None,
            kind: ExprKind::None,
        }
    }

    pub(crate) fn boolean(value: bool) -> Self {
        Self {
            ty: Type::Bool,
            kind: ExprKind::Bool(value),
        }
    }

    /// Store the decimal spelling so the literal can exceed the host integer width.
    pub(crate) fn integer(value: impl ToString) -> Self {
        Self {
            ty: Type::Int,
            kind: ExprKind::Int(value.to_string()),
        }
    }

    /// Construct an absent optional from a literal None.
    pub(crate) fn absent(ty: Type) -> Self {
        Self {
            ty,
            kind: ExprKind::Absent(Box::new(Self::none())),
        }
    }

    /// A single explicit split whose immediately selected field is its prefix
    /// (`split(sep, 1)[0]`) or its suffix (`rsplit(sep, 1)[-1]`).
    pub(crate) fn selected_split_args(&self, index: &Self) -> Option<(bool, &[Expr])> {
        let ExprKind::Builtin(Builtin::Text(TextOperation::Split(reverse)), args) = &self.kind
        else {
            return None;
        };
        let [_, separator, limit] = args.as_slice() else {
            return None;
        };
        let selected = if *reverse {
            matches!(&index.kind, ExprKind::Negate(one) if matches!(&one.kind, ExprKind::Int(n) if n == "1"))
        } else {
            matches!(&index.kind, ExprKind::Int(n) if n == "0")
        };
        (selected
            && matches!(separator.kind, ExprKind::Present(_))
            && matches!(&limit.kind, ExprKind::Int(n) if n == "1"))
        .then_some((*reverse, args))
    }
}

/// A reverse slice with a literal negative start and stride and no lower endpoint.
pub(crate) fn reverse_slice(parts: &[Expr]) -> Option<(&Expr, i64, i64)> {
    let [source, start, stop, step] = parts else {
        return None;
    };
    if !omitted(stop) {
        return None;
    }
    let start = if omitted(start) {
        -1
    } else {
        negative_component(start)?
    };
    Some((source, start, negative_component(step)?))
}

/// Whether a slice component is an omitted bound with no evaluation effects.
pub(crate) fn omitted(value: &Expr) -> bool {
    matches!(&value.kind, ExprKind::Absent(inner) if matches!(inner.kind, ExprKind::None))
}

/// Optional slice components wrap their integer value in Present.
fn negative_component(value: &Expr) -> Option<i64> {
    let value = match &value.kind {
        ExprKind::Present(value) => value.as_ref(),
        _ => return None,
    };
    let value = match &value.kind {
        ExprKind::Int(value) => value.parse::<i64>().ok()?,
        ExprKind::Negate(value) => {
            let ExprKind::Int(value) = &value.kind else {
                return None;
            };
            value.parse::<i64>().ok()?.checked_neg()?
        }
        _ => return None,
    };
    (-(1_i64 << 62)..0).contains(&value).then_some(value)
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
    Shuffle,
    Seed,
    Float,
    Bits,
    ChoiceIndex { secure: bool },
    Randint,
    Randrange,
    RandrangeOne,
    ChoiceRange { secure: bool },
}

/// Selects an ordering over the pooled inputs of an itertools iterator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Combinatorial {
    Product,
    Permutations,
    Combinations,
    CombinationsWithReplacement,
}

#[derive(Clone, Copy)]
pub(crate) enum PairTableOp {
    New,
    AddRow,
    Index,
    Push,
    Len,
    RowLen,
    Field(usize),
    Snapshot(usize),
    ViewLen(usize),
    ViewField(usize, usize),
}

#[derive(Clone)]
pub(crate) enum Builtin {
    ByteArrayNew,
    ByteArrayAppend,
    ByteArrayExtend,
    ByteArrayReverse,
    PairTable(PairTableOp),
    Combinatorial(Combinatorial),
    DecimalIntegerText,
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
    IterableTuple,
    TupleBufferNew,
    TupleBufferPush,
    TupleBufferFinish,
    Cycle,
    SysStdout,
    SysHasStdout,
    SysOriginalOutput {
        stderr: bool,
    },
    SysStdoutSet,
    SysStdoutDelete,
    TextOptionalReceiver,
    TextMetadata {
        errors: bool,
    },
    SysDisplayHook,
    BuiltinsLast,
    BuiltinsLen,
    FnmatchFunction {
        normcase: bool,
    },
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
    IsCoroutineFunction,
    MarkCoroutine(usize),
    FunctionWrappedSet,
    FunctionWrappedGet,
    StrEnumValue,
    IntEnumValue,
    HttpEnumPhrase,
    HttpEnumDescription,
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
    ExceptionSubclass(u32, u32),
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
        field: u32,
    },
    SysImplementation,
    SysStdlibModuleNames,
    SysStreamText {
        descriptor: u8,
        errors: bool,
    },
    SysEncoding(SysEncoding),
    SysVersionInfo {
        implementation: bool,
    },
    SysIntern,
    SysIsInterned,
    OsName,
    OsLinesep,
    OsPathsep,
    OsSeparator {
        alternate: bool,
    },
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
    ClockParts(bool),
    Mktime,
    MktimeOffset,
    TimeComponent,
    TimeSequenceLength,
    TimeExtras,
    TimeExtra(bool),
    TimeSleep,
    WarningFilter {
        category: u32,
        invalid: u32,
    },
    WarningReset,
    WarningScopeEnter,
    WarningScopeExit,
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
    IntExtremum {
        minimum: bool,
    },
    IntBitLength,
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
    ObjectStreamProject,
    ObjectTextProject,
    ObjectIsExactTuple,
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
    /// object.__repr__ for a rooted source instance, bypassing its __repr__ override.
    DefaultRecordRepr,
    Len,
    LengthResult,
    Abs,
    FloatCopySign,
    /// Stable permutation of a fully typed list by precomputed scalar keys.
    SortByKeys,
    SortByComparator,
    ListExtendBuffer,
    ListExtendSnapshot,
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
    /// Extend a proven periodic packed prefix and return the ordinal where normal iteration resumes.
    StructCyclePrefix,
    /// The guarded loop proves all elements fit i32 and cannot be replaced.
    ListGetI32,
    /// Reads a checked index from the stable buffer in a guarded loop body.
    ListGetStable,
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
    /// Checks that a list local owns storage before borrowing its buffer.
    ListPresent,
    StringGet,
    StringIter,
    /// Iterates Python line boundaries on a rooted immutable string.
    SplitLinesIter,
    /// Iterates every field of a forward split by a literal ASCII separator.
    SplitByteIter(u8),
    MatchingSplitIter(u8),
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
    /// Two nonnegative 31-bit digit subtotals in one immediate, or -1 for fallback.
    TryDigitSums([[u8; 10]; 2]),
    Bytes,
    BytesGet,
    Encode,
    Decode,
    DecodeAscii,
    BytesHex,
    FloatDivmod,
    UrlsafeB64encode,
    Base64Translate,
    BinasciiAsciiInput,
    Adler32,
    BinasciiDecode,
    BinasciiEncode,
    BinasciiUnhexlify,
    Base32Encode,
    Base32Decode,
    Base85Encode {
        alphabet: &'static [u8; 85],
        foldnuls: bool,
    },
    CodecLookup,
    CodecName,
    StrEncodeOptions,
    CodecEncode,
    CodecDecode,
    Socket(SocketOp),
    CodecWriter(WriterOperation),
    Sha1Hex,
    HmacCompareDigest {
        text: bool,
    },
    Sha1New,
    Sha256Hex,
    Sha256New,
    HashNew,
    NewPlainObject,
    Typing(TypingOperation),
    StrDescriptor(StrDescriptor),
    Placeholder(PlaceholderKind),
    InspectSentinel,
    HashAlgorithms {
        guaranteed: bool,
    },
    HashUpdate,
    HashUpdateFile,
    HashName,
    HashType,
    HashIdentity,
    HashDigestSize,
    HashBlockSize,
    HashDigest {
        hex: bool,
    },
    HashCopy,
    BytesIo,
    BytesIoValue,
    StringIo(StringIoOperation),
    ContextVar(ContextOperation),
    BinarySeek,
    BinaryTell,
    ZipOpenFile,
    ZipGetInfo,
    ZipImportData,
    ZipInfoList,
    ZipMember,
    ZipRead,
    ZipTest,
    ZipFilename,
    ZipMinReadSize,
    ZipField(ZipField),
    ZipOpen,
    ZipImportIndex,
    ZipNamelist,
    ZipClose,
    /// Encodes an integer with an explicit width, byte order, and sign policy.
    IntToBytes,
    /// Validates complete UTF-8 at the decode evaluation point and returns a
    /// retained bytes identity for an internal allocation-free character gather.
    BytesValidateUtf8,
    /// Receiver followed by three nullable integer words for start, stop, step.
    Slice,
    /// Compare two contiguous regions of immutable strings without copying them.
    StringSliceEqual,
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
    /// Parse a single field of an exact no-separator str.split() as a decimal integer.
    SplitIntField(u32),
    /// Prepare small decimal fields from one private immutable split source.
    SplitIntSource(u8),
    SplitIntCached(u32),
    SplitLines,
    /// Only the first line and a joined remainder can be observed.
    JoinedSplitLines,
    DecodeSplitLines,
    Ndiff,
    /// Copy an exact list of immutable strings into a distinct mutable list.
    StringListCopy,
    /// Select by up to two prefixes, optionally exclude one, then slice each match.
    StringListFilter(u32),
    /// Collect characters in a set or below a Unicode scalar threshold. An
    /// optional result represents an empty selection as an absent set.
    FilterCharacterSet(u32),
    /// Stable sort by the suffix after a fixed number of Unicode code points.
    StringListSortSuffix(u32),
    /// Ask whether a source has an item before evaluating a comprehension predicate.
    StringListNonempty,
    RStrip,
    SysArgv,
    OptionalStrConcat,
    SysOrigArgv,
    CanElideMapperFrames([usize; 2]),
    SysGetFrameModuleName,
    SysGetRecursionLimit,
    SysContextAwareWarnings,
    /// Native source metadata for a Python function or module, before its first statement.
    FaultFrame {
        file: String,
        name: String,
        line: u32,
    },
    FaultLine(u32),
    GeneratorFrameEnter(String),
    GeneratorFrameLeave,
    SysSetRecursionLimit,
    /// The child layout descriptors follow the literal subcommand order.
    ParseArgs {
        namespace: usize,
        children: Vec<usize>,
        process_argv: bool,
    },
    /// Format parser text or print it to the requested stream.
    ArgparseChoiceDigits(usize),
    ArgparseText {
        usage: bool,
        output: ArgparseOutput,
    },
    /// Retags a namespace after its immutable command field selects a child layout.
    RecordView,
    WeakProxyNew,
    WeakProxyTarget,
    WeakProxyCallable,
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
    JsonEncoderNew,
    JsonEncoderEncode(Type),
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
    /// A fresh partial with the same call interface, owning its original target.
    EmptyPartial,
    HashStr,
    HashBytes,
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
    RegexSearchText {
        anchored: bool,
        verbose: bool,
    },
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
    DictIsOrdered,
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
    DequeRotate,
    DequeGet {
        method: bool,
    },
    DequeSet,
    DequeDelete {
        method: bool,
    },
    DequeIndex,
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
    DictPopItem,
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
    /// Record the source location of a raised Python exception.
    ExceptionOrigin(u64),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u32)]
pub(crate) enum PlaceholderKind {
    Value,
    Class,
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
    IsLower,
    IsAscii,
    IsAlpha,
    IsDigit,
    ZFill,
    IsTitle,
    Title,
    SwapCase,
}
impl StrDescriptor {
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "lower" => Self::Lower,
            "title" => Self::Title,
            "swapcase" => Self::SwapCase,
            "strip" => Self::Strip,
            "lstrip" => Self::LStrip,
            "rstrip" => Self::RStrip,
            "isalnum" => Self::IsAlnum,
            "isascii" => Self::IsAscii,
            "isalpha" => Self::IsAlpha,
            "isdigit" => Self::IsDigit,
            "istitle" => Self::IsTitle,
            "islower" => Self::IsLower,
            "zfill" => Self::ZFill,
            _ => return None,
        })
    }
    pub(crate) fn is_predicate(self) -> bool {
        matches!(
            self,
            Self::IsAlnum
                | Self::IsAscii
                | Self::IsAlpha
                | Self::IsDigit
                | Self::IsLower
                | Self::IsTitle
        )
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Title => "title",
            Self::SwapCase => "swapcase",
            Self::Strip => "strip",
            Self::LStrip => "lstrip",
            Self::RStrip => "rstrip",
            Self::IsAlnum => "isalnum",
            Self::IsAscii => "isascii",
            Self::IsAlpha => "isalpha",
            Self::IsDigit => "isdigit",
            Self::IsTitle => "istitle",
            Self::IsLower => "islower",
            Self::ZFill => "zfill",
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
    ZFill {
        copy: bool,
    },
    Normalize,
    Lower,
    Upper,
    Capitalize,
    CaseMap {
        title: bool,
    },
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
    UrlQuote,
    UrlQuotePlus,
    UrlUnquote,
    UrlSplit,
    UrlJoin,
    ParseQsl,
    UrlProperty(UrlProperty),
    UrlGetUrl,
    UrlTranscode {
        encode: bool,
    },
    Dedent,
    ShlexSplit,
    ShlexQuote,
}

#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum UrlProperty {
    Username,
    Password,
    Hostname,
    Port,
}

#[derive(Clone, Copy)]
pub(crate) enum TextPredicate {
    Alnum,
    Ascii,
    Alpha,
    Digit,
    Space,
    Lower,
    Title,
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
    Parts,
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
    OpenTextOutput,
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
    TemporaryFile,
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
    AddExtra,
    Dictionary,
    HasAttr,
    FieldView,
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
    Handlers,
    AddHandler,
    RemoveHandler,
    WriteStream,
    FlushStream,
    StreamFile,
    HandlerRegister(usize),
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
    HandlerNames,
    FileEncodingUsesUtf8,
    PrintActiveException,
    New,
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
    DictConfig,
    DictConfigTyped,
    FileConfig,
    Disabled,
    SetDisabled,
    NewManager,
    Manager,
    ManagerRoot,
    ManagerDisable,
    SetManagerDisable,
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
    TransferFd,
    CopyFileRange,
    CheckCopyCount,
    CheckCopyOffset,
    Read,
    Write,
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
    DeviceEncoding,
    PtsName,
    TcGetPgrp,
    TcSetPgrp,
    GetBlocking,
    SetBlocking,
    GetInheritable,
    SetInheritable,
    OpenPty,
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
        // Discarded run calls supply check as an evaluated operand.
        fixed_check: Option<bool>,
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
    EnvironCopy {
        bytes: bool,
    },
    EnvironClear,
    EnvironUpdate {
        bytes: bool,
    },
    EnvironGet,
    EnvironKeys,
    EnvironIndex {
        bytes: bool,
    },
    EnvironDelete {
        bytes: bool,
    },
    EnvironSet {
        bytes: bool,
    },
    GetPid,
    GetCwd {
        bytes: bool,
    },
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
            (Self::Buffered, Type::BytesIO)
            | (Self::Any, Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO)
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
    /// An exact builtin string, excluding nominal string subclasses.
    ExactText,
    ContextVar,
    ContextToken,
    IoBase(IoBase),
    ForwardRef,
    SimpleNamespace,
    ModuleSpec,
    StrEnum,
    IntEnum,
    List,
    Deque,
    Dict,
    Set,
    Tuple,
    Exception(u32),
}

impl InstanceType {
    pub(crate) fn accepts(self, ty: Type, types: &crate::types::Types) -> bool {
        match self {
            Self::IoBase(base) => base.matches(ty) != Some(false),
            Self::Exact(Type::CompletedProcess(..)) => matches!(ty, Type::CompletedProcess(..)),
            Self::ContextVar => matches!(ty, Type::ContextVar(_)),
            Self::ContextToken => matches!(ty, Type::ContextToken(_)),
            Self::Exact(Type::Int) => matches!(ty, Type::Int | Type::Bool | Type::IntEnum(_)),
            Self::ExactText => matches!(ty, Type::Str | Type::StrLiteral(_)),
            Self::Exact(Type::Str) => {
                matches!(ty, Type::Str | Type::StrLiteral(_) | Type::StrEnum(_))
            }
            Self::Exact(Type::BuiltinClass) => matches!(
                ty,
                Type::BuiltinClass | Type::Placeholder(PlaceholderKind::Class)
            ),
            Self::Exact(expected) => {
                expected == ty
                    || (types.time_attributes(expected).is_some()
                        && types.time_attributes(ty).is_some())
            }
            Self::ForwardRef => matches!(ty, Type::ForwardRef(_)),
            Self::StrEnum => matches!(ty, Type::StrEnum(_)),
            Self::IntEnum => matches!(ty, Type::IntEnum(_)),
            Self::List => matches!(ty, Type::ListInt | Type::List(_)),
            Self::Deque => matches!(ty, Type::Deque(_)),
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

/// Selects the pull ABI for a cursor created by the compiler.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IterNextKind {
    Generic,
    String,
    CodePoint,
    /// One unsigned UTF-8 byte. The loop lowerer creates a hidden string
    /// iterator whose only uses are this pull and scope cleanup.
    Utf8Byte,
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
        kind: IterNextKind,
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

// Match ergonomics preserve the borrow of each operand and suite for inspection or rewriting.
macro_rules! statement_children {
    ($statement:expr, $expression:expr, $suite:expr) => {
        match $statement {
            Statement::Assign(_, value)
            | Statement::Expr(value)
            | Statement::Return(value)
            | Statement::Assert(value) => ($expression)(value),
            Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    ($expression)(value);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                ($expression)(a);
                ($expression)(b);
            }
            Statement::Scope { body, .. } => ($suite)(body),
            Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
                ($expression)(test);
                ($suite)(yes);
                ($suite)(no);
            }
            Statement::For {
                args, body, orelse, ..
            } => {
                for arg in args {
                    ($expression)(arg);
                }
                ($suite)(body);
                ($suite)(orelse);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                ($suite)(body);
                for ExceptHandler { body, .. } in handlers {
                    ($suite)(body);
                }
                ($suite)(orelse);
                ($suite)(finalbody);
            }
            Statement::Raise(Some(value)) => ($expression)(value),
            Statement::Move(..)
            | Statement::Clear(_)
            | Statement::Raise(None)
            | Statement::IterNext { .. }
            | Statement::Break
            | Statement::Continue
            | Statement::Exhaust => {}
        }
    };
}

impl Statement {
    /// Visits nested suites in source order, excluding suites owned by expressions.
    pub(crate) fn suites<'a>(&'a self, mut visit: impl FnMut(&'a [Statement])) {
        statement_children!(self, |_| {}, visit);
    }
}

enum Node<'a> {
    Statement(&'a Statement),
    Expression(&'a Expr),
}

fn walk_nodes<'a>(body: &'a [Statement], visit: &mut impl FnMut(Node<'a>)) {
    for statement in body {
        visit(Node::Statement(statement));
        statement_children!(statement, |value| walk_expression(value, visit), |suite| {
            walk_nodes(suite, visit)
        });
    }
}

fn walk_expression<'a>(value: &'a Expr, visit: &mut impl FnMut(Node<'a>)) {
    visit(Node::Expression(value));
    if let ExprKind::Scope { body, value, .. } = &value.kind {
        walk_nodes(body, visit);
        walk_expression(value, visit);
    } else {
        children(value, |child| walk_expression(child, visit));
    }
}

/// Visits all statement nodes once, including those nested inside eager expressions.
pub(crate) fn walk_statements<'a>(body: &'a [Statement], visit: &mut impl FnMut(&'a Statement)) {
    walk_nodes(body, &mut |node| {
        if let Node::Statement(statement) = node {
            visit(statement);
        }
    });
}

/// Includes loops in eager expression scopes when deciding whether a suite can be cloned.
pub(crate) fn contains_loop(body: &[Statement]) -> bool {
    let mut found = false;
    walk_statements(body, &mut |statement| {
        found |= matches!(statement, Statement::For { .. } | Statement::While(..));
    });
    found
}

/// Visits expression trees, including statements embedded in eager expressions.
/// Analyses must include every checked branch, even when it never executes.
pub(crate) fn walk_expressions<'a>(body: &'a [Statement], visit: &mut impl FnMut(&'a Expr)) {
    walk_nodes(body, &mut |node| {
        if let Node::Expression(value) = node {
            visit(value);
        }
    });
}

/// Finds forward string cursors whose only observable operation is a byte or scalar pull.
/// Clearing the cursor ends the borrow. Replacing it with a fresh string cursor
/// starts a new borrow. A move, a read of the cursor itself, or another kind of
/// pull requires the position stored in the runtime iterator.
pub(crate) fn private_string_cursors(function: &Function) -> Vec<usize> {
    let mut pulled = vec![false; function.locals.len()];
    let mut created = vec![false; function.locals.len()];
    let mut exposed = vec![false; function.locals.len()];
    for &parameter in &function.params {
        exposed[parameter] = true;
    }
    walk_nodes(&function.body, &mut |node| {
        match &node {
            Node::Statement(Statement::IterNext {
                iterator,
                kind: IterNextKind::CodePoint | IterNextKind::Utf8Byte,
                ..
            }) => pulled[*iterator] = true,
            Node::Statement(Statement::IterNext { iterator, .. }) => exposed[*iterator] = true,
            Node::Expression(Expr {
                kind: ExprKind::Local(slot),
                ..
            }) => exposed[*slot] = true,
            // Scope cleanup clears owners without publishing a replacement cursor.
            Node::Statement(Statement::Clear(_) | Statement::Scope { .. })
            | Node::Expression(Expr {
                kind: ExprKind::Scope { .. },
                ..
            }) => return,
            _ => {}
        }
        node_writes(node, &mut |slot, value| {
            if value.is_some_and(|v| matches!(v.kind, ExprKind::Builtin(Builtin::StringIter, _))) {
                created[slot] = true;
            } else {
                exposed[slot] = true;
            }
        });
    });
    (0..pulled.len())
        .filter(|&slot| pulled[slot] && created[slot] && !exposed[slot])
        .collect()
}

/// Find private cursors restarted from a parameter that is rooted throughout the
/// containing loop. Every creation and pull must lie inside that same loop.
pub(crate) fn invariant_string_cursors(function: &Function, private: &[usize]) -> BTreeSet<usize> {
    let mut result = BTreeSet::new();
    if private.is_empty() {
        return result;
    }
    let mut creates = vec![0_usize; function.locals.len()];
    let mut pulls = vec![0_usize; function.locals.len()];
    walk_statements(&function.body, &mut |statement| match statement {
        Statement::Assign(
            slot,
            Expr {
                kind: ExprKind::Builtin(Builtin::StringIter, _),
                ..
            },
        ) => creates[*slot] += 1,
        Statement::IterNext { iterator, .. } => pulls[*iterator] += 1,
        _ => {}
    });
    let candidates: Vec<_> = private
        .iter()
        .copied()
        .filter(|&slot| creates[slot] == 1)
        .collect();
    if candidates.is_empty() {
        return result;
    }
    fn loops(body: &[Statement], visit: &mut impl FnMut(&Statement, &[Statement])) {
        for statement in body {
            match statement {
                Statement::While(_, body, orelse) | Statement::For { body, orelse, .. } => {
                    visit(statement, body);
                    loops(orelse, visit);
                }
                _ => statement.suites(|suite| loops(suite, visit)),
            }
        }
    }
    // Lowering sometimes saves the source before initializing scalar loop counters.
    fn parameter_source(before: &[Statement], source: usize, stable: &[bool]) -> bool {
        if stable[source] {
            return true;
        }
        for statement in before.iter().rev() {
            match statement {
                Statement::Assign(
                    target,
                    Expr {
                        kind: ExprKind::Local(origin),
                        ..
                    },
                ) if *target == source => return stable[*origin],
                Statement::Assign(
                    target,
                    Expr {
                        kind: ExprKind::Int(_) | ExprKind::Bool(_),
                        ..
                    },
                ) if *target != source => {}
                _ => break,
            }
        }
        false
    }
    fn mark(suite: &[Statement], stable: &[bool], eligible: &[bool], result: &mut BTreeSet<usize>) {
        for (index, statement) in suite.iter().enumerate() {
            if let Statement::Assign(
                slot,
                Expr {
                    kind: ExprKind::Builtin(Builtin::StringIter, args),
                    ..
                },
            ) = statement
                && eligible[*slot]
                && let ExprKind::Local(source) = args[0].kind
                && parameter_source(&suite[..index], source, stable)
            {
                result.insert(*slot);
            }
            statement.suites(|nested| mark(nested, stable, eligible, result));
        }
    }
    loops(&function.body, &mut |statement, body| {
        let mut stable = vec![false; function.locals.len()];
        for &parameter in &function.params {
            stable[parameter] = function.locals[parameter] == Type::Str;
        }
        walk_writes(std::slice::from_ref(statement), |slot, _| {
            stable[slot] = false
        });
        if !stable.iter().any(|&rooted| rooted) {
            return;
        }
        let mut inside = vec![0_usize; function.locals.len()];
        walk_statements(body, &mut |statement| {
            if let Statement::IterNext { iterator, .. } = statement {
                inside[*iterator] += 1;
            }
        });
        let mut eligible = vec![false; function.locals.len()];
        for &slot in &candidates {
            eligible[slot] = inside[slot] == pulls[slot];
        }
        mark(body, &stable, &eligible, &mut result);
    });
    result
}

/// Visits every local write in a suite. Only ordinary assignments carry a right-hand side.
pub(crate) fn walk_writes(body: &[Statement], mut visit: impl FnMut(usize, Option<&Expr>)) {
    walk_nodes(body, &mut |node| node_writes(node, &mut visit));
}

/// Visits writes during operand evaluation, including assignments in eager scopes.
pub(crate) fn walk_expression_writes(value: &Expr, mut visit: impl FnMut(usize, Option<&Expr>)) {
    walk_expression(value, &mut |node| node_writes(node, &mut visit));
}

/// Whether evaluating an operand leaves the previously loaded local in place.
pub(crate) fn expression_preserves_local(expression: &Expr, slot: usize) -> bool {
    let mut preserved = true;
    walk_expression_writes(expression, |written, _| preserved &= written != slot);
    preserved
}

fn node_writes(node: Node<'_>, visit: &mut impl FnMut(usize, Option<&Expr>)) {
    let statement = match node {
        Node::Expression(value) => {
            match &value.kind {
                ExprKind::Scope { clear, .. } => {
                    for &slot in clear {
                        visit(slot, None);
                    }
                }
                ExprKind::Bind(slot, _) => visit(*slot, None),
                _ => {}
            }
            return;
        }
        Node::Statement(statement) => statement,
    };
    match statement {
        Statement::Assign(slot, value) => visit(*slot, Some(value)),
        Statement::Clear(slot) => visit(*slot, None),
        Statement::Move(target, source) => {
            visit(*target, None);
            visit(*source, None);
        }
        Statement::Scope { clear, .. } => {
            for &slot in clear {
                visit(slot, None);
            }
        }
        Statement::For {
            target,
            cursor,
            stop,
            step,
            ..
        } => {
            for &slot in [target, cursor, stop, step] {
                visit(slot, None);
            }
        }
        Statement::Try { handlers, .. } => {
            for handler in handlers {
                if let Some(slot) = handler.name {
                    visit(slot, None);
                }
            }
        }
        Statement::IterNext {
            key, value, ready, ..
        } => {
            for slot in [Some(*key), *value, Some(*ready)].into_iter().flatten() {
                visit(slot, None);
            }
        }
        _ => {}
    }
}

// The same field patterns yield shared or mutable operands depending on the borrow.
macro_rules! expression_children {
    ($kind:expr, $visit:ident) => {
        match $kind {
            ExprKind::Exception { message, .. } => {
                if let Some(message) = message {
                    $visit(message);
                }
            }
            ExprKind::Dict { entries, .. } => {
                for (key, value) in entries {
                    $visit(key);
                    $visit(value);
                }
            }
            ExprKind::List(values)
            | ExprKind::Set { values, .. }
            | ExprKind::Tuple(values)
            | ExprKind::Compare(values, _)
            | ExprKind::Boolean(_, values)
            | ExprKind::Builtin(_, values)
            | ExprKind::Call(_, CallArgs { values, .. })
            | ExprKind::Construct {
                args: CallArgs { values, .. },
                ..
            } => {
                for value in values {
                    $visit(value);
                }
            }
            ExprKind::Callable { receiver, .. } => {
                if let Some(receiver) = receiver {
                    $visit(receiver);
                }
            }
            ExprKind::Invoke {
                callable,
                args: CallArgs { values, .. },
            } => {
                $visit(callable);
                for value in values {
                    $visit(value);
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
            | ExprKind::Field(value, _) => $visit(value),
            ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
                $visit(left);
                $visit(right);
            }
            ExprKind::Conditional(test, yes, no) => {
                $visit(test);
                $visit(yes);
                $visit(no);
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::None
            | ExprKind::Local(_) => {}
        }
    };
}

/// Visits one expression's operands. Scoped statements are handled by the caller.
pub(crate) fn children<'a>(expression: &'a Expr, mut visit: impl FnMut(&'a Expr)) {
    expression_children!(&expression.kind, visit);
}

/// Visits one expression's operands for rewriting.
pub(crate) fn children_mut(expression: &mut Expr, mut visit: impl FnMut(&mut Expr)) {
    expression_children!(&mut expression.kind, visit);
}

/// Visits the immediate expressions and suites of one statement.
pub(crate) enum ChildMut<'a> {
    Expression(&'a mut Expr),
    Suite(&'a mut Vec<Statement>),
}

pub(crate) fn statement_children_mut(
    statement: &mut Statement,
    mut visit: impl FnMut(ChildMut<'_>),
) {
    statement_children!(
        statement,
        |value| visit(ChildMut::Expression(value)),
        |body| visit(ChildMut::Suite(body))
    );
}

/// Visits original statements child-first, including those in expression scopes.
/// A replacement installed by the visitor is not revisited.
pub(crate) fn rewrite_statements(body: &mut [Statement], visit: &mut impl FnMut(&mut Statement)) {
    walk_mut(body, &mut |_| {}, visit);
}

/// Rewrites every checked expression, including expressions inside eager scopes.
pub(crate) fn walk_expressions_mut(body: &mut [Statement], visit: &mut impl FnMut(&mut Expr)) {
    walk_mut(body, visit, &mut |_| {});
}

fn walk_mut(
    body: &mut [Statement],
    enter_expr: &mut impl FnMut(&mut Expr),
    leave_stmt: &mut impl FnMut(&mut Statement),
) {
    fn expression(
        value: &mut Expr,
        enter_expr: &mut impl FnMut(&mut Expr),
        leave_stmt: &mut impl FnMut(&mut Statement),
    ) {
        enter_expr(value);
        if let ExprKind::Scope { body, value, .. } = &mut value.kind {
            walk_mut(body, enter_expr, leave_stmt);
            expression(value, enter_expr, leave_stmt);
        } else {
            children_mut(value, |child| expression(child, enter_expr, leave_stmt));
        }
    }
    for statement in body {
        statement_children_mut(statement, |child| match child {
            ChildMut::Expression(value) => expression(value, enter_expr, leave_stmt),
            ChildMut::Suite(body) => walk_mut(body, enter_expr, leave_stmt),
        });
        leave_stmt(statement);
    }
}
