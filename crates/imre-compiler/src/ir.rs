//! Closed, representation-checked IR. Lowering preserves owned results and may
//! borrow rooted locals for the duration of operator and call evaluation.

pub(crate) mod control;

use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;
use std::collections::BTreeSet;

/// Return the exact value when it fits in the tagged integer representation.
pub(crate) fn tagged_integer(value: i64) -> Option<i64> {
    (-(1_i64 << 62)..1_i64 << 62)
        .contains(&value)
        .then_some(value)
}

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
    /// Immutable start, stop and step. Each iteration gets its own cursor.
    Range,
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
    /// A live Python value rooted by the registered activation and identified in its heap.
    PythonObject,
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
    RegexBytesPattern,
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
    RegexBytesMatch,
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
    MonthGrid,
    InstanceMonthGrid,
    FirstWeekday,
    SetFirstWeekday,
    Date,
    DateTime,
    Combine,
    FromIsoFormat,
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
    ControlSize { padded: bool },
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

    /// The checked sequence layout multiplied by an integer in either operand order.
    pub(crate) fn repeated_sequence(left: Self, right: Self) -> Option<Self> {
        let sequence = match (left, right) {
            (sequence, Self::Int) | (Self::Int, sequence) => sequence,
            _ => return None,
        };
        matches!(
            sequence,
            Self::Str | Self::Bytes | Self::ListInt | Self::List(_)
        )
        .then_some(sequence)
    }

    pub(crate) fn is_path(self) -> bool {
        matches!(self, Self::Path | Self::PurePath(_))
    }

    pub(crate) fn builtin_scalar_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Str => "str",
            Self::Bytes => "bytes",
            _ => return None,
        })
    }

    pub(crate) fn is_format_scalar(self) -> bool {
        self.builtin_scalar_name().is_some()
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
            Self::PythonObject => "Python object",
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
            Self::Range => "range",
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
            Self::RegexBytesPattern => "re.Pattern[bytes]",
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
            Self::RegexBytesMatch => "re.Match[bytes]",
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

/// Loader identity and payload selected for one module initializer.
pub(crate) struct ModuleRegistration<'a> {
    pub(crate) name: &'a str,
    pub(crate) file: &'a std::path::Path,
    pub(crate) loader: ModuleLoader<'a>,
}

pub(crate) enum ModuleLoader<'a> {
    Source {
        builtin: bool,
        text: Option<&'a [u8]>,
    },
    Physical {
        kind: PhysicalModuleKind<'a>,
        digest: [u8; 32],
        future_flags: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhysicalModuleKind<'a> {
    File,
    DeclaredFile(&'a std::path::Path),
    Zip,
}

pub(crate) struct Module {
    /// Interpreter settings already selected by the Python-style launcher.
    pub command_flags: Option<crate::CommandFlags>,
    /// NUL-separated source registrations that mask native providers in this image.
    pub builtin_module_sources: Option<String>,
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
    /// Checked instance method used to render a retained value after type erasure.
    pub repr_method: Option<usize>,
    pub str_method: Option<usize>,
    /// An explicit async override cannot be called through a synchronous text slot.
    pub async_str: bool,
    pub value_struct: bool,
    pub field_equality: bool,
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
        self.visit_expression_bindings(|slot| rebound[slot] = true);
        rebound
    }

    pub(crate) fn has_expression_bindings(&self) -> bool {
        let mut found = false;
        self.visit_expression_bindings(|_| found = true);
        found
    }

    fn visit_expression_bindings(&self, mut visit: impl FnMut(usize)) {
        walk_expressions(&self.body, &mut |value| {
            if let ExprKind::Bind(slot, _) = value.kind {
                visit(slot);
            }
        });
    }
}

/// The checked truth of a fixed-layout operand may call Python code.
#[derive(Clone)]
pub(crate) struct TruthPredicate {
    pub(crate) receiver: usize,
    pub(crate) condition: Box<Expr>,
}

#[derive(Clone)]
pub(crate) struct Expr {
    pub ty: Type,
    pub kind: ExprKind,
}

impl Expr {
    /// A starred display or insertion evaluates the iterable inside its spread marker.
    pub(crate) fn spread_operand(&self) -> Option<&Self> {
        match &self.kind {
            ExprKind::Builtin(Builtin::RegisteredSpread, operands) => Some(&operands[0]),
            _ => None,
        }
    }

    /// The checker's type at this use can refine a live Python handle.
    pub(crate) fn checked_operand_type(&self) -> Type {
        match self.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(_, _, Some(ty)), _) => ty,
            _ => self.ty,
        }
    }

    /// Written operands available to a direct named call. A module read has no call site.
    pub(crate) fn named_call_arguments(&self) -> Option<&[Expr]> {
        match &self.kind {
            ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => Some(&args.values),
            ExprKind::Exception { message, .. } => {
                Some(message.as_deref().map_or(&[], std::slice::from_ref))
            }
            ExprKind::Builtin(Builtin::GlobalGet(_), _) => None,
            ExprKind::Builtin(_, args) => Some(args),
            _ => None,
        }
    }

    /// A registered invocation carries the Python binder's position and source-ordered values.
    pub(crate) fn is_registered_invoke(&self) -> bool {
        let ExprKind::Invoke { callable, args } = &self.kind else {
            return false;
        };
        self.ty == Type::PythonObject
            && args.site.python().is_some_and(|site| {
                callable.registered_invoke_target(site)
                    && site.range.start() < site.callee_end
                    && site.callee_end < site.range.end()
                    && site.suspended_name.is_none_or(|name| {
                        site.range.contains_range(name) && name.end() == site.callee_end
                    })
                    && site
                        .scope
                        .as_ref()
                        .is_none_or(|scope| scope.ty == Type::PythonObject)
            })
            && args.order.is_empty()
    }

    fn registered_invoke_target(&self, site: &InvokeSite) -> bool {
        if self.ty == Type::PythonObject {
            return true;
        }
        let ExprKind::Builtin(Builtin::RegisteredLocation(range, ..), parts) = &self.kind else {
            return false;
        };
        let [local] = parts.as_slice() else {
            return false;
        };
        matches!(self.ty, Type::Callable(_))
            && local.ty == self.ty
            && matches!(local.kind, ExprKind::Local(_))
            && site.suspended_name == Some(*range)
    }

    pub(crate) fn unrepresented() -> Self {
        Self {
            ty: Type::PythonObject,
            kind: ExprKind::Builtin(Builtin::Unrepresented, vec![]),
        }
    }

    /// Read a local slot with its type at the use site.
    pub(crate) fn local(slot: usize, ty: Type) -> Self {
        Self {
            ty,
            kind: ExprKind::Local(slot),
        }
    }

    /// Read an operand at its original evaluation point in a prepared statement or region.
    pub(crate) fn registered_input(index: usize, ty: Type) -> Self {
        Self {
            ty,
            kind: ExprKind::Builtin(Builtin::RegisteredInput(index), Vec::new()),
        }
    }

    pub(crate) fn unary(op: UnaryOp, operand: Self, ty: Type) -> Self {
        if op == UnaryOp::UAdd && matches!(ty, Type::Int | Type::Float) && operand.ty == ty {
            return operand;
        }
        Self {
            ty,
            kind: ExprKind::Unary(op, Box::new(operand)),
        }
    }

    /// Negate an already checked condition.
    pub(crate) fn not(operand: Self) -> Self {
        Self::unary(UnaryOp::Not, operand, Type::Bool)
    }

    /// A unary operation on an object can run Python and mutate aliased storage.
    pub(crate) fn uses_python_unary_protocol(&self) -> bool {
        matches!(&self.kind, ExprKind::Unary(_, operand)
            if self.ty == Type::PythonObject || operand.ty == Type::PythonObject)
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

    /// Only two native strings can contribute fragments without running another operation.
    pub(crate) fn string_addends(&self) -> Option<(&Self, &Self)> {
        if let ExprKind::Binary(Binary::Add, left, right) = &self.kind
            && self.ty == Type::Str
            && left.ty == Type::Str
            && right.ty == Type::Str
        {
            Some((left, right))
        } else {
            None
        }
    }

    pub(crate) fn is_string_concat(&self) -> bool {
        self.string_addends().is_some()
            || matches!(self.kind, ExprKind::Builtin(Builtin::FormatJoin, _))
    }

    /// The freshly allocated list has no externally visible alias during argument evaluation.
    pub(crate) fn is_nonempty_list_literal(&self) -> bool {
        matches!(&self.kind, ExprKind::List(items) if !items.is_empty())
    }

    /// Read the operation beneath its diagnostic source positions.
    pub(crate) fn without_location(&self) -> &Self {
        let mut value = self;
        while let ExprKind::Builtin(Builtin::RegisteredLocation(..), args) = &value.kind {
            let [inner] = args.as_slice() else { break };
            value = inner;
        }
        value
    }

    /// Return a literal integer that fits the native tagged representation.
    pub(crate) fn tagged_integer_literal(&self) -> Option<i64> {
        let value = match &self.kind {
            ExprKind::Int(value) => value.parse::<i64>().ok()?,
            ExprKind::Unary(..) if self.uses_python_unary_protocol() => return None,
            ExprKind::Unary(UnaryOp::USub, value) => {
                value.tagged_integer_literal()?.checked_neg()?
            }
            _ => return None,
        };
        tagged_integer(value)
    }

    /// Split an integer index into its local and the literal offset added to it.
    pub(crate) fn local_integer_offset(&self) -> Option<(usize, i64)> {
        if self.ty != Type::Int {
            return None;
        }
        match &self.kind {
            ExprKind::Local(slot) => Some((*slot, 0)),
            ExprKind::Binary(Binary::Add, left, right) => match (&left.kind, &right.kind) {
                (ExprKind::Local(slot), _) => Some((*slot, right.tagged_integer_literal()?)),
                (_, ExprKind::Local(slot)) => Some((*slot, left.tagged_integer_literal()?)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Construct a sequence length, counting regex runs directly when the list cannot escape.
    pub(crate) fn sequence_len(mut values: Vec<Self>) -> Self {
        let kind = match values.as_mut_slice() {
            [
                Self {
                    kind:
                        ExprKind::Builtin(
                            Builtin::RegexCollect {
                                program: imre_regex::Collection::Runs(members),
                                split,
                            },
                            input,
                        ),
                    ..
                },
            ] if members.len() >= 256
                && members.len().is_multiple_of(4)
                && matches!(input.as_slice(), [text] if text.ty == Type::Str) =>
            {
                ExprKind::Builtin(
                    Builtin::RegexRunCount {
                        members: std::mem::take(members),
                        split: *split,
                    },
                    std::mem::take(input),
                )
            }
            _ => ExprKind::Builtin(Builtin::Len, values),
        };
        Self {
            ty: Type::Int,
            kind,
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
    pub(crate) fn selected_split_args(&self, index: &Self) -> Option<(bool, [&Self; 2])> {
        let ExprKind::Builtin(Builtin::Text(TextOperation::Split(reverse)), args) = &self.kind
        else {
            return None;
        };
        let [source, separator, limit] = args.as_slice() else {
            return None;
        };
        let ExprKind::Present(separator) = &separator.kind else {
            return None;
        };
        let selected = if *reverse {
            matches!(&index.kind, ExprKind::Unary(UnaryOp::USub, one) if matches!(&one.kind, ExprKind::Int(n) if n == "1"))
        } else {
            matches!(&index.kind, ExprKind::Int(n) if n == "0")
        };
        (selected && matches!(&limit.kind, ExprKind::Int(n) if n == "1"))
            .then_some((*reverse, [source, separator.as_ref()]))
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
        ExprKind::Unary(UnaryOp::USub, value) => {
            let ExprKind::Int(value) = &value.kind else {
                return None;
            };
            value.parse::<i64>().ok()?.checked_neg()?
        }
        _ => return None,
    };
    (-(1_i64 << 62)..0).contains(&value).then_some(value)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayKind {
    List,
    Tuple,
    Set,
}

/// The Python call position and, for an inline comprehension, its lexical namespace.
#[derive(Clone)]
pub(crate) struct InvokeSite {
    pub range: ruff_text_size::TextRange,
    pub callee_end: ruff_text_size::TextSize,
    pub suspended_name: Option<ruff_text_size::TextRange>,
    pub scope: Option<Box<Expr>>,
}

/// Immutable code identity shared by the positioned operations in one fixed activation.
pub(crate) struct FixedFrame {
    pub filename: String,
    pub function: ruff_python_ast::name::Name,
}

#[derive(Clone)]
pub(crate) struct FixedOrigin {
    pub frame: std::sync::Arc<FixedFrame>,
    pub line: u32,
    /// Byte columns for a single source line: start, delimiter and end.
    pub span: Option<[u32; 3]>,
}

#[derive(Clone, Default)]
pub(crate) struct FieldFormat {
    pub repr_methods: Box<[usize]>,
    pub origin: Option<Box<FixedOrigin>>,
}

/// A fused split-field conversion retains the origins of both checked operations.
#[derive(Clone)]
pub(crate) struct SplitFieldOrigins {
    pub index: Option<Box<FixedOrigin>>,
    pub conversion: Option<Box<FixedOrigin>>,
}

/// The checked position belongs to the call's fixed or Python invocation path.
#[derive(Clone)]
pub(crate) enum CallSite {
    None,
    Fixed(FixedOrigin),
    /// A retained local carries its loaded function identity into this invocation.
    SavedFunction(FixedOrigin, usize),
    Python(InvokeSite),
}

impl CallSite {
    pub(crate) fn fixed(&self) -> Option<&FixedOrigin> {
        match self {
            Self::Fixed(site) | Self::SavedFunction(site, _) => Some(site),
            _ => None,
        }
    }

    pub(crate) fn python(&self) -> Option<&InvokeSite> {
        match self {
            Self::Python(site) => Some(site),
            _ => None,
        }
    }
}

/// Values are evaluated and rooted in source order. `order` is a permutation of
/// their indices that supplies the native call's parameter order without
/// reevaluation. Methods include their receiver first; constructors omit the
/// implicit receiver, which is allocated only after these values are evaluated.
#[derive(Clone)]
pub(crate) struct CallArgs {
    pub site: CallSite,
    /// The callee's nominal tag feeds a complete member dispatch at this site.
    pub exhaustive_enum_match: bool,
    pub values: Vec<Expr>,
    /// Native formal order. Python binds the source-ordered values itself.
    pub order: Vec<usize>,
}

impl CallArgs {
    /// The binder maps each formal to its unique operand. Move whole operands,
    /// following each permutation cycle once, after their effects have been held.
    pub(crate) fn into_formal_order(mut self) -> Vec<Expr> {
        debug_assert_eq!(self.values.len(), self.order.len());
        for start in 0..self.order.len() {
            let mut index = start;
            loop {
                let next = std::mem::replace(&mut self.order[index], index);
                if next == start {
                    break;
                }
                self.values.swap(index, next);
                index = next;
            }
        }
        self.values
    }

    /// Uses the existing positional order for calls that need no argument binding.
    pub(crate) fn positional(values: Vec<Expr>) -> Self {
        Self::positional_at(values, CallSite::None)
    }

    pub(crate) fn positional_at(values: Vec<Expr>, site: CallSite) -> Self {
        let order = if site.python().is_some() {
            Vec::new()
        } else {
            (0..values.len()).collect()
        };
        Self {
            site,
            exhaustive_enum_match: false,
            order,
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
    Dict(Vec<DictEntry>),
    Set {
        element: Type,
        values: Vec<Expr>,
    },
    Tuple(Vec<Expr>),
    Present(Box<Expr>),
    Absent(Box<Expr>),
    Unwrap(Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Compare(Vec<Expr>, Vec<CmpOp>),
    Boolean(bool, Vec<Expr>),
    /// Returns the short-circuit operand, narrowed by its selected truth value,
    /// or the RHS. The operands are evaluated once and retain their identities.
    BooleanValue(bool, Box<Expr>, Box<Expr>, Option<TruthPredicate>),
    /// Only the selected arm runs; both arms have the expression's storage type.
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Unary(UnaryOp, Box<Expr>),
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
    Power,
    MatrixMultiply,
}

impl From<ruff_python_ast::Operator> for Binary {
    fn from(op: ruff_python_ast::Operator) -> Self {
        use ruff_python_ast::Operator;
        match op {
            Operator::Add => Self::Add,
            Operator::Sub => Self::Subtract,
            Operator::Mult => Self::Multiply,
            Operator::Div => Self::Divide,
            Operator::FloorDiv => Self::FloorDivide,
            Operator::Mod => Self::Modulo,
            Operator::BitAnd => Self::BitAnd,
            Operator::BitOr => Self::BitOr,
            Operator::BitXor => Self::BitXor,
            Operator::LShift => Self::LeftShift,
            Operator::RShift => Self::RightShift,
            Operator::Pow => Self::Power,
            Operator::MatMult => Self::MatrixMultiply,
        }
    }
}

impl Binary {
    pub(crate) fn protocol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::FloorDivide => "//",
            Self::Modulo => "%",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::LeftShift => "<<",
            Self::RightShift => ">>",
            Self::Power => "**",
            Self::MatrixMultiply => "@",
        }
    }
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

impl Compare {
    /// Object results, identity, and membership preserve Python operand semantics.
    pub(crate) fn uses_python_protocol(ty: Type, ops: &[CmpOp]) -> bool {
        ty == Type::PythonObject
            || ops
                .iter()
                .any(|op| matches!(op, CmpOp::Is | CmpOp::IsNot | CmpOp::In | CmpOp::NotIn))
    }

    pub(crate) fn operator(self) -> CmpOp {
        match self {
            Self::Equal => CmpOp::Eq,
            Self::NotEqual => CmpOp::NotEq,
            Self::Less => CmpOp::Lt,
            Self::LessEqual => CmpOp::LtE,
            Self::Greater => CmpOp::Gt,
            Self::GreaterEqual => CmpOp::GtE,
        }
    }

    pub(crate) fn from_operator(op: CmpOp) -> Option<Self> {
        match op {
            CmpOp::Eq => Some(Self::Equal),
            CmpOp::NotEq => Some(Self::NotEqual),
            CmpOp::Lt => Some(Self::Less),
            CmpOp::LtE => Some(Self::LessEqual),
            CmpOp::Gt => Some(Self::Greater),
            CmpOp::GtE => Some(Self::GreaterEqual),
            CmpOp::Is | CmpOp::IsNot | CmpOp::In | CmpOp::NotIn => None,
        }
    }
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

/// Prefixes that rule out every case-converted key in a literal string set.
#[derive(Clone)]
pub(crate) enum SetCasePrefix {
    ExcludedBytes([u64; 2]),
    ThreeByteHashes(u64),
    ThreeByteValues { mask: u64, tags: Vec<u32> },
}

/// Select characters in the given set or below the Unicode scalar threshold.
#[derive(Clone)]
pub(crate) struct CharacterFilter {
    pub(crate) below: u32,
    /// Exact members of a read-only literal set of single-byte strings.
    pub(crate) ascii_keys: Option<[u64; 2]>,
}

impl CharacterFilter {
    /// A byte scan matches exactly when both the keys and threshold select only ASCII.
    pub(crate) fn ascii_table(&self) -> Option<Vec<u8>> {
        let keys = self.ascii_keys?;
        if self.below > 128 {
            return None;
        }
        let mut table = vec![0; 256];
        for (byte, entry) in table[..128].iter_mut().enumerate() {
            let member = keys[byte / 64] & (1_u64 << (byte % 64)) != 0;
            *entry = u8::from((byte as u32) < self.below || member);
        }
        Some(table)
    }
}

/// Operations that need a registered Python frame after the complete graph has
/// been checked. Field formatting can share a frame required by executable
/// text or a socket sendfile call.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ObjectCall {
    ExecText,
    SocketSendfile,
    FormatField,
}

/// Python descriptor operation on a named attribute in the live frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttributeAction {
    Load,
    Store,
    Delete,
    InPlace(&'static str),
}

/// An omitted key expands the value as a mapping at this point in the display.
#[derive(Clone)]
pub(crate) struct DictEntry {
    pub(crate) key: Option<Expr>,
    pub(crate) value: Expr,
}

impl ExprKind {
    pub(crate) fn dict_pairs(entries: Vec<(Expr, Expr)>) -> Self {
        Self::Dict(
            entries
                .into_iter()
                .map(|(key, value)| DictEntry {
                    key: Some(key),
                    value,
                })
                .collect(),
        )
    }
}

/// A type parameter owns its binding and its lazy fields in declaration order.
#[derive(Clone, Debug)]
pub(crate) struct RegisteredTypeParameter<F> {
    pub(crate) binding: String,
    pub(crate) name: String,
    pub(crate) kind: TypeParameterKind,
    pub(crate) fields: Vec<(imre_ast::TypeParameterField, F)>,
}

impl<F> RegisteredTypeParameter<F> {
    pub(crate) fn try_map_fields<T, E>(
        &self,
        mut map: impl FnMut(&F) -> Result<T, E>,
    ) -> Result<RegisteredTypeParameter<T>, E> {
        let mut fields = Vec::with_capacity(self.fields.len());
        for (field, value) in &self.fields {
            fields.push((*field, map(value)?));
        }
        Ok(RegisteredTypeParameter {
            binding: self.binding.clone(),
            name: self.name.clone(),
            kind: self.kind,
            fields,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TypeParameterKind {
    TypeVar,
    TypeVarTuple,
    ParamSpec,
}

/// The name, location, and guard of a checked annotation value.
#[derive(Clone)]
pub(crate) struct AnnotationField {
    pub(crate) name: String,
    pub(crate) range: ruff_text_size::TextRange,
    pub(crate) index: Option<usize>,
}

#[derive(Clone)]
pub(crate) enum NamespaceAnnotation {
    Module {
        index: usize,
    },
    Class {
        site: ruff_text_size::TextSize,
        name: String,
    },
    String {
        name: String,
    },
}

/// Acquisition and adaptation are separate when a generator stores its Python iterator in `.0`.
#[derive(Clone, Copy)]
pub(crate) enum IteratorOp {
    Loop(ruff_text_size::TextRange),
    Async(ruff_text_size::TextRange),
    Get(ruff_text_size::TextRange),
    LoopCursor,
}

impl IteratorOp {
    pub(crate) fn runtime_op(self) -> &'static str {
        match self {
            Self::Loop(_) => "iter",
            Self::Async(_) => "async_iter",
            Self::Get(_) => "get_iter",
            Self::LoopCursor => "loop_cursor",
        }
    }
}

/// Borrow the operands consumed by registered iteration without changing their evaluation order.
pub(crate) enum IterationOperands<'a> {
    Iter(&'a Expr, IteratorOp),
    PairNeedsRow(&'a Expr),
    Truth(&'a Expr),
    Insert(&'a [Expr; 2]),
    Unpack(
        &'a Expr,
        usize,
        Option<usize>,
        Option<ruff_text_size::TextRange>,
    ),
    Namespace(&'a [String], Option<&'a Expr>),
}

impl Builtin {
    pub(crate) fn iteration_operands<'a>(
        &'a self,
        args: &'a [Expr],
    ) -> Option<Result<IterationOperands<'a>, &'static str>> {
        use IterationOperands::*;
        let one = || match args {
            [value] => Ok(value),
            _ => Err("registered iteration operation needs one operand"),
        };
        let two = || {
            args.try_into()
                .map_err(|_| "registered iteration operation needs two operands")
        };
        Some(match self {
            Self::RegisteredIter(op) => one().map(|value| Iter(value, *op)),
            Self::RegisteredPairNeedsRow => one().map(PairNeedsRow),
            Self::Bool => one().map(Truth),
            Self::Append | Self::SetAdd => two().map(Insert),
            Self::RegisteredUnpack {
                count,
                star,
                target,
            } => one().map(|value| Unpack(value, *count, *star, *target)),
            Self::RegisteredNamespace(names) => match args {
                [] | [_] => Ok(Namespace(names, args.first())),
                _ => Err("registered namespace takes at most one parent"),
            },
            _ => return None,
        })
    }
}

/// The generator site acquires its outer iterator in the defining scope.
pub(crate) fn generator_operands(
    scoped: bool,
    args: &[Expr],
) -> Result<(Option<&Expr>, &Expr), &'static str> {
    match (scoped, args) {
        (true, [scope, iterable]) => Ok((Some(scope), iterable)),
        (false, [iterable]) => Ok((None, iterable)),
        _ => Err("registered generator needs its outer iterable and any explicit scope"),
    }
}

/// A call evaluates its callable or method receiver before the argument binder runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegisteredCall {
    /// The outer get position for an immediately indexed partition method call.
    pub(crate) partition_field: Option<(u32, ruff_text_size::TextRange)>,
    pub(crate) range: ruff_text_size::TextRange,
    pub(crate) target: RegisteredCallTarget,
    pub(crate) arguments: RegisteredArguments,
    /// A comprehension supplies its Python namespace before the callable operand.
    pub(crate) explicit_scope: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegisteredCallTarget {
    Callable {
        end: ruff_text_size::TextSize,
        suspended_name: Option<ruff_text_size::TextRange>,
    },
    Method {
        name: ruff_python_ast::name::Name,
        attribute: ruff_text_size::TextRange,
        name_start: ruff_text_size::TextSize,
    },
}

/// Positional values precede keyword values in Python call evaluation order.
/// Packed entries distinguish each `*` iterable and each `**` mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegisteredArguments {
    Positional,
    Keywords {
        positional: usize,
        names: Vec<ruff_python_ast::name::Name>,
    },
    Packed(PackedArguments),
}

/// Source order of the positional items, starred iterables, named keywords and mappings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackedArguments {
    pub(crate) starred: Vec<bool>,
    pub(crate) keywords: Vec<Option<String>>,
}

/// A physical declaration and the layout of its evaluated header operands.
#[derive(Clone)]
pub(crate) enum DefinitionSite {
    Function(ruff_text_size::TextSize),
    Class {
        start: ruff_text_size::TextSize,
        /// The generic parameter frame owns its own base operands.
        arguments: Option<PackedArguments>,
    },
}

impl DefinitionSite {
    pub(crate) fn start(&self) -> ruff_text_size::TextSize {
        match self {
            Self::Function(start) | Self::Class { start, .. } => *start,
        }
    }
}

/// A statically named import that executes in a live Python namespace.
#[derive(Clone)]
pub(crate) enum NamespaceImport {
    Modules(Vec<ModuleImport>),
    From {
        module: String,
        level: u32,
        names: Vec<(String, String)>,
    },
}

#[derive(Clone)]
pub(crate) struct ModuleImport {
    pub(crate) name: String,
    pub(crate) binding: String,
    /// An explicit alias for a dotted name binds the leaf module.
    pub(crate) bind_leaf: bool,
}

/// Suspends the current Python activation after evaluating one complete operand.
#[derive(Clone, Copy)]
pub(crate) enum SuspendValue {
    Yield,
    Delegate,
    Await,
}

/// Identity of an immutable subscription operand within one compiled Python source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Constant {
    None,
    Ellipsis,
    Bool(bool),
    Int(String),
    Float(u64),
    Complex(u64, u64),
    Text(Vec<u8>),
    Bytes(Vec<u8>),
    Tuple(Vec<Self>),
    Slice(Box<[Self; 3]>),
}

impl Constant {
    pub(crate) fn slice(bounds: [Self; 3]) -> Self {
        Self::Slice(Box::new(bounds))
    }

    pub(crate) fn text(text: &str) -> Self {
        Self::Text(
            text.chars()
                .flat_map(|c| u32::from(c).to_le_bytes())
                .collect(),
        )
    }

    /// A constant key contains only immutable literals and distinguishes Python types and float bits.
    pub(crate) fn from_expr(value: &Expr) -> Option<Self> {
        Self::read_expr::<true>(value)
    }

    /// Check whether the expression can supply a constant key without copying its value.
    pub(crate) fn is_constant(value: &Expr) -> bool {
        Self::read_expr::<false>(value).is_some()
    }

    fn read_expr<const OWNED: bool>(value: &Expr) -> Option<Self> {
        // Recognition needs only the outer Option. Its successful value can be any constant.
        fn retain<const OWNED: bool>(make: impl FnOnce() -> Constant) -> Constant {
            if OWNED { make() } else { Constant::None }
        }
        Some(match &value.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(..), args) => {
                return Self::read_expr::<OWNED>(&args[0]);
            }
            ExprKind::Builtin(Builtin::RegisteredIndexConstant(key), _) => {
                retain::<OWNED>(|| key.clone())
            }
            ExprKind::None => Constant::None,
            ExprKind::Bool(b) => Constant::Bool(*b),
            ExprKind::Int(n) => retain::<OWNED>(|| Constant::Int(n.clone())),
            ExprKind::Float(n) => Constant::Float(n.to_bits()),
            ExprKind::Bytes(b) => retain::<OWNED>(|| Constant::Bytes(b.clone())),
            ExprKind::Str(text) => retain::<OWNED>(|| Constant::text(text)),
            ExprKind::Builtin(Builtin::RegisteredString(points), _) => {
                retain::<OWNED>(|| Constant::Text(points.clone()))
            }
            ExprKind::Builtin(Builtin::RegisteredComplex(r, i), _) => {
                Constant::Complex(r.to_bits(), i.to_bits())
            }
            ExprKind::Builtin(Builtin::RegisteredEllipsis, _) => Constant::Ellipsis,
            ExprKind::Tuple(items) => {
                if !OWNED {
                    return items.iter().all(Self::is_constant).then_some(Self::None);
                }
                Self::Tuple(items.iter().map(Self::from_expr).collect::<Option<_>>()?)
            }
            _ => return None,
        })
    }
}

#[derive(Clone)]
pub(crate) enum Builtin {
    /// An indeterminate operand during source checking. Its containing body retains
    /// the original representation failure and is ineligible for emission.
    Unrepresented,
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
    /// A first-class cursor that owns its current numeric prefix.
    AccumulateCursor,
    SysStdout,
    SysHasStdout,
    TargetLittleEndian,
    SysOriginalOutput {
        stderr: bool,
    },
    SysStdoutSet,
    SysStdoutDelete,
    RequireObjectReceiver,
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
    /// Read the retained nominal class from its checked constructor.
    ConstructorClass,
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
    FunctionWrappedGet(FixedOrigin),
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
    StringTupleContains,
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
    SysMaxSize,
    SysThreadInfo,
    SysThreadText {
        field: u32,
    },
    SysImplementation,
    SysBuiltinModuleNames,
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
    SignalPause,
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
    ObjectCall(ObjectCall, usize, ruff_text_size::TextRange),
    InputSelectStdout,
    InputFinish,
    Chr,
    UnicodePropertyOfOrdinal(UnicodeProperty),
    Ord,
    IntegerPower,
    ModularPower,
    NegativeIntegerPower,
    Int(Option<Box<FixedOrigin>>),
    Float(Option<Box<FixedOrigin>>),
    /// Represents an exact integer by its sign for a proved zero-comparison parameter.
    IntExtremum {
        minimum: bool,
    },
    MathComb,
    MathPerm,
    MathFactorial,
    IntBitCount,
    IntBitLength,
    IntHash,
    FloatIsInteger,
    FloatAcos,
    FloatAsin,
    FloatAtan,
    FloatDegrees,
    FloatFmod,
    FloatRadians,
    FloatErfc,
    Sumprod,
    /// Base-two logarithm after checking the Python domain in the caller.
    FloatLog2,
    FloatLog10,
    FloatLog1p,
    IntLog10,
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
    IsExactTuple,
    ObjectRecordIdentity {
        class: usize,
        reversed: bool,
    },
    MappingObjectGet,
    ExceptionTupleView,
    ExceptionKeywordSuggestion,
    BindSourceKeywords,
    /// A new nominal record whose arguments initialize its complete checked layout.
    RecordFromFields,
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
    /// Base object descriptor on a rooted operand with a statically known builtin class.
    BaseObjectRepr,
    /// object.__repr__ for a rooted source instance, bypassing its __repr__ override.
    DefaultRecordRepr,
    Len,
    LengthResult(Box<FixedOrigin>),
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
    /// Convert the field value before evaluating its nested format specification.
    FormatConversion(char),
    /// Format an already converted value with its checked specification. The
    /// targets are native repr methods that displaying this value can call.
    FormatField(FieldFormat),
    /// Join formatted operands in order, without invoking a str subclass's addition.
    FormatJoin,
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
    ListGet(Option<Box<FixedOrigin>>),
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
    ListGetI32(Option<Box<FixedOrigin>>),
    /// Reads a checked index from the stable buffer in a guarded loop body.
    ListGetStable(Option<Box<FixedOrigin>>),
    /// Internal operations on the spare capacity of a single guarded leaf loop.
    BeginListAppends(usize),
    FinishListAppends(usize),
    AppendInWindow(usize),
    GetInAppendWindow(usize, Option<Box<FixedOrigin>>),
    GetBoundedInAppendWindow {
        output: usize,
        min: i64,
        max: i64,
        origin: Option<Box<FixedOrigin>>,
    },
    /// Every call site and intervening operation preserve these exact element
    /// bounds. The access still checks its index and returns a tagged-small Int.
    ListGetBounded {
        min: i64,
        max: i64,
        origin: Option<Box<FixedOrigin>>,
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
    CursorScanBudget(usize),
    IntFitsI32,
    BoundedCursorValue {
        min: i64,
        max: i64,
    },
    ListPresent,
    StringGet,
    StringIter,
    /// Iterates Python line boundaries on a rooted immutable string.
    SplitLinesIter,
    /// Iterates every field of a forward split by a literal ASCII separator.
    SplitByteIter(u8),
    SortedSplitByteIter(u8),
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
    StringIntList(FixedOrigin),
    /// Materialize exact integer prefixes, optionally starting with an initial value.
    ListAccumulate,
    /// Consume the remaining owned values of an integer cursor into a fresh list.
    CollectIntCursor,
    /// Two nonnegative 31-bit digit subtotals in one immediate, or -1 for fallback.
    TryDigitSums([[u8; 10]; 2]),
    Bytes,
    BytesGet,
    Encode,
    Decode,
    DecodeAscii,
    DecodeLatin1,
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
    PointerIdentity,
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
    /// Read an unsigned big-endian integer from exact immutable bytes.
    IntFromBytes,
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
    SplitIntField(u32, Box<SplitFieldOrigins>),
    /// Prepare small decimal fields from one private immutable split source.
    SplitIntSource(u8),
    SplitIntCached(u32, Box<SplitFieldOrigins>),
    SplitTextSource(u8),
    SplitTextLen,
    SplitTextField(u8, Option<Box<FixedOrigin>>),
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
    FilterCharacterSet(CharacterFilter),
    /// Stable sort by the suffix after a fixed number of Unicode code points.
    StringListSortSuffix(u32),
    /// Ask whether a source has an item before evaluating a comprehension predicate.
    StringListNonempty,
    RStrip,
    SysArgv,
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
    /// Emit the ordered child suite bound at this index in a prepared control region.
    RegisteredSuite(usize),
    /// The owning checked statement or region supplies this deferred operand.
    RegisteredInput(usize),
    /// Evaluate each operand for its effects and release it before evaluating the next.
    RegisteredDiscard,
    /// Pass a completed interactive expression to the current sys.displayhook.
    RegisteredDisplayHook,
    /// The source position of this operand and its optional checked scalar type.
    RegisteredLocation(
        ruff_text_size::TextRange,
        ruff_text_size::TextSize,
        Option<Type>,
    ),
    /// Construct the native function at this physical definition site from its evaluated header.
    RegisteredDefinition(DefinitionSite),
    /// Evaluate packed bases in this generic header, then construct its prepared class namespace.
    RegisteredClassNamespace(ruff_text_size::TextSize, PackedArguments),
    /// Bind the prepared lambda at this site. An explicit lexical scope, when present,
    /// precedes the default values in Python parameter order.
    RegisteredLambda(ruff_text_size::TextSize, bool),
    /// Evaluate the outer iterable after any explicit scope and acquire the child's cursor.
    RegisteredGenerator(ruff_text_size::TextRange, bool, IteratorOp),
    /// Construct a lazy alias whose value callable was checked at this declaration site.
    RegisteredTypeAlias(ruff_text_size::TextSize),
    RegisteredImport(NamespaceImport),
    RegisteredSuspend(SuspendValue, ruff_text_size::TextRange),
    /// Raise AssertionError at this site after evaluating its optional message.
    RegisteredAssertionFailure {
        statement: ruff_text_size::TextSize,
        test: ruff_text_size::TextRange,
    },
    RegisteredNamespaceAnnotation(NamespaceAnnotation),
    RegisteredAnnotationFormat,
    /// The lazy provider's field metadata and its checked value operands share one order.
    RegisteredAnnotations {
        fields: Box<[AnnotationField]>,
    },
    /// Bind the string operand to a checked child function in this provider.
    BindAnnotationExpression {
        site: ruff_text_size::TextSize,
        spelling: String,
    },
    /// Delete one current-scope binding, reporting a missing name at its source target.
    RegisteredDeleteName(String, ruff_text_size::TextRange),
    RegisteredCall(RegisteredCall),
    /// Resolve the code name in the live registered Python scope.
    RegisteredName(String),
    /// Apply Python's in-place protocol to the previously loaded target and the RHS.
    RegisteredInPlace(&'static str),
    /// Store evaluates the RHS before the receiver; in-place retains the receiver across the RHS.
    RegisteredAttribute {
        action: AttributeAction,
        name: String,
        range: ruff_text_size::TextRange,
    },
    /// In source order, each pair consumes a key and value, and each unpack one mapping.
    /// The registered frame owns these operands through dictionary construction.
    /// A starred operand consumed by its enclosing registered list or tuple display.
    RegisteredSpread,
    /// Object and key operands, with the original subscript and end of its receiver.
    RegisteredSubscript(ruff_text_size::TextRange, ruff_text_size::TextSize),
    /// Two bounds represent a simple slice. A third operand is its explicit stride.
    RegisteredSlice,
    RegisteredIndexConstant(Constant),
    RegisteredEllipsis,
    RegisteredComplex(f64, f64),
    /// Little-endian Python code points preserve non-scalar values and linked literal identity.
    RegisteredString(Vec<u8>),
    /// Construct a template from its literal strings tuple and its evaluated interpolations tuple.
    RegisteredTemplate,
    /// Capture the evaluated value and format spec without formatting the value.
    RegisteredInterpolation {
        expression: String,
        conversion: Option<char>,
    },
    /// Acquire or adapt an iterator through its corresponding Python protocol.
    RegisteredIter(IteratorOp),
    /// Whether two-target binding needs an ordinary row from this acquired cursor.
    RegisteredPairNeedsRow,
    /// Snapshot a sequence for ordered target stores, including the optional starred remainder.
    RegisteredUnpack {
        count: usize,
        star: Option<usize>,
        target: Option<ruff_text_size::TextRange>,
    },
    /// Allocate a child of the supplied Python namespace, declaring its private target names.
    RegisteredNamespace(Vec<String>),
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
    FractionHash(usize, usize),
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
        spelling: Constant,
        supplied_flags: u16,
        pattern: std::sync::Arc<imre_regex::FixedPattern>,
    },
    RegexFlags,
    RegexSpelling,
    UnittestCompilePattern,
    RegexSearch,
    RegexSearchText {
        anchored: bool,
        verbose: bool,
    },
    RegexSub(Vec<u8>),
    RegexCollect {
        program: imre_regex::Collection,
        split: bool,
    },
    RegexRunCount {
        members: Vec<u8>,
        split: bool,
    },
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
    /// Delete the evaluated key from its receiver. Each entry uses its native storage.
    DeleteItem,
    DictClear,
    DictUpdate,
    /// False visits keys; true visits key/value pairs without an intermediate tuple.
    DictIter(bool),
    /// Arguments are the element then set, matching membership evaluation order.
    SetContains,
    SetCaseLengthPossible,
    SetCasePrefixPossible(SetCasePrefix),
    SetAdd,
    SetDiscard,
    SetIter,
    FreezeStrings,
    ExceptionMessage,
    ExceptionName,
    JsonErrorField(u32),
    /// Record the source location of a raised Python exception.
    ExceptionOrigin(FixedOrigin),
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
    IsNumeric,
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
            "isnumeric" => Self::IsNumeric,
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
                | Self::IsNumeric
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
            Self::IsNumeric => "isnumeric",
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

/// Character properties shared by checked calls and the native Unicode database.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnicodeProperty {
    Bidirectional,
    Category,
    EastAsianWidth,
}

impl UnicodeProperty {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Bidirectional => "bidirectional",
            Self::Category => "category",
            Self::EastAsianWidth => "east_asian_width",
        }
    }
}

/// Closed string operations preserve exact argument and storage types.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextOperation {
    Predicate(TextPredicate),
    SplitLineEnds,
    ExpandTabs,
    ZFill {
        copy: bool,
    },
    UnicodeProperty(UnicodeProperty),
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

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum UrlProperty {
    Username,
    Password,
    Hostname,
    Port,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextPredicate {
    Alnum,
    Ascii,
    Alpha,
    Digit,
    Numeric,
    Space,
    Lower,
    Title,
    Upper,
}

/// Numeric selectors for the runtime's imre_path_query ABI.
#[derive(Clone, Copy, PartialEq, Eq)]
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
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum PathLink {
    Symlink = 0,
    CheckSymlink = 1,
    Hardlink = 2,
}

/// Native path operations keep their exact result types in the checked IR.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathOperation {
    Scripts,
    OsExists,
    New,
    PureNew(PathFlavor),
    Drive,
    Root,
    Anchor,
    Parts,
    CountPart,
    AsPosix,
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
            Self::Exact(Type::BuiltinClass) => {
                matches!(
                    ty,
                    Type::BuiltinClass | Type::Placeholder(PlaceholderKind::Class)
                ) || types.constructor_base(ty).is_some()
            }
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
            Self::Tuple => matches!(ty, Type::Tuple(_) | Type::VarTuple(_) | Type::CodecInfo),
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
    pub matcher: ExceptionMatcher,
    pub name: Option<usize>,
    pub body: Vec<Statement>,
}

/// The manager protocol and the context expression's traceback position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextSite {
    pub is_async: bool,
    pub range: ruff_text_size::TextRange,
    pub anchor: ruff_text_size::TextSize,
}

/// The enter result stays live through this protected assignment, before the suite.
#[derive(Clone)]
pub(crate) struct ContextBinding {
    pub entered: usize,
    pub body: Vec<Statement>,
}

#[derive(Clone)]
pub(crate) enum ExceptionMatcher {
    /// Empty for a bare handler; otherwise fixed builtin exception categories.
    Categories(Vec<u32>),
    /// Evaluate the Python class only when a pending exception reaches this handler.
    Object(ruff_text_size::TextRange, Box<Expr>),
    /// Split and detach a subgroup while leaving unmatched exceptions for the next arm.
    Subgroup(ruff_text_size::TextRange, Box<Expr>),
}

/// The implicit await position and optional source statement to restore before each pull.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct AsyncPull {
    pub(crate) range: ruff_text_size::TextRange,
    pub(crate) header: Option<ruff_text_size::TextSize>,
}

/// Selects the pull ABI for a cursor created by the compiler.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IterNextKind {
    Generic,
    Async(AsyncPull),
    /// An owned field from a private single-byte split cursor.
    SplitField,
    String,
    CodePoint,
    /// One unsigned UTF-8 byte. The loop lowerer creates a hidden string
    /// iterator whose only uses are this pull and scope cleanup.
    Utf8Byte,
}

/// Operand evaluation order for a subscript assignment or a dictionary display.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemOrder {
    Assignment,
    Display,
}

impl ItemOrder {
    /// Return the receiver, index and value in their storage roles after evaluation.
    pub(crate) fn evaluate<T, R>(
        self,
        receiver: T,
        index: T,
        value: T,
        mut evaluate: impl FnMut(T) -> R,
    ) -> (R, R, R) {
        match self {
            Self::Assignment => {
                let value = evaluate(value);
                (evaluate(receiver), evaluate(index), value)
            }
            Self::Display => (evaluate(receiver), evaluate(index), evaluate(value)),
        }
    }
}

/// A fixed-layout update has a checked arithmetic operation. A live Python item
/// uses its in-place protocol, including the fallback to the binary operator.
#[derive(Clone, Copy)]
pub(crate) enum ItemUpdate {
    Fixed(Binary),
    Protocol(&'static str),
}

#[derive(Clone)]
pub(crate) enum MatchTest {
    Always,
    Singleton(Option<bool>),
    Equal(Expr),
    Or(Vec<MatchTest>),
    Capture(Box<MatchTest>, String),
    Sequence(imre_ast::SequencePatternPlan, Vec<MatchTest>),
    Mapping(Vec<(Expr, MatchTest)>, Option<String>),
    Class {
        range: ruff_text_size::TextRange,
        class: Expr,
        positional: Vec<MatchTest>,
        keywords: Vec<(String, MatchTest)>,
    },
}

#[derive(Clone)]
pub(crate) struct MatchPattern {
    pub(crate) test: MatchTest,
    pub(crate) names: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct MatchCase {
    pub(crate) pattern: MatchPattern,
    pub(crate) captures: Vec<usize>,
    pub(crate) guard: Option<Expr>,
    pub(crate) body: Vec<Statement>,
}

#[derive(Clone)]
pub(crate) struct RaisedException {
    pub(crate) exception: Expr,
    pub(crate) cause: Option<Box<Expr>>,
    pub(crate) origin: Option<Box<FixedOrigin>>,
}

#[derive(Clone)]
pub(crate) enum Statement {
    /// An original statement checkpoint and its optional call without a separate highlight.
    Position(ruff_text_size::TextRange, Option<ruff_text_size::TextRange>),
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
    SetItem(ItemOrder, Expr, Expr, Expr),
    AugmentItem(Expr, Expr, ItemUpdate, Expr),
    SetField(Expr, usize, Expr),
    AugmentField(Expr, usize, Binary, Expr),
    Expr(Expr),
    If(Expr, Vec<Statement>, Vec<Statement>),
    /// The evaluated subject survives failed guards, with captures bound before the guard.
    Match(Expr, Vec<MatchCase>),
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
    /// Written loop jumps checkpoint their source before unwinding. Synthesized breaks have no site.
    Break(Option<ruff_text_size::TextRange>),
    Continue(ruff_text_size::TextRange),
    /// A compiler-owned iterator reached its end; unlike break, this runs else.
    Exhaust,
    /// A successfully acquired exit protects the target binding and suite.
    With {
        manager: Expr,
        site: ContextSite,
        binding: Option<ContextBinding>,
        body: Vec<Statement>,
    },
    /// Handlers protect only `body`; `finalbody` runs for every exit reason.
    Try {
        body: Vec<Statement>,
        handlers: Vec<ExceptHandler>,
        orelse: Vec<Statement>,
        finalbody: Vec<Statement>,
    },
    /// None re-raises the dynamically active exception, including across calls.
    Raise(Option<RaisedException>),
}

// Visit value patterns in source order with either a shared or a mutable borrow.
macro_rules! pattern_expressions {
    ($test:expr, $visit:expr) => {{
        let mut pending = vec![$test];
        while let Some(test) = pending.pop() {
            match test {
                MatchTest::Equal(value) => ($visit)(value),
                MatchTest::Or(tests) | MatchTest::Sequence(_, tests) => {
                    pending.extend(tests.into_iter().rev())
                }
                MatchTest::Capture(inner, _) => pending.push(inner),
                MatchTest::Mapping(items, _) => {
                    let mut children = Vec::with_capacity(items.len());
                    for (key, child) in items {
                        ($visit)(key);
                        children.push(child);
                    }
                    pending.extend(children.into_iter().rev());
                }
                MatchTest::Class {
                    class,
                    positional,
                    keywords,
                    ..
                } => {
                    ($visit)(class);
                    pending.extend(keywords.into_iter().rev().map(|(_, test)| test));
                    pending.extend(positional.into_iter().rev());
                }
                MatchTest::Always | MatchTest::Singleton(_) => {}
            }
        }
    }};
}

// Match ergonomics preserve the borrow of each operand and suite for inspection or rewriting.
macro_rules! statement_children {
    ($statement:expr, $expression:expr, $suite:expr) => {
        match $statement {
            Statement::Assign(_, value) | Statement::Expr(value) | Statement::Return(value) => {
                ($expression)(value)
            }
            Statement::SetItem(order, receiver, index, value) => {
                order.evaluate(receiver, index, value, $expression);
            }
            Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    ($expression)(value);
                }
            }
            Statement::SetField(receiver, _, rhs) => {
                ($expression)(rhs);
                ($expression)(receiver);
            }
            Statement::AugmentField(a, _, _, b) => {
                ($expression)(a);
                ($expression)(b);
            }
            Statement::With {
                manager,
                binding,
                body,
                ..
            } => {
                ($expression)(manager);
                if let Some(ContextBinding { body, .. }) = binding {
                    ($suite)(body);
                }
                ($suite)(body);
            }
            Statement::Scope { body, .. } => ($suite)(body),
            Statement::Match(subject, cases) => {
                ($expression)(subject);
                for MatchCase {
                    pattern,
                    guard,
                    body,
                    ..
                } in cases
                {
                    let MatchPattern { test, .. } = pattern;
                    pattern_expressions!(test, $expression);
                    if let Some(guard) = guard {
                        ($expression)(guard);
                    }
                    ($suite)(body);
                }
            }
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
                for ExceptHandler { matcher, body, .. } in handlers {
                    if let ExceptionMatcher::Object(_, class)
                    | ExceptionMatcher::Subgroup(_, class) = matcher
                    {
                        ($expression)(class);
                    }
                    ($suite)(body);
                }
                ($suite)(orelse);
                ($suite)(finalbody);
            }
            Statement::Raise(Some(RaisedException {
                exception, cause, ..
            })) => {
                ($expression)(exception);
                if let Some(cause) = cause {
                    ($expression)(cause);
                }
            }
            Statement::Position(..)
            | Statement::Move(..)
            | Statement::Clear(_)
            | Statement::Raise(None)
            | Statement::IterNext { .. }
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Exhaust => {}
        }
    };
}

impl Statement {
    pub(crate) fn assign_item(receiver: Expr, key: Expr, value: Expr) -> Self {
        Self::SetItem(ItemOrder::Assignment, receiver, key, value)
    }

    pub(crate) fn store_global(slot: usize, value: Expr) -> Self {
        Self::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::GlobalSet(slot), vec![value]),
        })
    }

    pub(crate) fn raise(exception: Expr) -> Self {
        Self::raise_at(exception, None, None)
    }

    pub(crate) fn raise_at(
        exception: Expr,
        cause: Option<Expr>,
        origin: Option<FixedOrigin>,
    ) -> Self {
        Self::Raise(Some(RaisedException {
            exception,
            cause: cause.map(Box::new),
            origin: origin.map(Box::new),
        }))
    }

    /// Visit immediate operands, including the subject, value patterns and guards of a match.
    pub(crate) fn operands<'a>(&'a self, visit: impl FnMut(&'a Expr)) {
        let mut visit = visit;
        statement_children!(self, &mut visit, |_| {});
    }

    /// Visits nested suites in source order, excluding suites owned by expressions.
    pub(crate) fn suites<'a>(&'a self, mut visit: impl FnMut(&'a [Statement])) {
        statement_children!(self, |_| {}, visit);
    }
}

pub(crate) enum Node<'a> {
    Statement(&'a Statement),
    Expression(&'a Expr),
}

/// Visits every IR node together with its immediate containing expression, when present.
pub(crate) fn walk_nodes<'a>(
    body: &'a [Statement],
    visit: &mut impl FnMut(Node<'a>, Option<&'a Expr>),
) {
    all_nodes(body, &mut |node, parent| {
        visit(node, parent);
        true
    });
}

/// Visit a suite until its caller rejects an operation.
pub(crate) fn all_nodes<'a>(
    body: &'a [Statement],
    visit: &mut impl FnMut(Node<'a>, Option<&'a Expr>) -> bool,
) -> bool {
    for statement in body {
        if !visit(Node::Statement(statement), None) {
            return false;
        }
        let mut complete = true;
        statement_children!(
            statement,
            |value| { complete = complete && all_operand_nodes(value, None, visit) },
            |suite| { complete = complete && all_nodes(suite, visit) }
        );
        if !complete {
            return false;
        }
    }
    true
}

/// Visit an operand and its scoped statements until the caller declines one.
pub(crate) fn all_operand_nodes<'a>(
    expression: &'a Expr,
    parent: Option<&'a Expr>,
    visit: &mut impl FnMut(Node<'a>, Option<&'a Expr>) -> bool,
) -> bool {
    if !visit(Node::Expression(expression), parent) {
        return false;
    }
    if let ExprKind::Scope { body, value, .. } = &expression.kind {
        all_nodes(body, visit) && all_operand_nodes(value, Some(expression), visit)
    } else {
        let mut complete = true;
        children(expression, |child| {
            complete = complete && all_operand_nodes(child, Some(expression), visit);
        });
        complete
    }
}

/// Visits all statement nodes once, including those nested inside eager expressions.
pub(crate) fn walk_statements<'a>(body: &'a [Statement], visit: &mut impl FnMut(&'a Statement)) {
    walk_nodes(body, &mut |node, _| {
        if let Node::Statement(statement) = node {
            visit(statement);
        }
    });
}

/// Includes loops in eager expression scopes when deciding whether a suite can be cloned.
pub(crate) fn contains_loop(body: &[Statement]) -> bool {
    let loop_free = all_nodes(body, &mut |node, _| {
        !matches!(
            node,
            Node::Statement(Statement::For { .. } | Statement::While(..))
        )
    });
    !loop_free
}

/// Walk one expression with the same parent and nested-scope information as a suite.
pub(crate) fn walk_operand_nodes<'a>(
    value: &'a Expr,
    visit: &mut impl FnMut(Node<'a>, Option<&'a Expr>),
) {
    all_operand_nodes(value, None, &mut |node, parent| {
        visit(node, parent);
        true
    });
}

/// Visits an operand and its nested expressions, including eager scope bodies.
pub(crate) fn walk_operand_expressions<'a>(value: &'a Expr, visit: &mut impl FnMut(&'a Expr)) {
    walk_operand_nodes(value, &mut |node, _| {
        if let Node::Expression(expression) = node {
            visit(expression);
        }
    });
}

/// Visits expression trees, including statements embedded in eager expressions.
/// Analyses must include every checked branch, even when it never executes.
pub(crate) fn walk_expressions<'a>(body: &'a [Statement], visit: &mut impl FnMut(&'a Expr)) {
    walk_nodes(body, &mut |node, _| {
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
    walk_nodes(&function.body, &mut |node, _| {
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

/// A local keeps a fact only if every value copied into it also has that fact.
/// Copy edges can point backward through a loop or through a moved temporary.
pub(crate) fn propagate_copy_facts(proven: &mut [bool], copies: &[(usize, usize)]) {
    loop {
        let mut changed = false;
        for &(source, target) in copies {
            if !proven[source] && proven[target] {
                proven[target] = false;
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

/// Only a plain function owns the writable third word used by wrapper links.
pub(crate) fn retain_unknown_link_owners(
    function: &Function,
    owners: &mut Vec<&Expr>,
    inputs: &[&CallArgs],
    linked: impl Fn(&Expr) -> bool,
) {
    fn origin(value: &Expr) -> &ExprKind {
        match &value.kind {
            ExprKind::Builtin(Builtin::FunctionWrappedSet, args) => origin(&args[0]),
            ExprKind::Scope { value, .. } => origin(value),
            kind => kind,
        }
    }

    let plain_function = |kind: &ExprKind| matches!(kind, ExprKind::Builtin(Builtin::StaticFunction(_), args) if linked(&args[0]));
    let mut plain = vec![true; function.locals.len()];
    for (index, &parameter) in function.params.iter().enumerate() {
        plain[parameter] = !inputs.is_empty()
            && inputs.iter().all(|args| {
                args.order
                    .get(index)
                    .and_then(|&source| args.values.get(source))
                    .is_some_and(|value| plain_function(origin(value)))
            });
    }
    let mut copies = Vec::new();
    walk_nodes(&function.body, &mut |node, _| match node {
        // Clearing a slot does not give it a new callable.
        Node::Statement(Statement::Clear(_) | Statement::Scope { .. })
        | Node::Expression(Expr {
            kind: ExprKind::Scope { .. },
            ..
        }) => {}
        Node::Statement(Statement::Move(target, source)) => copies.push((*source, *target)),
        _ => node_writes(node, &mut |slot, value| match value.map(origin) {
            Some(ExprKind::Local(source)) => copies.push((*source, slot)),
            Some(kind) if plain_function(kind) => {}
            _ => plain[slot] = false,
        }),
    });
    propagate_copy_facts(&mut plain, &copies);
    owners.retain(|owner| match origin(owner) {
        ExprKind::Local(slot) => !plain[*slot],
        kind => !plain_function(kind),
    });
}

/// Visits every local write in a suite. Only ordinary assignments carry a right-hand side.
pub(crate) fn walk_writes(body: &[Statement], mut visit: impl FnMut(usize, Option<&Expr>)) {
    walk_nodes(body, &mut |node, _| node_writes(node, &mut visit));
}

/// Visits writes during operand evaluation, including assignments in eager scopes.
pub(crate) fn walk_expression_writes(value: &Expr, mut visit: impl FnMut(usize, Option<&Expr>)) {
    walk_operand_nodes(value, &mut |node, _| node_writes(node, &mut visit));
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
                ExprKind::BooleanValue(_, left, _, Some(predicate)) => {
                    visit(predicate.receiver, Some(left));
                    visit(predicate.receiver, None);
                }
                _ => {}
            }
            return;
        }
        Node::Statement(statement) => statement,
    };
    match statement {
        Statement::Assign(slot, value) => visit(*slot, Some(value)),
        Statement::Match(_, cases) => {
            for &slot in cases.iter().flat_map(|case| &case.captures) {
                visit(slot, None);
            }
        }
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
        Statement::With {
            binding: Some(binding),
            ..
        } => visit(binding.entered, None),
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
            ExprKind::Dict(entries) => {
                for DictEntry { key, value } in entries {
                    if let Some(key) = key {
                        $visit(key);
                    }
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
                args: CallArgs { site, values, .. },
            } => {
                if let CallSite::Python(InvokeSite {
                    scope: Some(scope), ..
                }) = site
                {
                    $visit(scope);
                }
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
            | ExprKind::Unary(_, value)
            | ExprKind::Field(value, _) => $visit(value),
            ExprKind::BooleanValue(_, left, right, predicate) => {
                $visit(left);
                if let Some(TruthPredicate { condition, .. }) = predicate {
                    $visit(condition);
                }
                $visit(right);
            }
            ExprKind::Binary(_, left, right) => {
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
