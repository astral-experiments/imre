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
    pub filename: Id,
    pub first_line: u32,
    pub module_definition: bool,
    pub coroutine: bool,
    pub nested: Vec<(usize, Id)>,
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

    pub(super) fn roots(&self) -> impl Iterator<Item = Id> + '_ {
        self.args
            .iter()
            .flatten()
            .copied()
            .chain(self.trace.iter().flat_map(|trace| {
                trace
                    .frames
                    .iter()
                    .map(|&(_, frame)| frame)
                    .chain(trace.instance)
                    .chain(trace.context)
                    .chain(trace.cause)
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
                super::stdlib::BinaryFile::Reader(_) => "_io.BufferedReader",
                super::stdlib::BinaryFile::Writer(_) => "_io.BufferedWriter",
            },
            Value::TextFile(_) => "_io.TextIOWrapper",
            Value::CodecInfo { .. } => "CodecInfo",
            Value::Struct { .. } => "Struct",
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
            Value::Code(_) => "code",
            Value::Generator { .. } => "generator",
            Value::TypeVar(_) => "TypeVar",
            Value::GenericAlias(alias) => alias.kind.type_name(),
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

#[derive(Clone)]
pub(super) struct Scope {
    pub class_body: Option<Vec<String>>,
    pub is_abstract: bool,
    pub class_cell: Option<Id>,
    pub class_lineage: Option<(Id, Id)>,
    pub first_argument: Option<String>,
    pub names: indexmap::IndexMap<String, Id>,
    pub namespace: Option<Id>,
    pub globals: HashSet<String>,
    pub exec_globals: Option<ExecGlobals>,
    pub exec_locals: Option<Id>,
    pub parent: Option<Id>,
    pub module: String,
}

#[derive(Clone, Copy)]
pub(super) struct ExecGlobals {
    pub dict: Id,
    pub builtins: Id,
}

#[derive(Clone)]
pub(super) enum Value {
    None,
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
        items: Vec<Id>,
        frozen: bool,
    },
    Dict(Vec<(Id, Id)>),
    MappingProxy(Id),
    MappingView {
        mapping: Id,
        kind: DictViewKind,
    },
    DictView {
        dict: Id,
        kind: DictViewKind,
    },
    DictIterator(DictIterator),
    DefaultDict {
        factory: Id,
        entries: Vec<(Id, Id)>,
    },
    Scope(Scope),
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
        code: Id,
        scope: Id,
        class_cell: Option<Id>,
        defaults: Id,
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
    },
    TypeVar(super::typing::TypeVariable),
    GenericAlias(super::typing::Alias),
    Instance {
        class: Id,
        fields: HashMap<String, Id>,
        dictionary: Option<Id>,
        scalar: Option<Id>,
        memory_stream: Option<super::stdlib::io::MemoryStream>,
    },
    Native(String),
    AttrGetter(std::rc::Rc<[Vec<String>]>),
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
    Cached {
        function: Id,
        entries: Vec<CachedCall>,
    },
    Yielded(Id),
    AstWalk(std::collections::VecDeque<Id>),
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
        input: Id,
    },
    Zip {
        inputs: std::rc::Rc<[Id]>,
        strict: bool,
        pending: Option<Id>,
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
    File(std::rc::Rc<super::stdlib::BinaryFile>),
    TextFile(crate::objects::TextFile),
    RegexPattern(std::rc::Rc<super::stdlib::regex::Pattern>),
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
    fn children(&self, visit: &mut Vec<Id>) {
        match self {
            Self::List(v) | Self::Tuple(v) | Self::Set { items: v, .. } => visit.extend(v),
            Self::Iterator { sequence, .. } => visit.extend(*sequence),
            Self::DictIterator(iter) => visit.extend(iter.dict),
            Self::FormatIterator { input, .. } => visit.push(*input),
            Self::ClassMethod(callable) => visit.push(*callable),
            Self::ThreadLocal(local) => visit.extend(local.live()),
            Self::Count { current, step } => visit.extend([*current, *step]),
            Self::Repeat(repeat) => visit.push(repeat.value),
            Self::Map { function, input } => visit.extend([*function, *input]),
            Self::Zip {
                inputs, pending, ..
            } => {
                visit.extend(inputs.iter());
                visit.extend(pending);
            }
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
            Self::DictView { dict, .. } | Self::MappingProxy(dict) => visit.push(*dict),
            Self::Hmac(mac) => {
                visit.extend([mac.inner, mac.outer, mac.digest_size, mac.block_size])
            }
            Self::Context(bindings) => {
                for (key, value) in bindings.iter() {
                    visit.extend([*key, *value]);
                }
            }
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
            Self::AstWalk(queue) => visit.extend(queue),
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
                visit.extend(scope.class_cell);
                if let Some((bases, mro)) = scope.class_lineage {
                    visit.extend([bases, mro]);
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
            Self::Function {
                scope,
                class_cell,
                code,
                defaults,
                fields,
                ..
            } => {
                visit.extend([*scope, *code, *defaults]);
                visit.extend(class_cell);
                fields.children(visit);
            }
            Self::Code(code) => {
                visit.extend([code.name, code.filename, code.parameters]);
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
            } => {
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
            | Self::TokenMissing
            | Self::File(_)
            | Self::TextFile(_) => {}
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
    index: usize,
}

impl DictIterator {
    /// Capture the original size while keeping the entries live.
    pub(super) fn new(dict: Id, kind: DictViewKind, size: usize) -> Self {
        Self {
            dict: Some(dict),
            kind,
            expected_len: Some(size),
            index: 0,
        }
    }

    /// An invalid or exhausted iterator has no remaining length hint.
    pub(super) fn length_hint(&self, ctx: &Context) -> usize {
        self.dict
            .and_then(|dict| ctx.dict_entries(dict))
            .filter(|entries| Some(entries.len()) == self.expected_len)
            .map_or(0, |entries| entries.len() - self.index)
    }

    /// Check the live size before each pull, including the one that reaches exhaustion.
    pub(super) fn next(&mut self, ctx: &mut Context) -> Result<Option<Id>> {
        let Some(dict) = self.dict else {
            return Ok(None);
        };
        let entries = ctx.dict_entries(dict).expect("dictionary iterator source");
        if self.expected_len != Some(entries.len()) {
            self.expected_len = None;
            return Err(Error::new(
                "RuntimeError",
                "dictionary changed size during iteration",
            ));
        }
        let Some(&(key, value)) = entries.get(self.index) else {
            self.dict = None;
            return Ok(None);
        };
        self.index += 1;
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
    // ABC registrations are weak edges. Sweep them before recycling a dead class's slot.
    pub(super) abc_registrations: Vec<[Id; 2]>,
    free: Vec<usize>,
    frames: Vec<Frame>,
    next_collection: usize,
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
            abc_registrations: Vec::new(),
            free: Vec::new(),
            frames: vec![Frame::new(NONE, NONE, NONE)],
            next_collection: 4096,
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
        self.frames.pop();
        self.keep(value);
    }
    pub fn keep(&mut self, value: Id) {
        self.frames.last_mut().unwrap().live.push(value);
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
    ) {
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
        if self.slots.len() < self.next_collection || !self.free.is_empty() {
            return;
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
        self.abc_registrations
            .retain(|&[owner, member]| marked[owner as usize] && marked[member as usize]);
        for (id, alive) in marked.into_iter().enumerate() {
            if !alive && self.slots[id].take().is_some() {
                self.free.push(id);
            }
        }
        self.next_collection = (self.slots.len() - self.free.len())
            .saturating_mul(2)
            .max(4096);
    }
}
