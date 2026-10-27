//! Values shared by native functions loading Python modules through `sys.path`.

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
    pub args: Option<Vec<Id>>,
    pub os_args: Option<Box<crate::objects::exceptions::OsErrorArgs>>,
    pub trace: Option<Box<Trace>>,
    pub exit_status: Option<ExitStatus>,
    pub import: Option<crate::shared::Shared<ImportFields>>,
}

impl Error {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            class: None,
            message: message.into(),
            args: None,
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
            .iter()
            .flatten()
            .copied()
            .chain(self.class)
            .chain(self.trace.iter().flat_map(|trace| {
                trace
                    .frames
                    .iter()
                    .map(|&(_, frame)| frame)
                    .chain((trace.stop_value != NONE).then_some(trace.stop_value))
                    .chain(trace.instance)
                    .chain(trace.context)
                    .chain(trace.cause)
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
            Value::List(_) => "list",
            Value::CallableIterator(_) => "callable_iterator",
            Value::NamedTuple {
                class: Some(class), ..
            } => class,
            Value::Tuple(_) | Value::NamedTuple { .. } => "tuple",
            Value::Slice(_) => "slice",
            Value::Range { .. } => "range",
            Value::Enumerate { .. } => "enumerate",
            Value::Iterator { kind, .. } => kind,
            Value::DictIterator(iter) => match iter.kind {
                DictViewKind::Keys => "dict_keyiterator",
                DictViewKind::Values => "dict_valueiterator",
                DictViewKind::Items => "dict_itemiterator",
            },
            Value::Set { frozen: true, .. } => "frozenset",
            Value::Set { frozen: false, .. } => "set",
            Value::Dict(_) => "dict",
            Value::MappingProxy(_) => "mappingproxy",
            Value::DefaultDict { .. } => "defaultdict",
            Value::Function { .. } => "function",
            Value::ItemGetter(_) => "operator.itemgetter",
            Value::Code(_) => "code",
            Value::Cell { .. } => "cell",
            Value::Generator { .. } => "generator",
            Value::TypeVar(_) => "TypeVar",
            Value::GenericAlias(alias) => alias.kind.type_name(),
            Value::Class { .. } => "type",
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
            Value::ReadOnlyDescriptor { .. } => "getset_descriptor",
            Value::Raised(error) => &error.kind,
            Value::Traceback { .. } => "traceback",
            Value::PythonFrame { .. } => "frame",
            Value::Module { .. } => "module",
            Value::Instance { class, .. } => match self.heap.get(*class) {
                Value::Class { name, .. } | Value::Native(name) => name,
                _ => "object",
            },
            _ => "object",
        }
    }

    pub(super) fn lookup_error(&self, kind: &str, key: Id) -> Error {
        let mut error = Error::new(kind, self.repr(key));
        error.args = Some(vec![key]);
        error
    }

    pub(super) fn exception(&mut self, kind: &str, args: &[Id]) -> Error {
        let mut error = Error::new(kind, "");
        error.args = Some(args.to_vec());
        if kind == "StopIteration" {
            error.set_stop_value(args.first().copied().unwrap_or(NONE));
        }
        error.message = self.exception_message(&error, &mut Vec::new());
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
        let os_args = error.os_args();
        let (kind, message) = error.into_parts();
        match message {
            Ok(message) => Self {
                os_args: os_args.map(Box::new),
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
        write!(f, "{}: {}", self.kind, self.message)
    }
}

pub(super) type Result<T> = std::result::Result<T, Error>;

/// Data retained by class namespaces after their body has run.
#[derive(Clone, Default)]
pub(super) struct ClassNamespace {
    pub bases: Id,
    pub mro: Id,
    pub annotation_thunks: Option<Id>,
    /// Slot names consumed when the class is created, independent of later namespace mutations.
    pub slot_names: Option<std::rc::Rc<[String]>>,
}

#[derive(Clone)]
pub(super) struct Scope {
    pub class_body: Option<Vec<String>>,
    pub enum_namespace: Option<Box<super::enums::auto::EnumNamespace>>,
    pub is_abstract: bool,
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
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
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
    DictView {
        dict: Id,
        kind: DictViewKind,
    },
    DictIterator(DictIterator),
    DefaultDict {
        factory: Id,
        entries: super::dictionary::Entries,
    },
    Scope(Scope),
    Cell {
        scope: Id,
        name: String,
    },
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
    Instance {
        class: Id,
        fields: HashMap<String, Id>,
        dictionary: Option<Id>,
        scalar: Option<Id>,
        property: Option<Box<super::property::Property>>,
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
    ReadOnlyDescriptor {
        get: Id,
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
    pub args: Vec<Id>,
    pub kwargs: Vec<(String, Id)>,
    pub result: Id,
}

impl Value {
    fn visit_bindings(bindings: &HashMap<Id, Id>, visit: &mut Vec<Id>) {
        for (key, value) in bindings {
            visit.extend([*key, *value]);
        }
    }

    fn children(&self, visit: &mut Vec<Id>) {
        match self {
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
            Self::Range { .. } => {}
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
                    visit.extend(class.annotation_thunks);
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
            Self::Module { scope, origin, .. } => {
                visit.push(*scope);
                visit.extend(origin);
            }
            Self::Cell { scope, .. } => visit.push(*scope),
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
            }
            Self::Generator {
                code,
                scope,
                iterators,
                ..
            } => {
                visit.extend([*code, *scope]);
                visit.extend(iterators);
            }
            Self::Class {
                scope, ancestors, ..
            } => {
                visit.push(*scope);
                visit.extend(ancestors);
            }
            Self::TypeVar(var) => visit.extend([var.name, var.module, var.bound, var.constraints]),
            Self::GenericAlias(alias) => visit.extend([alias.origin, alias.args, alias.parameters]),
            Self::Instance {
                class,
                fields,
                dictionary,
                scalar,
                memory_stream,
                property,
            } => {
                if let Some(property) = property {
                    property.children(visit);
                }
                if let Some(super::stdlib::io::MemoryStream::Bytes(stream)) = memory_stream {
                    visit.extend(stream.snapshot.get());
                }
                visit.push(*class);
                visit.extend(dictionary);
                visit.extend(fields.values());
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
            Self::ReadOnlyDescriptor { get, .. } => visit.push(*get),
            Self::Partial {
                function,
                args,
                keywords,
                coroutine_marker,
            } => {
                visit.extend([*function, *args, *keywords]);
                visit.extend(coroutine_marker);
            }
            Self::Cached { function, entries } => {
                visit.push(*function);
                for call in entries {
                    visit.extend(&call.args);
                    visit.extend(call.kwargs.iter().map(|(_, id)| *id));
                    visit.push(call.result);
                }
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
    remaining: usize,
}

impl DictIterator {
    /// Capture the original size while keeping the entries live.
    pub(super) fn new(dict: Id, kind: DictViewKind, size: usize) -> Self {
        Self {
            dict: Some(dict),
            kind,
            expected_len: Some(size),
            position: 0,
            remaining: size,
        }
    }

    /// An invalid or exhausted iterator has no remaining length hint.
    pub(super) fn length_hint(&self, ctx: &Context) -> usize {
        self.dict
            .and_then(|dict| ctx.dict_entries(dict))
            .filter(|entries| Some(entries.len()) == self.expected_len)
            .map_or(0, |_| self.remaining)
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
        if self.expected_len != Some(entries.len()) {
            self.expected_len = None;
            return Err(Error::new(
                "RuntimeError",
                "dictionary changed size during iteration",
            ));
        }
        let Some((position, key, value)) = entries.at(self.position) else {
            self.dict = None;
            return Ok(None);
        };
        if self.remaining == 0 {
            self.dict = None;
            return Err(Error::new(
                "RuntimeError",
                "dictionary keys changed during iteration",
            ));
        }
        self.position = position + 1;
        self.remaining -= 1;
        Ok(Some(match self.kind {
            DictViewKind::Keys => key,
            DictViewKind::Values => value,
            DictViewKind::Items => ctx.alloc(Value::Tuple(vec![key, value])),
        }))
    }
}

/// Native frames hold temporaries until the next Python statement. A caller's
/// partial expression remains rooted while evaluating a nested native function.
pub(super) struct Heap {
    slots: Vec<Option<Value>>,
    free: Vec<usize>,
    pub(super) immutable_hashes: HashMap<Id, i64>,
    frames: Vec<Frame>,
    next_collection: usize,
    weak: HashMap<Id, Vec<Id>>,
    // A dead proxy is still observable after its registry entry disappears.
    pub(super) has_weak_values: bool,
    weak_edge_removed: bool,
}

struct Frame {
    scope: Id,
    code: Id,
    live: Vec<Id>,
    location: Option<Location>,
    object: Option<Id>,
    inherited: Id,
    active: Id,
}

impl Frame {
    fn new(scope: Id, code: Id, inherited: Id) -> Self {
        Self {
            scope,
            code,
            live: Vec::new(),
            location: None,
            object: None,
            inherited,
            active: inherited,
        }
    }
}

impl Heap {
    pub fn new() -> Self {
        Self {
            slots: vec![
                Some(Value::None),
                Some(Value::Bool(false)),
                Some(Value::Bool(true)),
                Some(Value::Ellipsis),
                Some(Value::NotImplemented),
            ],
            free: Vec::new(),
            immutable_hashes: HashMap::new(),
            frames: vec![Frame::new(NONE, NONE, NONE)],
            next_collection: 4096,
            weak: HashMap::new(),
            has_weak_values: false,
            weak_edge_removed: false,
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
    ) -> Id {
        let references = self.weak_references(target);
        if callback == NONE
            && let Some(&id) = references.iter().find(|&&id|
                matches!(self.get(id), Value::Weak(wr) if wr.kind == kind && wr.callback == NONE))
        {
            return id;
        }
        // Callback-free canonical references precede proxies, then callbacks in newest-first order.
        let index = references
            .iter()
            .position(|&id| {
                let Value::Weak(wr) = self.get(id) else {
                    unreachable!()
                };
                wr.callback != NONE || (callback == NONE && kind == super::weakref::Kind::Reference)
            })
            .unwrap_or(references.len());
        let id = self.alloc(Value::Weak(super::weakref::Reference {
            target: Some(target),
            callback,
            kind,
            hash: None,
        }));
        self.weak.entry(target).or_default().insert(index, id);
        self.has_weak_values = true;
        // The caller may hold either the target or the new wrapper only as a temporary.
        self.weak_edge_removed = true;
        id
    }

    fn weak_edge(&self, value: Id) -> bool {
        self.weak.contains_key(&value)
            || matches!(self.get(value), Value::Weak(wr) if wr.target.is_some())
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

    pub fn weak_edge_removed(&mut self, value: Id) {
        if self.has_weak_values
            && (self.weak_edge(value) || matches!(self.get(value), Value::Function { .. }))
            && !self.bound_in_active_frame(value)
        {
            // A dropped function may have held the last strong path through its captured scope.
            self.weak_edge_removed = true;
        }
    }

    pub fn alloc(&mut self, value: Value) -> Id {
        let index = if let Some(index) = self.free.pop() {
            self.slots[index] = Some(value);
            index
        } else {
            self.slots.push(Some(value));
            self.slots.len() - 1
        };
        let id = index as Id;
        self.keep(id);
        id
    }

    pub fn get(&self, id: Id) -> &Value {
        self.slots[id as usize]
            .as_ref()
            .expect("live native handle")
    }
    pub fn get_mut(&mut self, id: Id) -> &mut Value {
        self.slots[id as usize]
            .as_mut()
            .expect("live native handle")
    }
    pub fn enter(&mut self, scope: Id, code: Id) {
        self.frames
            .push(Frame::new(scope, code, self.active_exception()));
    }
    pub fn leave(&mut self, value: Id) {
        let frame = self.frames.pop().expect("returning native activation");
        if self.has_weak_values
            && let Value::Scope(scope) = self.get(frame.scope)
        {
            let released =
                frame.live.iter().chain(scope.names.values()).any(|&id| {
                    id != value && self.weak_edge(id) && !self.bound_in_active_frame(id)
                });
            if released {
                // A normal function allocates its scope in the caller before entering.
                // Once it returns, only an escaped closure or frame can keep that scope alive.
                if scope.parent.is_some() && scope.class_body.is_none() {
                    let caller = &mut self.frames.last_mut().unwrap().live;
                    if let Some(index) = caller.iter().position(|&id| id == frame.scope) {
                        caller.swap_remove(index);
                    }
                }
                self.weak_edge_removed = true;
            }
        }
        self.keep(value);
    }
    pub fn keep(&mut self, value: Id) {
        self.frames.last_mut().unwrap().live.push(value);
    }

    /// Mark temporary roots that must survive the next iteration of a native loop.
    pub fn temporary_mark(&self) -> usize {
        self.frames.last().unwrap().live.len()
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

    pub fn current_scope(&self) -> Id {
        self.frames.last().unwrap().scope
    }

    pub fn current_location(&self) -> Option<Location> {
        self.frames.last()?.location.clone()
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
            && let Some(Value::PythonFrame { lineno, .. }) = &mut self.slots[id as usize]
        {
            *lineno = location.line;
        }
        frame.location = Some(location);
        frame.live.clear();
        frame.live.extend(live);
        if (!self.has_weak_values || !std::mem::take(&mut self.weak_edge_removed))
            && (self.slots.len() < self.next_collection || !self.free.is_empty())
        {
            return None;
        }
        let mut pending: Vec<_> = globals.collect();
        pending.extend([NONE, FALSE, TRUE, ELLIPSIS, NOT_IMPLEMENTED]);
        for frame in &self.frames {
            pending.extend([frame.scope, frame.code, frame.active, frame.inherited]);
            pending.extend(frame.object);
            pending.extend(&frame.live);
        }
        let mut marked = vec![false; self.slots.len()];
        while let Some(id) = pending.pop() {
            if !std::mem::replace(&mut marked[id as usize], true) {
                self.get(id).children(&mut pending);
            }
        }
        // Clear non-owning handles before any dead target slot can be reused. Dead wrappers
        // never receive callbacks, and every live wrapper is cleared before any callback runs.
        let mut callbacks = Vec::new();
        self.weak.retain(|&target, refs| {
            refs.retain(|&id| marked[id as usize]);
            if marked[target as usize] {
                return !refs.is_empty();
            }
            for &id in refs.iter() {
                let Some(Value::Weak(wr)) = &mut self.slots[id as usize] else {
                    unreachable!()
                };
                wr.target = None;
                let callback = std::mem::replace(&mut wr.callback, NONE);
                if callback != NONE {
                    callbacks.push((callback, id));
                }
            }
            false
        });
        for (id, alive) in marked.into_iter().enumerate() {
            if !alive && self.slots[id].take().is_some() {
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
        assert!(heap.slots[key as usize].is_none());
        assert!(heap.slots[value as usize].is_none());
    }
}
