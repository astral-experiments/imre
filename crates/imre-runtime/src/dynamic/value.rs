//! Values shared by native functions loading Python modules through `sys.path`.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;

use super::Context;
use crate::objects::exceptions::ExitStatus;

pub(super) type Id = u64;
pub(super) type Code = unsafe extern "C" fn(*mut Context, Id) -> Id;

/// Immutable executable and argument layout shared by functions from one Python definition.
#[derive(Clone)]
pub(super) struct CodeObject {
    pub body: Body,
    pub parameters: Id,
    pub name: Id,
    pub qualname: Id,
    pub doc: Id,
    pub filename: Id,
    pub first_line: u32,
    pub module_definition: bool,
    pub coroutine: bool,
    pub nested: Vec<(usize, Id)>,
    pub freevars: Vec<String>,
    pub closure_supported: bool,
}

#[derive(Clone)]
pub(super) enum Body {
    Native(Code),
    Source(std::rc::Rc<super::source_code::Program>),
    PendingGenerator {
        source: &'static str,
        body_line: u32,
    },
}

#[derive(Clone, Copy)]
pub(super) struct Hmac {
    pub inner: Id,
    pub outer: Id,
    pub digest_size: Id,
    pub block_size: Id,
}
pub(super) const NONE: Id = 0;
pub(super) const FALSE: Id = 1;
pub(super) const TRUE: Id = 2;
pub(super) const ELLIPSIS: Id = 3;
pub(super) const NOT_IMPLEMENTED: Id = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Location {
    pub file: std::borrow::Cow<'static, str>,
    pub line: u32,
    pub span: Option<[u32; 3]>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Trace {
    pub frames: Vec<(Location, Id)>,
    pub instance: Option<Id>,
    pub context: Option<Id>,
    pub cause: Option<Id>,
    pub import_message: Option<Id>,
    /// Live members belong to the exception instance across traceback clones.
    pub source_syntax: Option<crate::shared::Shared<[std::cell::Cell<Id>; 9]>>,
    pub import_names: Option<[Id; 3]>,
    pub dictionary: Option<Id>,
    stop_value: Id,
    cached: Option<(usize, Id)>,
}

#[derive(Debug)]
pub(super) struct ImportFields {
    pub name: Option<String>,
    pub path: Option<String>,
    pub name_from: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Error {
    pub kind: String,
    pub class: Option<Id>,
    pub message: String,
    pub args: Option<Id>,
    pub decode: Option<Box<DecodeState>>,
    pub os_args: Option<crate::shared::Shared<super::os_error::Args>>,
    pub trace: Option<allocator_api2::boxed::Box<Trace>>,
    pub exit_status: Option<ExitStatus>,
    pub import: Option<crate::shared::Shared<ImportFields>>,
}

/// The decoder reuses these writable fields while BaseException.args keeps its first tuple.
#[derive(Clone, Debug)]
pub(super) struct DecodeState {
    pub encoding: Id,
    pub object: Id,
    pub start: i64,
    pub end: i64,
    pub reason: Id,
}

impl Error {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            class: None,
            message: message.into(),
            args: None,
            decode: None,
            os_args: None,
            trace: None,
            exit_status: None,
            import: None,
        }
    }

    /// Attach the module and member names exposed on a Python import exception.
    pub fn with_import(
        mut self,
        name: Option<&str>,
        path: Option<String>,
        name_from: Option<&str>,
    ) -> Self {
        let fields = ImportFields {
            name: name.map(str::to_owned),
            path,
            name_from: name_from.map(str::to_owned),
        };
        let Ok(fields) = crate::shared::Shared::try_new(fields) else {
            return Self::new("MemoryError", "");
        };
        self.import = Some(fields);
        self
    }

    /// Read the independent StopIteration value without allocating exception metadata.
    pub(super) fn stop_value(&self) -> Id {
        self.trace.as_ref().map_or(NONE, |trace| trace.stop_value)
    }

    /// Store the value in the cold payload, which owns its reference during collection.
    pub(super) fn set_stop_value(&mut self, value: Id) {
        if value != NONE {
            self.tracing().stop_value = value;
        } else if let Some(trace) = self.trace.as_mut() {
            trace.stop_value = NONE;
        }
    }

    pub(super) fn roots(&self) -> impl Iterator<Item = Id> + '_ {
        self.args
            .into_iter()
            .chain(self.class)
            .chain(
                self.decode
                    .iter()
                    .flat_map(|d| [d.encoding, d.object, d.reason]),
            )
            .chain(self.os_args.iter().flat_map(|args| args.roots()))
            .chain(self.trace.iter().flat_map(|trace| {
                trace
                    .frames
                    .iter()
                    .map(|&(_, frame)| frame)
                    .chain((trace.stop_value != NONE).then_some(trace.stop_value))
                    .chain(trace.instance)
                    .chain(trace.context)
                    .chain(trace.cause)
                    .chain(trace.import_message)
                    .chain(
                        trace
                            .source_syntax
                            .iter()
                            .flat_map(|fields| fields.iter().map(std::cell::Cell::get)),
                    )
                    .chain(trace.import_names.into_iter().flatten())
                    .chain(trace.dictionary)
                    .chain(trace.cached.map(|(_, head)| head))
            }))
    }

    pub(super) fn tracing(&mut self) -> &mut Trace {
        self.trace.get_or_insert_with(Default::default)
    }
}

impl Context {
    /// Reuse the Python exception object when an already observed error is raised again.
    pub(super) fn raised_instance(&mut self, mut error: Error) -> Id {
        if let Some(id) = error.trace.as_mut().and_then(|trace| trace.instance.take()) {
            *self.heap.get_mut(id) = Value::Raised(error);
            id
        } else {
            self.alloc(Value::Raised(error))
        }
    }

    pub(super) fn record_exception_frame(&mut self, error: &mut Error, span: Option<&[u32; 4]>) {
        if let Some((mut loc, frame)) = self.heap.trace_location() {
            if let Some(&[line, start, split, end]) = span {
                loc.line = line;
                loc.span = Some([start, split, end]);
            }
            let trace = error.tracing();
            let active = self.heap.active_exception();
            if trace.frames.is_empty() && active != NONE && trace.instance != Some(active) {
                trace.context = Some(active);
            }
            trace.frames.push((loc, frame));
        }
    }

    /// Preserve identity of an observed traceback head and reuse its tail on a later raise.
    pub(super) fn exception_traceback(&mut self, exception: Id) -> Result<Id> {
        let Value::Raised(error) = self.heap.get(exception) else {
            return Err(Error::new("TypeError", "expected exception"));
        };
        let Some(trace) = &error.trace else {
            return Ok(NONE);
        };
        let (count, mut head) = trace.cached.unwrap_or((0, NONE));
        let length = trace.frames.len();
        if length != count {
            let missing = trace.frames[count..].to_vec();
            for (at, frame) in missing {
                head = self.alloc(Value::Traceback {
                    frame,
                    next: head,
                    lineno: i64::from(at.line),
                    span: at.span,
                });
            }
            let Value::Raised(error) = self.heap.get_mut(exception) else {
                unreachable!()
            };
            error.tracing().cached = Some((length, head));
        }
        Ok(head)
    }

    pub(super) fn type_name(&self, value: Id) -> &str {
        match self.heap.get(value) {
            Value::None => "NoneType",
            Value::Weak(wr) => wr.kind.class(),
            Value::Ellipsis => "ellipsis",
            Value::NotImplemented => "NotImplementedType",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Complex(_) => "complex",
            Value::Calendar { value, .. } => value.class(),
            Value::Str(_) => "str",
            Value::Map { .. } => "map",
            Value::Filter { .. } => "filter",
            Value::Zip { .. } => "zip",
            Value::Count { .. } => "count",
            Value::Repeat(_) => "repeat",
            Value::Bytes(_) => "bytes",
            Value::ByteArray(_) => "bytearray",
            Value::File(file) => match file.as_ref() {
                super::stdlib::BinaryFile::Reader { .. } => "_io.BufferedReader",
                super::stdlib::BinaryFile::Writer(_) => "_io.BufferedWriter",
            },
            Value::TextFile(_) => "_io.TextIOWrapper",
            Value::CodecInfo { .. } => "CodecInfo",
            Value::Struct { .. } => "Struct",
            Value::BufferedRandom(_) => "_io.BufferedRandom",
            Value::ReverseSequence { .. } => "reversed",
            Value::Deque(_) => super::deque::CLASS,
            Value::DequeIterator { reverse: true, .. } => super::deque::REVERSE_ITERATOR,
            Value::DequeIterator { .. } => super::deque::ITERATOR,
            Value::List(_) => "list",
            Value::CallableIterator(_) => "callable_iterator",
            Value::NamedTuple {
                class: Some(class), ..
            } => class,
            Value::Tuple(_) | Value::NamedTuple { .. } => "tuple",
            Value::Slice(_) => "slice",
            Value::Range(_) => "range",
            Value::RangeIterator(iter) => iter.kind,
            Value::Enumerate { .. } => "enumerate",
            Value::Iterator { kind, .. } => kind,
            Value::MappingViewIterator { .. } => "generator",
            Value::DictIterator(iter) => match (iter.order_state, iter.reverse, iter.kind) {
                (Some(_), _, _) => "odict_iterator",
                (_, false, DictViewKind::Keys) => "dict_keyiterator",
                (_, false, DictViewKind::Values) => "dict_valueiterator",
                (_, false, DictViewKind::Items) => "dict_itemiterator",
                (_, true, DictViewKind::Keys) => "dict_reversekeyiterator",
                (_, true, DictViewKind::Values) => "dict_reversevalueiterator",
                (_, true, DictViewKind::Items) => "dict_reverseitemiterator",
            },
            Value::Set { frozen: true, .. } => "frozenset",
            Value::Set { frozen: false, .. } => "set",
            Value::Dict(_) => "dict",
            Value::MappingProxy(_) => "mappingproxy",
            Value::DefaultDict { .. } => "defaultdict",
            Value::Function { .. } => "function",
            Value::ItemGetter(_) => "operator.itemgetter",
            Value::Code(_) => "code",
            Value::Cell(_) => "cell",
            Value::Generator { .. } => "generator",
            Value::TypeVar(_) => "TypeVar",
            Value::GenericAlias(alias) => alias.kind.type_name(),
            Value::Union(_) => "Union",
            Value::Class { .. } => "type",
            Value::MappingView { kind, .. } => match kind {
                DictViewKind::Keys => "KeysView",
                DictViewKind::Values => "ValuesView",
                DictViewKind::Items => "ItemsView",
            },
            Value::Method { .. } => "method",
            Value::ClassMethod(_) => "classmethod",
            Value::ThreadLocal(_) => super::thread_local::CLASS,
            Value::PrimitiveLock(_) => super::thread_lock::CLASS,
            Value::FormatIterator { kind, .. } => match kind {
                FormatKind::Markup => "formatteriterator",
                FormatKind::Field => "fieldnameiterator",
            },
            Value::Partial { .. } => "partial",
            Value::PartialPlaceholder => "_PlaceholderType",
            Value::StaticMethod { .. } => "staticmethod",
            Value::Property(_) => "property",
            Value::GetSetDescriptor { .. } | Value::WeakrefDescriptor(_) => "getset_descriptor",
            Value::Raised(error) => &error.kind,
            Value::Traceback { .. } => "traceback",
            Value::PythonFrame { .. } => "frame",
            Value::Module {
                constructor: None, ..
            } => "module",
            Value::Instance { class, .. }
            | Value::Module {
                constructor: Some(class),
                ..
            } => match self.heap.get(*class) {
                Value::Class { .. } | Value::Native(_) => self.class_name(*class),
                _ => "object",
            },
            _ => "object",
        }
    }

    /// Return the exact arguments retained by a Python exception.
    pub(super) fn exception_arguments(&self, error: &Error) -> Option<&[Id]> {
        let Value::Tuple(args) = self.heap.get(error.args?) else {
            unreachable!("exception arguments must be an exact tuple")
        };
        Some(args)
    }

    pub(super) fn lookup_error(&mut self, kind: &str, key: Id) -> Error {
        let mut error = Error::new(kind, self.repr(key));
        error.args = Some(self.alloc(Value::Tuple(vec![key])));
        error
    }

    /// Keep the decoder's original object and offsets when crossing into boxed Python.
    pub(super) fn decode_error(&mut self, native: crate::Error) -> Error {
        let decoded = native.decode_args().map(|args| {
            let encoding = self.text(args.encoding);
            let object = self.alloc(Value::Bytes(args.object.to_vec()));
            let reason = self.text(args.reason);
            let values = vec![
                encoding,
                object,
                self.int(args.start),
                self.int(args.end),
                reason,
            ];
            let state = DecodeState {
                encoding,
                object,
                start: args.start,
                end: args.end,
                reason,
            };
            (self.alloc(Value::Tuple(values)), state)
        });
        let mut error = Error::from(native);
        if error.kind == "UnicodeDecodeError"
            && let Some((values, state)) = decoded
        {
            error.args = Some(values);
            error.decode = Some(Box::new(state));
        }
        error
    }

    /// A subsequent failure on the same decode updates only the byte range and reason.
    pub(super) fn advance_decode_error(
        &mut self,
        exception: Id,
        start: usize,
        end: usize,
        reason: &str,
    ) {
        let reason = self.text(reason);
        let Value::Raised(Error {
            decode: Some(state),
            ..
        }) = self.heap.get_mut(exception)
        else {
            unreachable!("active decoder exception");
        };
        state.start = start as i64;
        state.end = end as i64;
        state.reason = reason;
    }

    pub(super) fn exception(&mut self, kind: &str, args: &[Id]) -> Error {
        let mut error = Error::new(kind, "");
        error.args = Some(self.alloc(Value::Tuple(args.to_vec())));
        if kind == "StopIteration" {
            error.set_stop_value(args.first().copied().unwrap_or(NONE));
        }
        error.message = self.exception_message(&error);
        if kind == "SystemExit" {
            error.exit_status = match args {
                [] | [NONE] => Some(ExitStatus::Code(0)),
                [code] => match self.heap.get(*code) {
                    Value::Int(i) => Some(ExitStatus::from_i64(i.to_i64())),
                    Value::Bool(b) => Some(ExitStatus::Code(i32::from(*b))),
                    _ => None,
                },
                _ => None,
            };
        }
        error
    }
}

impl From<crate::Error> for Error {
    fn from(error: crate::Error) -> Self {
        let os_args = match error
            .os_args()
            .map(|args| {
                super::os_error::Args::new(super::os_error::Args::Native {
                    source: args,
                    fields: std::cell::Cell::new(None),
                })
            })
            .transpose()
        {
            Ok(args) => args,
            Err(error) => return error,
        };
        let (kind, message) = error.into_parts();
        match message {
            Ok(message) => Self {
                os_args,
                ..Self::new(kind, message.to_string())
            },
            Err(_) => Self::new("MemoryError", String::new()),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(trace) = &self.trace
            && !trace.frames.is_empty()
        {
            writeln!(f, "Traceback (most recent call last):")?;
            for (loc, _) in trace.frames.iter().rev() {
                writeln!(f, "  File {:?}, line {}", loc.file, loc.line)?;
            }
        }
        if self.message.is_empty() {
            f.write_str(&self.kind)
        } else {
            write!(f, "{}: {}", self.kind, self.message)
        }
    }
}

pub(super) type Result<T> = std::result::Result<T, Error>;

/// Data retained by class namespaces after their body has run.
#[derive(Clone, Default)]
pub(super) struct ClassNamespace {
    pub bases: Id,
    pub mro: Id,
    /// A name assigned through type's __name__ descriptor, including a str subclass.
    pub name_override: Option<Id>,
    /// Backing dictionary shared by live class namespace proxies.
    pub dictionary: Option<Id>,
    pub namedtuple_defaults: Option<Id>,
    pub annotation_thunks: Option<Id>,
    /// Slot names consumed when the class is created, independent of later namespace mutations.
    pub slot_names: Option<std::rc::Rc<[String]>>,
}

#[derive(Clone)]
pub(super) struct Scope {
    pub class_body: Option<Vec<String>>,
    pub enum_namespace: Option<Box<super::enums::auto::EnumNamespace>>,
    pub is_abstract: bool,
    pub is_comprehension: bool,
    pub class_cell: Option<Id>,
    pub class_namespace: Option<Box<ClassNamespace>>,
    pub first_argument: Option<String>,
    pub names: indexmap::IndexMap<String, Id>,
    pub closure: Box<ClosureScope>,
    pub namespace: Option<Id>,
    pub globals: HashSet<String>,
    pub exec_globals: Option<ExecGlobals>,
    pub exec_locals: Option<Id>,
    pub parent: Option<Id>,
    pub module: String,
}

#[derive(Clone, Default)]
pub(super) struct ClosureScope {
    pub declared: HashSet<String>,
    pub declared_complete: bool,
    pub nonlocals: HashSet<String>,
    pub cells: HashMap<String, Id>,
}

#[derive(Clone, Copy)]
pub(super) struct ExecGlobals {
    pub dict: Id,
    pub builtins: Id,
}

#[derive(Clone)]
pub(super) enum Value {
    None,
    Weak(super::weakref::Reference),
    Ellipsis,
    NotImplemented,
    Bool(bool),
    Int(crate::Int),
    Float(f64),
    Complex(super::complex::Complex),
    Calendar {
        value: crate::objects::Calendar,
        tzinfo: Id,
    },
    Selector(std::rc::Rc<std::cell::RefCell<super::stdlib::readiness::Selector>>),
    Slice([Id; 3]),
    Range(super::range::Range),
    RangeIterator(super::range::Cursor),
    Str(String),
    Bytes(Vec<u8>),
    ByteArray(Vec<u8>),
    CodecInfo {
        entry: crate::objects::CodecInfo,
        decode: Id,
    },
    CodecDecoder(crate::objects::Codec),
    Hash(std::rc::Rc<std::cell::RefCell<crate::objects::HashState>>),
    Hmac(Hmac),
    Struct {
        format: String,
        layout: std::rc::Rc<crate::objects::interpreter_stdlib::StructFormat>,
    },
    LoggerHandle(std::rc::Rc<crate::objects::logging::Logger>),
    HandlerLock(std::sync::Arc<crate::objects::logging::HandlerLock>),
    Socket(crate::ObjectOwner),
    Deque(Box<crate::objects::DequeRing<Id>>),
    ReverseSequence {
        source: Option<Id>,
        remaining: usize,
    },
    DequeIterator {
        deque: Id,
        state: usize,
        remaining: usize,
        reverse: bool,
    },
    List(Vec<Id>),
    Tuple(Vec<Id>),
    NamedTuple {
        class: Option<&'static str>,
        names: Vec<String>,
        values: Vec<Id>,
        extras: Vec<(&'static str, Id)>,
    },
    Set {
        entries: super::dictionary::Entries,
        frozen: bool,
    },
    Dict(super::dictionary::Entries),
    MappingProxy(Id),
    MappingView {
        mapping: Id,
        kind: DictViewKind,
    },
    MappingViewIterator {
        mapping: Option<Id>,
        source: Option<Id>,
        kind: DictViewKind,
        running: bool,
    },
    DictView {
        dict: Id,
        kind: DictViewKind,
        ordered: bool,
    },
    DictIterator(DictIterator),
    DefaultDict {
        factory: Id,
        entries: super::dictionary::Entries,
    },
    Scope(Scope),
    Cell(Option<Id>),
    ClassCell(Option<Id>),
    Super {
        start: Id,
        receiver: Id,
        owner: Id,
    },
    Module {
        name: String,
        scope: Id,
        origin: Option<Id>,
        initializing: bool,
        // Imported modules are resolved separately from independently constructed ones.
        constructor: Option<Id>,
    },
    Function {
        fields: super::function_fields::FunctionFields,
        closure: Id,
        code: Id,
        scope: Id,
        class_cell: Option<Id>,
        defaults: Id,
        kwdefaults: Id,
    },
    Code(CodeObject),
    Generator {
        owner: FrameOwner,
        code: Id,
        scope: Id,
        iterators: Vec<Id>,
        offset: Option<usize>,
        running: bool,
    },
    Class {
        name: String,
        scope: Id,
        ancestors: Vec<Id>,
        namedtuple_fields: Option<std::rc::Rc<[String]>>,
    },
    TypeVar(super::typing::TypeVariable),
    GenericAlias(super::typing::Alias),
    Union(super::union::Union),
    Instance {
        class: Id,
        fields: indexmap::IndexMap<String, Id>,
        /// Exact ordinary instance dictionary once exposed or replaced.
        namespace: Option<Id>,
        dictionary: Option<Id>,
        scalar: Option<Id>,
        property: Option<Box<super::property::Property>>,
        weak_reference: Option<Box<super::weakref::Reference>>,
        memory_stream: Option<super::stdlib::io::MemoryStream>,
    },
    Native(String),
    AttrGetter(std::rc::Rc<[Vec<String>]>),
    /// The original keys are held in a Python tuple, shared by multi-key reductions.
    ItemGetter(Id),
    ThreadLocal(super::thread_local::Dictionaries),
    PrimitiveLock(std::sync::Arc<crate::objects::locks::PrimitiveLock>),
    Raised(Error),
    Traceback {
        frame: Id,
        next: Id,
        lineno: i64,
        span: Option<[u32; 3]>,
    },
    PythonFrame {
        scope: Id,
        code: Id,
        lineno: u32,
    },
    Method {
        callable: Id,
        receiver: Id,
        explicit: bool,
    },
    ClassMethod(Id),
    StaticMethod {
        callable: Id,
        dictionary: Id,
    },
    Property(super::property::Property),
    WeakrefDescriptor(Id),
    GetSetDescriptor {
        get: Id,
        set: Option<Id>,
        owner: &'static str,
        name: &'static str,
    },
    Partial {
        function: Id,
        args: Id,
        keywords: Id,
        coroutine_marker: Option<Id>,
    },
    PartialPlaceholder,
    Cached {
        function: Id,
        entries: Vec<CachedCall>,
        table: Id,
        marker: Id,
    },
    Yielded(Id),
    AstWalk {
        queue: std::collections::VecDeque<Id>,
        running: bool,
    },
    Enumerate {
        input: Id,
        next_index: crate::Int,
    },
    Count {
        current: Id,
        step: Id,
    },
    Repeat(crate::objects::Repeat<Id>),
    Map {
        function: Id,
        inputs: std::rc::Rc<[Id]>,
        strict: bool,
    },
    Filter {
        predicate: Id,
        input: Id,
    },
    Zip {
        inputs: std::rc::Rc<[Id]>,
        strict: bool,
    },
    CallableIterator(Option<[Id; 2]>),
    // Private to a compiled loop. Python-visible iterators keep their original identity.
    LoopIterator {
        source: Id,
        peeked: Option<Id>,
    },
    Iterator {
        kind: &'static str,
        sequence: Option<Id>,
        offset: usize,
        reverse: bool,
        context: Option<std::rc::Rc<HashMap<Id, Id>>>,
    },
    FormatIterator {
        input: Id,
        offset: usize,
        kind: FormatKind,
    },
    IoMethod {
        owner: Id,
        name: &'static str,
    },
    File(crate::shared::Shared<super::stdlib::BinaryFile>),
    TextFile(crate::objects::TextFile),
    RegexPattern(std::rc::Rc<super::stdlib::regex::Pattern>),
    BufferedRandom(crate::Shared<std::cell::RefCell<crate::objects::FileState>>),
    RegexMatch {
        groups: Vec<Option<Id>>,
        names: HashMap<String, usize>,
    },
    Context(std::rc::Rc<HashMap<Id, Id>>),
    ContextVar {
        name: Id,
        default: Option<Id>,
        repr_active: std::cell::Cell<bool>,
    },
    ContextToken {
        context: Id,
        variable: Id,
        old: Option<Id>,
        used: bool,
    },
    TokenMissing,
}

#[derive(Clone, Copy)]
pub(super) enum FormatKind {
    Markup,
    Field,
}

#[derive(Clone)]
pub(super) struct CachedCall {
    pub key: Id,
    pub hash: i64,
}

impl Value {
    pub(super) fn instance_class(&self) -> Option<Id> {
        match self {
            Self::Instance { class, .. } => Some(*class),
            Self::Module { constructor, .. } => *constructor,
            _ => None,
        }
    }

    fn visit_bindings(bindings: &HashMap<Id, Id>, visit: &mut MarkQueue<'_>) {
        for (key, value) in bindings {
            visit.extend([*key, *value]);
        }
    }

    fn children(&self, visit: &mut MarkQueue<'_>) {
        match self {
            Self::Deque(ring) => visit.extend(&ring.items),
            Self::ReverseSequence { source, .. } => visit.extend(*source),
            Self::DequeIterator { deque, .. } => visit.push(*deque),
            Self::List(v) | Self::Tuple(v) => visit.extend(v),
            Self::Set { entries, .. } => visit.extend(entries.iter().map(|&(key, _)| key)),
            Self::Iterator {
                sequence, context, ..
            } => {
                visit.extend(*sequence);
                if let Some(bindings) = context {
                    Self::visit_bindings(bindings, visit);
                }
            }
            Self::DictIterator(iter) => visit.extend(iter.dict),
            Self::FormatIterator { input, .. } => visit.push(*input),
            Self::ClassMethod(callable) => visit.push(*callable),
            Self::ThreadLocal(local) => visit.extend(local.live()),
            Self::Count { current, step } => visit.extend([*current, *step]),
            Self::Repeat(repeat) => visit.push(repeat.value),
            Self::ItemGetter(keys) => visit.push(*keys),
            Self::Map {
                function, inputs, ..
            } => {
                visit.push(*function);
                visit.extend(inputs.iter());
            }
            Self::Zip { inputs, .. } => visit.extend(inputs.iter()),
            Self::Filter { predicate, input } => visit.extend([*predicate, *input]),
            Self::CallableIterator(state) => visit.extend(state.iter().flatten()),
            Self::LoopIterator { source, peeked } => {
                visit.push(*source);
                visit.extend(peeked);
            }
            Self::NamedTuple { values, extras, .. } => {
                visit.extend(values);
                visit.extend(extras.iter().map(|&(_, id)| id));
            }
            Self::Dict(entries) => {
                for (key, value) in entries {
                    visit.extend([*key, *value]);
                }
            }
            Self::MappingView { mapping, .. } => visit.push(*mapping),
            Self::MappingViewIterator {
                mapping, source, ..
            } => {
                visit.extend(*mapping);
                visit.extend(*source);
            }

            Self::DictView { dict, .. } | Self::MappingProxy(dict) => visit.push(*dict),
            Self::Hmac(mac) => {
                visit.extend([mac.inner, mac.outer, mac.digest_size, mac.block_size])
            }
            Self::Context(bindings) => Self::visit_bindings(bindings, visit),
            Self::ContextVar { name, default, .. } => {
                visit.push(*name);
                visit.extend(default);
            }
            Self::ContextToken {
                context,
                variable,
                old,
                ..
            } => {
                visit.extend([*context, *variable]);
                visit.extend(old);
            }
            Self::Yielded(item) => visit.push(*item),
            Self::Slice(parts) => visit.extend(parts),
            Self::Range(_) | Self::RangeIterator(_) => {}
            Self::AstWalk { queue, .. } => visit.extend(queue),
            Self::Enumerate { input, .. } => visit.push(*input),
            Self::Calendar { tzinfo, .. } => visit.push(*tzinfo),
            Self::DefaultDict { factory, entries } => {
                visit.push(*factory);
                for (key, value) in entries {
                    visit.extend([*key, *value]);
                }
            }
            Self::Selector(selector) => selector.borrow().trace(visit),
            Self::Scope(scope) => {
                visit.extend(scope.closure.cells.values().copied());
                visit.extend(scope.class_cell);
                if let Some(state) = &scope.enum_namespace {
                    visit.extend(state.generator);
                    visit.extend(&state.history);
                }
                if let Some(class) = &scope.class_namespace {
                    visit.extend([class.bases, class.mro]);
                    visit.extend(class.name_override);
                    visit.extend(class.namedtuple_defaults);
                    visit.extend(class.annotation_thunks);
                    visit.extend(class.dictionary);
                }
                visit.extend(scope.parent);
                visit.extend(scope.namespace);
                visit.extend(scope.exec_locals);
                if let Some(globals) = scope.exec_globals {
                    visit.extend([globals.dict, globals.builtins]);
                }
                visit.extend(scope.names.values());
            }
            Self::ClassCell(class) => visit.extend(class),
            Self::Super {
                start,
                receiver,
                owner,
            } => visit.extend([*start, *receiver, *owner]),
            Self::Module {
                scope,
                origin,
                constructor,
                ..
            } => {
                visit.push(*scope);
                visit.extend(origin);
                visit.extend(constructor);
            }
            Self::Cell(value) => visit.extend(value),
            Self::Function {
                closure,
                scope,
                class_cell,
                code,
                defaults,
                kwdefaults,
                fields,
                ..
            } => {
                visit.extend([*scope, *code, *defaults, *kwdefaults, *closure]);
                visit.extend(class_cell);
                fields.children(visit);
            }
            Self::Code(code) => {
                visit.extend([
                    code.name,
                    code.qualname,
                    code.doc,
                    code.filename,
                    code.parameters,
                ]);
                visit.extend(code.nested.iter().map(|&(_, id)| id));
                if let Body::Source(program) = &code.body {
                    visit.push(program.string_roots);
                }
            }
            Self::Generator {
                owner,
                code,
                scope,
                iterators,
                ..
            } => {
                visit.extend([*code, *scope, owner.root()]);
                visit.extend(iterators);
            }
            Self::Class {
                scope, ancestors, ..
            } => {
                visit.push(*scope);
                visit.extend(ancestors);
            }
            Self::TypeVar(var) => visit.extend([var.name, var.module, var.bound, var.constraints]),
            Self::GenericAlias(alias) => {
                visit.extend([alias.origin, alias.args, alias.parameters]);
                if let super::typing::AliasKind::Literal(members) = alias.kind {
                    visit.push(members);
                }
            }
            Self::Union(union) => {
                visit.extend([union.args, union.members, union.parameters]);
                visit.extend(union.unhashable);
            }
            Self::Instance {
                class,
                fields,
                namespace,
                dictionary,
                scalar,
                weak_reference,
                memory_stream,
                property,
            } => {
                if let Some(property) = property {
                    property.children(visit);
                }
                if let Some(reference) = weak_reference {
                    visit.push(reference.callback);
                }
                match memory_stream {
                    Some(super::stdlib::io::MemoryStream::Bytes(stream)) => {
                        visit.extend(stream.snapshot.get())
                    }
                    Some(super::stdlib::io::MemoryStream::Wrapper(stream)) => {
                        stream.children(visit)
                    }
                    _ => {}
                }
                visit.push(*class);
                visit.extend(dictionary);
                visit.extend(fields.values());
                visit.extend(namespace);
                visit.extend(scalar);
            }
            Self::Method {
                callable, receiver, ..
            } => visit.extend([*callable, *receiver]),
            Self::StaticMethod {
                callable,
                dictionary,
            } => visit.extend([*callable, *dictionary]),
            Self::Property(property) => property.children(visit),
            Self::Weak(wr) => visit.push(wr.callback),
            Self::WeakrefDescriptor(owner) => visit.push(*owner),
            Self::GetSetDescriptor { get, set, .. } => {
                visit.push(*get);
                visit.extend(*set);
            }
            Self::Partial {
                function,
                args,
                keywords,
                coroutine_marker,
            } => {
                visit.extend([*function, *args, *keywords]);
                visit.extend(coroutine_marker);
            }
            Self::Cached {
                function,
                entries,
                table,
                marker,
            } => {
                visit.extend([*function, *table, *marker]);
                visit.extend(entries.iter().map(|call| call.key));
            }
            Self::Raised(error) => visit.extend(error.roots()),
            Self::Traceback { frame, next, .. } => visit.extend([*frame, *next]),
            Self::PythonFrame { scope, code, .. } => visit.extend([*scope, *code]),
            Self::IoMethod { owner, .. } => visit.push(*owner),
            Self::CodecInfo { decode, .. } => visit.push(*decode),
            Self::RegexPattern(pattern) => visit.push(pattern.source),
            Self::RegexMatch { groups, .. } => visit.extend(groups.iter().flatten()),
            Self::None
            | Self::Ellipsis
            | Self::NotImplemented
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Complex(_)
            | Self::Str(_)
            | Self::Bytes(_)
            | Self::ByteArray(_)
            | Self::CodecDecoder(_)
            | Self::Struct { .. }
            | Self::Hash(_)
            | Self::LoggerHandle(_)
            | Self::HandlerLock(_)
            | Self::PrimitiveLock(_)
            | Self::Socket(_)
            | Self::Native(_)
            | Self::AttrGetter(_)
            | Self::PartialPlaceholder
            | Self::TokenMissing
            | Self::File(_)
            | Self::TextFile(_)
            | Self::BufferedRandom(_) => {}
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DictViewKind {
    Keys,
    Values,
    Items,
}

#[derive(Clone, Copy)]
pub(super) struct DictIterator {
    dict: Option<Id>,
    kind: DictViewKind,
    expected_len: Option<usize>,
    position: usize,
    reverse: bool,
    remaining: usize,
    order_state: Option<(usize, usize, usize)>,
}

impl DictIterator {
    /// Capture the original size while keeping the entries live.
    pub(super) fn new(
        dict: Id,
        kind: DictViewKind,
        size: usize,
        order_state: Option<(usize, usize, usize)>,
        reverse_end: Option<usize>,
    ) -> Self {
        Self {
            dict: Some(dict),
            kind,
            expected_len: Some(size),
            position: reverse_end.unwrap_or(0),
            reverse: reverse_end.is_some(),
            remaining: size,
            order_state,
        }
    }

    /// An invalid or exhausted iterator has no remaining length hint.
    pub(super) fn length_hint(&self, ctx: &Context, default: i64) -> i64 {
        if self.order_state.is_some() {
            return default;
        }
        self.dict
            .and_then(|dict| ctx.dict_entries(dict))
            .filter(|entries| Some(entries.len()) == self.expected_len)
            .map_or(0, |_| self.remaining as i64)
    }

    /// Check the live size before each pull, including the one that reaches exhaustion.
    pub(super) fn next(&mut self, ctx: &mut Context) -> Result<Option<Id>> {
        let Some(dict) = self.dict else {
            return Ok(None);
        };
        let storage = ctx.dict_storage(dict).expect("dictionary iterator source");
        let (Value::Dict(entries) | Value::DefaultDict { entries, .. }) = ctx.heap.get(storage)
        else {
            unreachable!("dictionary storage");
        };
        if let Some(order) = self.order_state {
            if self.remaining == 0 {
                self.dict = None;
                return Ok(None);
            }
            if order != entries.order_state() || self.expected_len != Some(entries.len()) {
                self.dict = None;
                return Err(Error::new(
                    "RuntimeError",
                    "OrderedDict mutated during iteration",
                ));
            }
        }
        if self.expected_len != Some(entries.len()) {
            self.expected_len = None;
            return Err(Error::new(
                "RuntimeError",
                "dictionary changed size during iteration",
            ));
        }
        let entry = if self.order_state.is_some() {
            let rank = if self.reverse {
                self.position.checked_sub(1)
            } else {
                Some(self.position)
            };
            rank.filter(|&rank| rank < entries.len()).map(|rank| {
                let (key, value) = entries[entries.ordered_dense(rank)];
                (rank, key, value)
            })
        } else if self.reverse {
            entries.before(self.position)
        } else {
            entries.at(self.position)
        };
        let Some((position, key, value)) = entry else {
            self.dict = None;
            return Ok(None);
        };
        if !self.reverse && self.remaining == 0 {
            self.dict = None;
            return Err(Error::new(
                "RuntimeError",
                "dictionary keys changed during iteration",
            ));
        }
        self.position = if self.reverse { position } else { position + 1 };
        self.remaining = self.remaining.saturating_sub(1);
        Ok(Some(match self.kind {
            DictViewKind::Keys => key,
            DictViewKind::Values => value,
            DictViewKind::Items => ctx.alloc(Value::Tuple(vec![key, value])),
        }))
    }
}

// A slot's trace link is UNMARKED until queued. END terminates the queue and marks a
// visited slot. Both sentinels exceed the largest possible Vec index.
const UNMARKED: usize = usize::MAX;
const END: usize = usize::MAX - 1;

struct Slot {
    value: Option<Value>,
    trace: Cell<usize>,
}

impl Slot {
    fn new(value: Value) -> Self {
        Self {
            value: Some(value),
            trace: Cell::new(UNMARKED),
        }
    }

    fn marked(&self) -> bool {
        self.trace.get() != UNMARKED
    }
}

/// The pending trace lives in heap slots, so looking up a weak intern entry needs no scratch allocation.
pub(super) struct MarkQueue<'a> {
    slots: &'a [Slot],
    next: usize,
}

impl MarkQueue<'_> {
    fn push(&mut self, id: Id) {
        let slot = &self.slots[id as usize];
        if !slot.marked() {
            slot.trace.set(self.next);
            self.next = id as usize;
        }
    }

    pub(super) fn extend<T: std::borrow::Borrow<Id>>(&mut self, ids: impl IntoIterator<Item = T>) {
        for id in ids {
            self.push(*id.borrow());
        }
    }

    fn pop(&mut self) -> Option<Id> {
        let index = self.next;
        if index == END {
            return None;
        }
        self.next = self.slots[index].trace.replace(END);
        Some(index as Id)
    }
}

/// Native frames hold temporaries until the next Python statement. A caller's
/// partial expression remains rooted while evaluating a nested native function.
pub(super) struct Heap {
    slots: Vec<Slot>,
    // Native owners identify spellings. Runtime-created strings keep only a weak boxed handle.
    interned: HashMap<crate::StrOwner, InternedString>,
    // ABC registrations are weak edges. Sweep them before recycling a dead class's slot.
    pub(super) abc_registrations: Vec<[Id; 2]>,
    free: Vec<usize>,
    pub(super) immutable_hashes: HashMap<Id, i64>,
    frames: Vec<Frame>,
    next_collection: usize,
    weak: HashMap<Id, Vec<Id>>,
    // A dead proxy is still observable after its registry entry disappears.
    pub(super) has_weak_values: bool,
    collect_next_statement: bool,
    // Weak handles to buffered output files returned by open().
    file_writers: Vec<Id>,
}

#[derive(Clone, Copy)]
struct InternedString {
    id: Id,
    literal: bool,
}

/// A Python call observes its mutable function, while bare code keeps its entry module.
#[derive(Clone, Copy)]
pub(super) enum FrameOwner {
    Function(Id),
    Module(Id),
}

impl FrameOwner {
    fn root(self) -> Id {
        match self {
            Self::Function(id) | Self::Module(id) => id,
        }
    }
}

struct Frame {
    owner: FrameOwner,
    scope: Id,
    code: Id,
    live: Vec<Id>,
    location: Option<Location>,
    object: Option<Id>,
    inherited: Id,
    active: Id,
    // This activation or one of its calls opened a buffered output file.
    opened_writer: bool,
}

impl Frame {
    fn new(scope: Id, code: Id, owner: FrameOwner, inherited: Id) -> Self {
        Self {
            owner,
            scope,
            code,
            live: Vec::new(),
            location: None,
            object: None,
            inherited,
            active: inherited,
            opened_writer: false,
        }
    }
}

impl Heap {
    pub fn new() -> Self {
        Self {
            slots: vec![
                Slot::new(Value::None),
                Slot::new(Value::Bool(false)),
                Slot::new(Value::Bool(true)),
                Slot::new(Value::Ellipsis),
                Slot::new(Value::NotImplemented),
            ],
            interned: HashMap::new(),
            abc_registrations: Vec::new(),
            free: Vec::new(),
            immutable_hashes: HashMap::new(),
            frames: vec![Frame::new(NONE, NONE, FrameOwner::Module(NONE), NONE)],
            next_collection: 4096,
            weak: HashMap::new(),
            has_weak_values: false,
            collect_next_statement: false,
            file_writers: Vec::new(),
        }
    }

    pub fn weak_references(&self, target: Id) -> &[Id] {
        self.weak.get(&target).map_or(&[], Vec::as_slice)
    }

    pub fn new_weak_reference(
        &mut self,
        target: Id,
        callback: Id,
        kind: super::weakref::Kind,
        subclass: Option<Id>,
    ) -> Id {
        let references = self.weak_references(target);
        if subclass.is_none() && callback == NONE
            && let Some(&id) = references.iter().find(|&&id|
                matches!(self.get(id), Value::Weak(wr) if wr.kind == kind && wr.callback == NONE))
        {
            return id;
        }
        // Callback-free canonical references precede proxies, then callbacks in newest-first order.
        let index = references
            .iter()
            .position(|&id| match self.get(id) {
                Value::Weak(wr) => {
                    wr.callback != NONE
                        || (subclass.is_none()
                            && callback == NONE
                            && kind == super::weakref::Kind::Reference)
                }
                _ => true,
            })
            .unwrap_or(references.len());
        let reference = super::weakref::Reference {
            target: Some(target),
            callback,
            kind,
            hash: None,
        };
        let id = if let Some(owner) = subclass {
            let Value::Instance { weak_reference, .. } = self.get_mut(owner) else {
                unreachable!()
            };
            *weak_reference = Some(Box::new(reference));
            owner
        } else {
            self.alloc(Value::Weak(reference))
        };
        self.weak.entry(target).or_default().insert(index, id);
        self.has_weak_values = true;
        // The caller may hold either the target or the new wrapper only as a temporary.
        self.collect_next_statement = true;
        id
    }

    fn weak_edge(&self, value: Id) -> bool {
        self.weak.contains_key(&value)
            || self
                .get(value)
                .weak_reference()
                .is_some_and(|wr| wr.target.is_some())
    }

    /// A name in a surviving frame keeps this object alive until that binding changes.
    fn bound_in_active_frame(&self, value: Id) -> bool {
        self.frames.iter().rev().any(|frame| {
            let Value::Scope(scope) = self.get(frame.scope) else {
                return false;
            };
            if let Some(namespace) = scope.namespace {
                return matches!(self.get(namespace), Value::Dict(entries)
                    if entries.iter().any(|&(_, bound)| bound == value));
            }
            scope.names.values().any(|&bound| bound == value)
        })
    }

    pub(super) fn replace_cell(&mut self, cell: Id, value: Option<Id>) -> Option<Id> {
        let Value::Cell(contents) = self.get_mut(cell) else {
            unreachable!("compiled closure cell")
        };
        let previous = std::mem::replace(contents, value);
        if let Some(old) = previous
            && Some(old) != value
        {
            self.weak_edge_removed(old);
        }
        previous
    }

    pub fn weak_edge_removed(&mut self, value: Id) {
        if self.has_weak_values
            && (self.weak_edge(value)
                || matches!(
                    self.get(value),
                    Value::Function { .. } | Value::Cell(Some(_))
                ))
            && !self.bound_in_active_frame(value)
        {
            // A function or a cell may own the last strong path to a captured value.
            self.collect_next_statement = true;
        }
    }

    pub fn alloc(&mut self, value: Value) -> Id {
        if let Value::Str(text) = &value
            && let Some(owner) = crate::objects::short_string(text)
        {
            return self.intern_literal(owner);
        }
        self.alloc_slot(value)
    }

    fn alloc_slot(&mut self, value: Value) -> Id {
        let index = if let Some(index) = self.free.pop() {
            self.slots[index].value = Some(value);
            index
        } else {
            self.slots.push(Slot::new(value));
            self.slots.len() - 1
        };
        let id = index as Id;
        self.keep(id);
        id
    }

    /// A code literal and every occurrence of a Latin-1 character share their Python handle.
    fn intern_literal(&mut self, owner: crate::StrOwner) -> Id {
        if let Some(entry) = self.interned.get_mut(&owner) {
            entry.literal = true;
            let id = entry.id;
            self.keep(id);
            return id;
        }
        let id = self.alloc_slot(Value::Str(owner.to_string()));
        self.interned
            .insert(owner, InternedString { id, literal: true });
        id
    }

    /// Mark the shared spelling as a live code constant and return its local Python handle.
    pub fn code_name(&mut self, text: &str) -> Id {
        if let Some(entry) = self.interned.get_mut(text) {
            entry.literal = true;
            let id = entry.id;
            self.keep(id);
            return id;
        }
        self.intern_literal(crate::sys::intern_owner(&crate::objects::literal_string(
            text,
        )))
    }

    pub fn string_literal(&mut self, text: &str) -> Id {
        if text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return self.code_name(text);
        }
        self.alloc(Value::Str(text.to_owned()))
    }

    /// Runtime-compiled ASCII constants are interned for the lifetime of their code.
    pub fn source_string_constant(&mut self, text: &str) -> Option<Id> {
        let intern = text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if intern && let Some(entry) = self.interned.get(text) {
            let id = entry.id;
            self.keep(id);
            return Some(id);
        }
        let mut value = String::new();
        value.try_reserve_exact(text.len()).ok()?;
        value.push_str(text);
        let id = self.alloc(Value::Str(value));
        Some(if intern {
            self.intern(id, std::iter::empty())
        } else {
            id
        })
    }

    pub fn is_interned(&self, id: Id) -> bool {
        matches!(self.get(id), Value::Str(s) if self.interned.get(s.as_str()).is_some_and(|entry| entry.id == id))
    }

    /// A dead boxed string may still occupy its slot until the next collection.
    pub fn intern(&mut self, id: Id, roots: impl Iterator<Item = Id>) -> Id {
        let Value::Str(text) = self.get(id) else {
            unreachable!()
        };
        if let Some(&entry) = self.interned.get(text.as_str())
            && (entry.id == id
                || entry.literal
                || self.bound_in_active_frame(entry.id)
                || self.reachable(entry.id, roots))
        {
            self.keep(entry.id);
            return entry.id;
        }
        let Ok(owner) = crate::objects::try_literal_string(text) else {
            return id;
        };
        let Some(owner) = crate::sys::try_intern_owner(&owner) else {
            return id;
        };
        if self.interned.try_reserve(1).is_err() {
            return id;
        }
        self.interned
            .insert(owner, InternedString { id, literal: false });
        id
    }

    pub fn get(&self, id: Id) -> &Value {
        self.slots[id as usize]
            .value
            .as_ref()
            .expect("live native handle")
    }
    pub fn get_mut(&mut self, id: Id) -> &mut Value {
        self.slots[id as usize]
            .value
            .as_mut()
            .expect("live native handle")
    }
    pub fn enter(&mut self, scope: Id, code: Id, owner: FrameOwner) {
        self.frames
            .push(Frame::new(scope, code, owner, self.active_exception()));
    }
    /// Release the activation and report whether a returning function may have dropped a writer.
    pub fn leave(&mut self, value: Id, returning: bool) -> bool {
        let frame = self.frames.pop().expect("returning native activation");
        let opened = frame.opened_writer && !self.file_writers.is_empty();
        if opened {
            self.frames.last_mut().unwrap().opened_writer = true;
        }
        let writer_released = opened && returning && self.has_unbound_writer(value);
        if (self.has_weak_values || writer_released)
            && let Value::Scope(scope) = self.get(frame.scope)
        {
            let weak_released = self.has_weak_values
                && frame.live.iter().chain(scope.names.values()).any(|&id| {
                    id != value && self.weak_edge(id) && !self.bound_in_active_frame(id)
                });
            if weak_released || writer_released {
                // A normal function allocates its scope in the caller before entering.
                // Once it returns, only an escaped closure or frame can keep that scope alive.
                if scope.parent.is_some() && scope.class_body.is_none() {
                    let caller = &mut self.frames.last_mut().unwrap().live;
                    if let Some(index) = caller.iter().position(|&id| id == frame.scope) {
                        caller[index] = NONE;
                    }
                }
                self.collect_next_statement |= weak_released;
            }
        }
        self.keep(value);
        writer_released
    }
    /// Reserve temporary roots before taking a snapshot that may outlive a Python callback.
    pub(super) fn reserve_temporaries(&mut self, additional: usize) -> Result<()> {
        self.frames
            .last_mut()
            .unwrap()
            .live
            .try_reserve(additional)
            .map_err(|_| Error::new("MemoryError", ""))
    }

    pub fn keep(&mut self, value: Id) {
        self.frames.last_mut().unwrap().live.push(value);
    }

    /// Mark temporary roots that must survive the next iteration of a native loop.
    pub fn temporary_mark(&self) -> usize {
        self.frames.last().unwrap().live.len()
    }

    /// Compiled calls root their fresh argument list before evaluating its operands.
    pub(super) fn argument_mark(&self, arguments: Id) -> Option<usize> {
        if self.file_writers.is_empty() {
            return None;
        }
        self.frames
            .last()?
            .live
            .iter()
            .position(|&id| id == arguments)
    }

    /// A scalar and a container of scalars cannot hold an output file.
    pub(super) fn could_own_writer(&self, id: Id, skip_bound: bool) -> bool {
        let scalar = |id| {
            matches!(
                self.get(id),
                Value::None
                    | Value::Bool(_)
                    | Value::Int(_)
                    | Value::Float(_)
                    | Value::Complex(_)
                    | Value::Str(_)
                    | Value::Bytes(_)
                    | Value::ByteArray(_)
                    | Value::Native(_)
            )
        };
        if scalar(id) || (skip_bound && self.bound_in_active_frame(id)) {
            return false;
        }
        match self.get(id) {
            Value::List(values) | Value::Tuple(values) => values.iter().any(|&item| !scalar(item)),
            Value::Dict(entries) => !entries.is_empty(),
            _ => true,
        }
    }

    /// Discard the evaluated operand while preserving earlier arguments in the same statement.
    pub(super) fn discard_truth(&mut self, mark: usize) -> bool {
        if self.file_writers.is_empty() {
            return false;
        }
        let could_own_writer = self
            .frames
            .last()
            .unwrap()
            .live
            .iter()
            .skip(mark)
            .any(|&id| self.could_own_writer(id, true));
        if !could_own_writer || !self.has_unbound_writer(NONE) {
            return false;
        }
        self.release_temporaries_since(mark);
        true
    }

    pub fn release_temporaries_since(&mut self, mark: usize) {
        self.frames.last_mut().unwrap().live.truncate(mark);
    }

    pub fn active_exception(&self) -> Id {
        self.frames.last().unwrap().active
    }

    pub fn replace_active(&mut self, exception: Id) -> Id {
        std::mem::replace(&mut self.frames.last_mut().unwrap().active, exception)
    }

    /// Materialize one frame per activation only when an exception unwinds it.
    pub fn trace_location(&mut self) -> Option<(Location, Id)> {
        let frame = self.frames.last()?;
        let location = frame.location.clone()?;
        if let Some(object) = frame.object {
            return Some((location, object));
        }
        let object = self.alloc(Value::PythonFrame {
            scope: frame.scope,
            code: frame.code,
            lineno: location.line,
        });
        self.frames.last_mut().unwrap().object = Some(object);
        Some((location, object))
    }

    pub fn caller_module(&self) -> Id {
        match self.frames.last().unwrap().owner {
            FrameOwner::Module(module) => module,
            FrameOwner::Function(function) => {
                let Value::Function { fields, .. } = self.get(function) else {
                    unreachable!("executing Python function")
                };
                fields.module
            }
        }
    }

    pub fn current_scope(&self) -> Id {
        self.frames.last().unwrap().scope
    }

    pub fn current_location(&self) -> Option<Location> {
        self.frames.last()?.location.clone()
    }

    /// Trace the same roots for collection and for a weak interning lookup between collections.
    fn mark(&self, globals: impl Iterator<Item = Id>) {
        let mut pending = MarkQueue {
            slots: &self.slots,
            next: END,
        };
        pending.extend(globals);
        pending.extend([NONE, FALSE, TRUE, ELLIPSIS, NOT_IMPLEMENTED]);
        pending.extend(
            self.interned
                .values()
                .filter(|entry| entry.literal)
                .map(|entry| entry.id),
        );
        for frame in &self.frames {
            pending.extend([
                frame.scope,
                frame.code,
                frame.owner.root(),
                frame.active,
                frame.inherited,
            ]);
            pending.extend(frame.object);
            pending.extend(&frame.live);
        }
        while let Some(id) = pending.pop() {
            self.get(id).children(&mut pending);
        }
    }

    fn reachable(&self, id: Id, roots: impl Iterator<Item = Id>) -> bool {
        self.mark(roots);
        let reachable = self.slots[id as usize].marked();
        for slot in &self.slots {
            slot.trace.set(UNMARKED);
        }
        reachable
    }

    /// Track a disk writer in the activation whose locals or return value may own it.
    pub(super) fn alloc_file_writer(&mut self, file: Value) -> Result<Id> {
        self.file_writers
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        let id = self.alloc(file);
        self.file_writers.push(id);
        self.frames.last_mut().unwrap().opened_writer = true;
        Ok(id)
    }

    /// A returned writer or a surviving name keeps a direct owner.
    pub(super) fn has_unbound_writer(&self, retained: Id) -> bool {
        self.file_writers
            .iter()
            .any(|&id| id != retained && !self.bound_in_active_frame(id))
    }

    /// Return whether a buffered file could need collecting at a target boundary.
    pub(super) fn has_file_writers(&self) -> bool {
        !self.file_writers.is_empty()
    }

    /// Recheck unbound writers after control flow discards temporary roots.
    pub fn release_unbound_writers(&mut self) {
        self.collect_next_statement |= self.has_unbound_writer(NONE);
    }

    /// Keep the operands needed by later targets after a source assignment releases its old value.
    pub(super) fn assignment_target(&mut self, live: &[Id], replaced: bool) -> bool {
        // The private unpack list still owns all items waiting for a target.
        if !replaced || !self.has_unbound_writer(NONE) {
            return false;
        }
        let temporaries = &mut self.frames.last_mut().unwrap().live;
        temporaries.clear();
        temporaries.extend_from_slice(live);
        true
    }

    /// A named expression retains earlier operands and its result after replacing the binding.
    pub(super) fn named_assignment(&mut self, mark: usize, value: Id) -> bool {
        if !self.has_unbound_writer(value) {
            return false;
        }
        self.release_temporaries_since(mark);
        self.keep(value);
        true
    }

    pub fn statement(
        &mut self,
        globals: impl Iterator<Item = Id>,
        live: &[Id],
        location: Location,
        caught: Id,
    ) -> Option<Vec<(Id, Id)>> {
        let frame = self.frames.last_mut().unwrap();
        frame.active = if caught == NONE {
            frame.inherited
        } else {
            caught
        };
        if let Some(id) = frame.object
            && let Some(Value::PythonFrame { lineno, .. }) = &mut self.slots[id as usize].value
        {
            *lineno = location.line;
        }
        frame.location = Some(location);
        frame.live.clear();
        frame.live.extend(live);
        if !std::mem::take(&mut self.collect_next_statement)
            && (self.slots.len() < self.next_collection || !self.free.is_empty())
        {
            return None;
        }
        self.collect(globals)
    }

    /// Drop unreachable values and preserve callbacks that must run after the sweep.
    pub(super) fn collect(&mut self, globals: impl Iterator<Item = Id>) -> Option<Vec<(Id, Id)>> {
        self.mark(globals);
        self.file_writers.retain(|&id| {
            let slot = &self.slots[id as usize];
            slot.marked()
                && match &slot.value {
                    Some(Value::TextFile(file)) => !file.closed(),
                    Some(Value::File(file)) => !file.closed(),
                    _ => false,
                }
        });
        self.interned
            .retain(|_, entry| self.slots[entry.id as usize].marked());
        // Clear non-owning handles before any dead target slot can be reused. Dead wrappers
        // never receive callbacks, and every live wrapper is cleared before any callback runs.
        let mut callbacks = Vec::new();
        self.weak.retain(|&target, refs| {
            refs.retain(|&id| self.slots[id as usize].marked());
            if self.slots[target as usize].marked() {
                return !refs.is_empty();
            }
            for &id in refs.iter() {
                let wr = self.slots[id as usize]
                    .value
                    .as_mut()
                    .unwrap()
                    .weak_reference_mut()
                    .unwrap();
                wr.target = None;
                let callback = std::mem::replace(&mut wr.callback, NONE);
                if callback != NONE {
                    callbacks.push((callback, id));
                }
            }
            false
        });
        self.abc_registrations.retain(|&[owner, member]| {
            self.slots[owner as usize].marked() && self.slots[member as usize].marked()
        });
        for (id, slot) in self.slots.iter_mut().enumerate() {
            let dead = slot.trace.replace(UNMARKED) == UNMARKED;
            if dead && slot.value.take().is_some() {
                self.immutable_hashes.remove(&(id as Id));
                self.free.push(id);
            }
        }
        self.next_collection = (self.slots.len() - self.free.len())
            .saturating_mul(2)
            .max(4096);
        for &(callback, wr) in &callbacks {
            self.keep(callback);
            self.keep(wr);
        }
        (!callbacks.is_empty()).then_some(callbacks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_context_iterator_traces_bindings_until_dropped() {
        let mut heap = Heap::new();
        let name = heap.alloc(Value::Str("key".into()));
        let key = heap.alloc(Value::ContextVar {
            name,
            default: None,
            repr_active: std::cell::Cell::new(false),
        });
        let value = heap.alloc(Value::List(Vec::new()));
        let view = heap.alloc(Value::Iterator {
            sequence: None,
            offset: 1,
            reverse: false,
            kind: "values",
            context: Some(std::rc::Rc::new(HashMap::from([(key, value)]))),
        });
        let location = Location {
            file: "test".into(),
            line: 1,
            span: None,
        };
        heap.next_collection = 0;
        heap.statement(std::iter::empty(), &[view], location.clone(), NONE);
        assert!(matches!(heap.get(key), Value::ContextVar { .. }));
        assert!(matches!(heap.get(value), Value::List(_)));

        heap.next_collection = 0;
        heap.statement(std::iter::empty(), &[], location, NONE);
        assert!(heap.slots[key as usize].value.is_none());
        assert!(heap.slots[value as usize].value.is_none());
    }
}

#[cfg(test)]
mod reachability_tests {
    use super::{Heap, Value};

    #[test]
    fn clear_trace_links_between_cyclic_reachability_queries() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let root = heap.alloc(Value::List(Vec::new()));
        let child = heap.alloc(Value::Tuple(vec![root]));
        let other = heap.alloc(Value::List(vec![root]));
        let Value::List(items) = heap.get_mut(root) else {
            unreachable!()
        };
        items.extend([child, child]);
        heap.release_temporaries_since(mark);

        assert!(heap.reachable(child, [root].into_iter()));
        assert!(!heap.reachable(other, [root].into_iter()));
        assert!(!heap.reachable(child, std::iter::empty()));
        assert!(heap.reachable(child, [other].into_iter()));
    }
}
