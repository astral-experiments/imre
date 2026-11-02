//! Values shared by native functions loading Python modules through `sys.path`.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// CPython flags for boxed Python function definitions.
    pub flags: Option<u32>,
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
const EMPTY_TUPLE: Id = 5;

pub(super) const REGEX_PATTERN_TYPE: &str = "re.Pattern";
pub(super) const REGEX_MATCH_TYPE: &str = "re.Match";
pub(super) const TEMPLATE_TYPE: &str = "string.templatelib.Template";
pub(super) const TEMPLATE_ITER_TYPE: &str = "string.templatelib.TemplateIter";
pub(super) const INTERPOLATION_TYPE: &str = "string.templatelib.Interpolation";

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
    pub cause: Option<Id>,
    pub import_message: Option<Id>,
    /// Live members belong to the exception instance across traceback clones.
    pub source_syntax: Option<crate::shared::Shared<[std::cell::Cell<Id>; 9]>>,
    pub import_names: Option<[Id; 3]>,
    pub name_error_name: Option<Id>,
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
    pub kind: std::borrow::Cow<'static, str>,
    /// The Python exception class, or NONE for a native error.
    pub class: Id,
    pub message: String,
    pub args: Option<Id>,
    pub decode: Option<Box<DecodeState>>,
    pub os_args: Option<crate::shared::Shared<super::os_error::Args>>,
    // Keep the context reachable even if the optional traceback cannot be allocated.
    pub context: Id,
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
            kind: if kind == "MemoryError" {
                std::borrow::Cow::Borrowed("MemoryError")
            } else {
                std::borrow::Cow::Owned(kind.to_owned())
            },
            class: NONE,
            message: message.into(),
            args: None,
            decode: None,
            os_args: None,
            context: NONE,
            trace: None,
            exit_status: None,
            import: None,
        }
    }

    /// Construct an error without copying a static exception name.
    pub fn new_static(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: std::borrow::Cow::Borrowed(kind),
            ..Self::new("", message)
        }
    }

    /// Read the class handle of an exception constructed from a Python class.
    pub(super) fn class(&self) -> Option<Id> {
        (self.class != NONE).then_some(self.class)
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
            .chain(self.class())
            .chain((self.context != NONE).then_some(self.context))
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
                    .chain(trace.cause)
                    .chain(trace.import_message)
                    .chain(
                        trace
                            .source_syntax
                            .iter()
                            .flat_map(|fields| fields.iter().map(std::cell::Cell::get)),
                    )
                    .chain(trace.import_names.into_iter().flatten())
                    .chain(trace.name_error_name)
                    .chain(trace.dictionary)
                    .chain(trace.cached.map(|(_, head)| head))
            }))
    }

    /// Acquire the cold metadata while an error is already propagating.
    fn try_tracing(&mut self) -> Option<&mut Trace> {
        if self.trace.is_none() {
            self.trace = Some(allocator_api2::boxed::Box::try_new(Trace::default()).ok()?);
        }
        self.trace.as_deref_mut()
    }

    pub(super) fn clear_traceback(&mut self) {
        if let Some(trace) = self.trace.as_mut() {
            trace.frames.clear();
            trace.cached = None;
        }
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

    /// Return the same Python frame to introspection and traceback construction.
    pub(super) fn active_frame(&mut self, depth: usize) -> Result<Option<(Location, Id)>> {
        let Some((index, _)) = self
            .heap
            .frames
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, frame)| frame.code != NONE)
            .nth(depth)
        else {
            return Ok(None);
        };
        self.materialize_frame(index)
    }

    fn materialize_frame(&mut self, index: usize) -> Result<Option<(Location, Id)>> {
        let frame = &self.heap.frames[index];
        if frame.code == NONE {
            return Ok(None);
        }
        let Some(location) = frame.location.clone() else {
            return Ok(None);
        };
        if let Some(object) = frame.object {
            return Ok(Some((location, object)));
        }
        // An escaped ordinary frame owns its caller even after both functions return.
        // Materialize the missing prefix iteratively, stopping at a cached frame or
        // a generator whose resumer is meaningful only during this activation.
        let start = self.heap.frames[..=index]
            .iter()
            .rposition(|frame| {
                frame.object.is_some()
                    || (matches!(frame.owner, FrameOwner::Generator(_)) && frame.location.is_some())
            })
            .unwrap_or(0);
        let mut previous = NONE;
        for position in start..=index {
            let frame = &self.heap.frames[position];
            if let Some(object) = frame.object {
                previous = object;
                continue;
            }
            let Some(line) = frame.location.as_ref().map(|loc| loc.line) else {
                continue;
            };
            if frame.code == NONE {
                continue;
            }
            let (scope, code, owner) = (frame.scope, frame.code, frame.owner);
            let globals = self.module_globals(scope)?;
            let object = self.alloc(Value::PythonFrame {
                scope,
                code,
                globals,
                back: (!matches!(owner, FrameOwner::Generator(_))).then_some(previous),
                lineno: line,
            });
            if let FrameOwner::Generator(id) = owner {
                let Value::Generator { frame, .. } = self.heap.get_mut(id) else {
                    unreachable!()
                };
                *frame = Some(object);
            }
            self.heap.frames[position].object = Some(object);
            previous = object;
        }
        Ok(Some((location, previous)))
    }

    /// A suspended or finished generator has no resumer; an active one may have a new caller.
    pub(super) fn generator_frame_back(&mut self, object: Id) -> Result<Id> {
        let Some(index) = self
            .heap
            .frames
            .iter()
            .rposition(|frame| frame.object == Some(object))
        else {
            return Ok(NONE);
        };
        let Some(caller) = self.heap.frames[..index]
            .iter()
            .rposition(|frame| frame.code != NONE && frame.location.is_some())
        else {
            return Ok(NONE);
        };
        Ok(self.materialize_frame(caller)?.map_or(NONE, |(_, id)| id))
    }

    pub(super) fn record_exception_frame(&mut self, error: &mut Error, span: Option<&[u32; 4]>) {
        let failure = match self.materialize_frame(self.heap.frames.len() - 1) {
            Err(failure) => failure,
            Ok(None) => return,
            Ok(Some((mut loc, frame))) => {
                if let Some(&[line, start, split, end]) = span {
                    loc.line = line;
                    loc.span = Some([start, split, end]);
                }
                let active = self.heap.active_exception();
                if error.context == NONE
                    && active != NONE
                    && error.trace.as_ref().is_none_or(|trace| {
                        trace.frames.is_empty() && trace.instance != Some(active)
                    })
                {
                    error.context = active;
                }
                if let Some(trace) = error.try_tracing()
                    && trace.frames.try_reserve(1).is_ok()
                {
                    trace.frames.push((loc, frame));
                    return;
                }
                Error::new("MemoryError", "")
            }
        };
        let original = std::mem::replace(error, failure);
        error.context = self.raised_instance(original);
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

    /// Adopt the exact Python traceback chain and replace any previously retained frames.
    pub(super) fn set_traceback(&mut self, exception: Id, tb: Id) -> Result<()> {
        let Value::Raised(error) = self.heap.get_mut(exception) else {
            return Err(Error::new("TypeError", "expected a BaseException instance"));
        };
        if tb == NONE {
            error.clear_traceback();
            self.heap.collect_next_statement = true;
            return Ok(());
        }
        let mut frames = Vec::new();
        let mut node = tb;
        while node != NONE {
            let Value::Traceback {
                frame,
                next,
                lineno,
                span,
            } = *self.heap.get(node)
            else {
                return Err(Error::new(
                    "TypeError",
                    "__traceback__ must be a traceback or None",
                ));
            };
            let Value::PythonFrame { code, .. } = *self.heap.get(frame) else {
                unreachable!("traceback retains a frame")
            };
            let Value::Code(code) = self.heap.get(code) else {
                unreachable!("frame retains its code object")
            };
            let file = self.str_value(code.filename)?.to_owned().into();
            frames
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            frames.push((
                Location {
                    file,
                    line: lineno as u32,
                    span,
                },
                frame,
            ));
            node = next;
        }
        frames.reverse();
        let Value::Raised(error) = self.heap.get_mut(exception) else {
            unreachable!()
        };
        let trace = error.tracing();
        trace.cached = Some((frames.len(), tb));
        trace.frames = frames;
        self.heap.collect_next_statement = true;
        Ok(())
    }

    pub(super) fn type_name(&self, value: Id) -> &str {
        match self.heap.get(value) {
            Value::Calendar { value, .. } => value.class(),
            Value::Poll(_) => "select.poll",
            Value::Count { .. } => "count",
            Value::Repeat(_) => "repeat",
            Value::CodecInfo { .. } => "CodecInfo",
            Value::Struct { .. } => "Struct",
            Value::NamedFileCloser(_) => "tempfile._TemporaryFileCloser",
            Value::BufferedRandom(..) => "_io.BufferedRandom",
            Value::NamedTuple {
                class: Some(class), ..
            } => class,
            Value::NamedTuple { .. } => "tuple",
            Value::DefaultDict { .. } => "defaultdict",
            Value::TupleGetter { .. } => "_tuplegetter",
            Value::TypeVar(_) => "TypeVar",
            Value::TypeVarTuple { .. } => "TypeVarTuple",
            Value::GenericAlias(alias) => alias.kind.type_name(),
            Value::Union(_) => "Union",
            Value::Method { .. } => "method",
            Value::Cached { .. } => "_lru_cache_wrapper",
            Value::Partial { .. } => "partial",
            Value::PartialPlaceholder => "_PlaceholderType",
            Value::KeyWrapper { .. } => "functools.KeyWrapper",
            Value::Raised(error) => &error.kind,
            Value::Module {
                constructor: None, ..
            } => "module",
            Value::Instance { class, .. }
                if self.classes.get(super::namespace::CLASS) == Some(class) =>
            {
                super::namespace::CLASS
            }
            Value::Instance { class, .. }
            | Value::Module {
                constructor: Some(class),
                ..
            } => match self.heap.get(*class) {
                Value::Class { .. } | Value::Native(_) => self.class_name(*class),
                _ => "object",
            },
            value => value.intrinsic_type_name().unwrap_or("object"),
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
    /// Closure cell shared by runtime-source method annotation providers.
    pub annotation_dict_cell: Option<Id>,
    /// Scratch field dictionary used while a class body is executing.
    pub annotation_scratch: Option<Id>,
    /// Executed deferred sites in source order, including repeated field names.
    pub annotation_sites: Option<Box<std::collections::BTreeMap<u32, (Id, Id)>>>,
    /// Class annotation attributes are served by the installed provider.
    pub provider_annotations: bool,
    /// Slot names consumed when the class is created, independent of later namespace mutations.
    pub slot_names: Option<std::rc::Rc<[String]>>,
    /// Dynamic classes install real descriptors instead of using inline fields by spelling.
    pub member_slots: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ComprehensionKind {
    ClassBody,
    Ordinary,
}

#[derive(Clone)]
pub(super) struct Scope {
    pub class_body: Option<Vec<String>>,
    pub enum_namespace: Option<Box<super::enums::auto::EnumNamespace>>,
    pub is_abstract: bool,
    pub comprehension: Option<ComprehensionKind>,
    pub class_cell: Option<Id>,
    pub class_namespace: Option<Box<ClassNamespace>>,
    pub first_argument: Option<String>,
    pub names: indexmap::IndexMap<String, Id>,
    pub closure: allocator_api2::boxed::Box<ClosureScope>,
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
        // Preserve the fixed timezone name and its Python str subclass methods.
        tzname: Id,
    },
    Poll(std::rc::Rc<std::cell::RefCell<crate::objects::readiness::Poll>>),
    Scandir(std::rc::Rc<super::stdlib::os::scandir::Scan>),
    DirEntry(super::stdlib::os::scandir::Entry),
    Selector(std::rc::Rc<std::cell::RefCell<super::stdlib::readiness::Selector>>),
    Slice([Id; 3]),
    Template {
        strings: Id,
        interpolations: Id,
    },
    TemplateIter {
        strings: Id,
        interpolations: Id,
        from_strings: bool,
    },
    Interpolation {
        value: Id,
        expression: Id,
        conversion: Id,
        format_spec: Id,
    },
    Range(super::range::Range),
    RangeIterator(super::range::Cursor),
    Str(String),
    /// Python code points for text containing at least one surrogate.
    SurrogateStr(Vec<u32>),
    Array(super::array::Array),
    Bytes(Vec<u8>),
    /// A contiguous read-only view keeps its original bytes exporter alive until release.
    MemoryView {
        owner: Option<Id>,
        hash: Option<i64>,
    },
    ByteArray(Vec<u8>),
    CodecInfo {
        entry: crate::objects::CodecInfo,
        decode: Id,
    },
    CodecDecoder(crate::objects::Codec),
    Hash(crate::objects::SharedHash),
    Hmac(Hmac),
    Struct {
        format: String,
        layout: std::rc::Rc<crate::objects::interpreter_stdlib::StructFormat>,
    },
    LoggerHandle(std::rc::Rc<crate::objects::logging::Logger>),
    MetadataArchive(std::rc::Rc<crate::objects::ZipArchive>),
    HandlerLock(std::sync::Arc<crate::objects::logging::HandlerLock>),
    Socket(crate::ObjectOwner),
    Deque(Box<crate::objects::DequeRing<Id>>),
    SequenceIterator {
        source: Option<Id>,
        index: i64,
    },
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
        // Exception class retained by this generation of the _locale module.
        locale_error: Option<Id>,
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
    // The coroutine owns its private suspended frame. Each await iterator owns the coroutine.
    Coroutine(Id),
    CoroutineWrapper(Id),
    Generator {
        owner: FrameOwner,
        frame: Option<Id>,
        code: Id,
        scope: Id,
        iterators: Vec<Id>,
        protected: Vec<super::source_code::FinallyBlock>,
        offset: Option<usize>,
        running: bool,
    },
    Class {
        /// Private layout tag for native I/O class generations.
        io_kind: Option<&'static str>,
        metaclass: Option<Id>,
        name: String,
        scope: Id,
        ancestors: Vec<Id>,
        namedtuple_fields: Option<std::rc::Rc<[String]>>,
    },
    TypeVar(super::typing::TypeVariable),
    TypeVarTuple {
        name: Id,
        module: Id,
        default: Id,
    },
    NoDefault,
    GenericAlias(super::typing::Alias),
    Union(super::union::Union),
    Instance {
        class: Id,
        fields: indexmap::IndexMap<String, Id>,
        /// Storage addressed by the declaring member, independent of the instance dictionary.
        member_values: Option<Box<HashMap<Id, Id>>>,
        /// Exact ordinary instance dictionary once exposed or replaced.
        namespace: Option<Id>,
        dictionary: Option<Id>,
        scalar: Option<Id>,
        property: Option<Box<super::property::Property>>,
        weak_reference: Option<Box<super::weakref::Reference>>,
        cycle: Option<Box<super::stdlib::itertools::Cycle>>,
        memory_stream: Option<super::stdlib::io::MemoryStream>,
    },
    Native(String),
    AttrGetter(std::rc::Rc<[Vec<String>]>),
    /// A tuple field position and the descriptor’s writable docstring.
    TupleGetter {
        index: i64,
        doc: Option<Id>,
    },
    /// The writable document member of the native tuple descriptor type.
    TupleGetterDoc,
    ReadOnlyMember(&'static str, &'static str),
    SlotMember {
        owner: Id,
        name: Id,
    },
    /// The original keys are held in a Python tuple, shared by multi-key reductions.
    ItemGetter(Id),
    ThreadLocal(super::thread_local::Dictionaries),
    ThreadHandle(std::rc::Rc<super::thread_worker::Handle>),
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
        globals: Id,
        // Ordinary frames retain their caller. Generator frames resolve their live resumer.
        back: Option<Id>,
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
        dictionary: Option<Id>,
    },
    PartialPlaceholder,
    KeyWrapper {
        comparator: Id,
        object: Option<Id>,
    },
    CacheDecorator {
        maxsize: Id,
        typed: Id,
    },
    CacheParameters {
        maxsize: Id,
        typed: Id,
    },
    Cached {
        function: Id,
        capacity: Option<usize>,
        typed: bool,
        hits: i64,
        misses: i64,
        dictionary: Id,
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
    ProcessHandle(crate::objects::BoxedPopen),
    File(crate::shared::Shared<super::stdlib::BinaryFile>),
    TextFile(crate::objects::TextFile),
    RegexScanner {
        pattern: Id,
        subject: Id,
        start: Option<usize>,
        end: usize,
        retry: bool,
    },
    RegexPattern(std::rc::Rc<super::stdlib::regex::Pattern>),
    NamedFileCloser(crate::Shared<std::cell::RefCell<super::stdlib::tempfile::NamedCloser>>),
    BufferedRandom(
        crate::Shared<std::cell::RefCell<crate::objects::FileState>>,
        Id,
    ),
    RegexMatch(std::rc::Rc<super::stdlib::regex::Match>),
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

pub(super) const CACHE_CAPACITY: usize = 128;

#[derive(Clone)]
pub(super) struct CachedCall {
    pub key: Id,
    pub hash: i64,
}

impl Value {
    /// Class of a native file whose Python descriptors are installed by _io.
    pub(super) fn native_io_class_name(&self) -> Option<&'static str> {
        match self {
            Self::TextFile(_) => Some("_io.TextIOWrapper"),
            Self::File(_) if self.intrinsic_type_name() == Some("_io.BufferedReader") => {
                Some("_io.BufferedReader")
            }
            _ => None,
        }
    }

    /// Returns the name used for both builtin class lookup and diagnostic messages.
    pub(super) fn intrinsic_type_name(&self) -> Option<&str> {
        Some(match self {
            Self::Selector(handle) => handle.borrow().class_name(),
            Self::Context(_) => "_contextvars.Context",
            Self::ContextVar { .. } => "_contextvars.ContextVar",
            Self::ContextToken { .. } => "_contextvars.Token",
            Self::None => "NoneType",
            Self::Weak(wr) => wr.kind.class(),
            Self::Ellipsis => "ellipsis",
            Self::NotImplemented => "NotImplementedType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Complex(_) => "complex",
            Self::Str(_) | Self::SurrogateStr(_) => "str",
            Self::Map { .. } => "map",
            Self::Filter { .. } => "filter",
            Self::Hash(state) => state.lock().class_name(),
            Self::Zip { .. } => "zip",
            Self::Array(_) => "array.array",
            Self::Bytes(_) => "bytes",
            Self::MemoryView { .. } => "memoryview",
            Self::ByteArray(_) => "bytearray",
            Self::File(file) => match file.as_ref() {
                super::stdlib::BinaryFile::Descriptor(fd) if fd.reading => "_io.BufferedReader",
                super::stdlib::BinaryFile::Descriptor(_) => "_io.BufferedWriter",
                super::stdlib::BinaryFile::Reader { .. } => "_io.BufferedReader",
                super::stdlib::BinaryFile::Writer { .. } => "_io.BufferedWriter",
            },
            Self::ProcessHandle(_) => "_imre_subprocess.Handle",
            Self::BufferedRandom(..) => "_io.BufferedRandom",
            Self::TextFile(_) => "_io.TextIOWrapper",
            Self::SequenceIterator { .. } => "iterator",
            Self::ReverseSequence { .. } => "reversed",
            Self::Deque(_) => super::deque::CLASS,
            Self::DequeIterator { reverse: true, .. } => super::deque::REVERSE_ITERATOR,
            Self::DequeIterator { .. } => super::deque::ITERATOR,
            Self::List(_) => "list",
            Self::CallableIterator(_) => "callable_iterator",
            Self::RegexPattern(_) => REGEX_PATTERN_TYPE,
            Self::RegexMatch(_) => REGEX_MATCH_TYPE,
            Self::RegexScanner { .. } => "_sre.SRE_Scanner",
            Self::Tuple(_) => "tuple",
            Self::Slice(_) => "slice",
            Self::Template { .. } => TEMPLATE_TYPE,
            Self::TemplateIter { .. } => TEMPLATE_ITER_TYPE,
            Self::Interpolation { .. } => INTERPOLATION_TYPE,
            Self::Range(_) => "range",
            Self::Scandir(_) => super::stdlib::os::scandir::ITER,
            Self::DirEntry(_) => super::stdlib::os::scandir::ENTRY,
            Self::RangeIterator(iter) => iter.kind,
            Self::Enumerate { .. } => "enumerate",
            Self::Iterator { kind, .. } => kind,
            Self::DictIterator(iter) => iter.class_name(),
            Self::Set { frozen: true, .. } => "frozenset",
            Self::Set { frozen: false, .. } => "set",
            Self::Dict(_) => "dict",
            Self::MappingProxy(_) => "mappingproxy",
            Self::Function { .. } => "function",
            Self::TupleGetterDoc | Self::ReadOnlyMember(_, _) | Self::SlotMember { .. } => {
                "member_descriptor"
            }
            Self::ItemGetter(_) => "operator.itemgetter",
            Self::Code(_) => "code",
            Self::Cell(_) | Self::ClassCell(_) => "cell",
            Self::Generator { .. } => "generator",
            Self::Coroutine(_) => "coroutine",
            Self::CoroutineWrapper(_) => "coroutine_wrapper",
            Self::NoDefault => "NoDefaultType",
            Self::Class { .. } => "type",
            Self::DictView { kind, ordered, .. } => match (kind, ordered) {
                (DictViewKind::Keys, false) => "dict_keys",
                (DictViewKind::Items, false) => "dict_items",
                (DictViewKind::Values, false) => "dict_values",
                (DictViewKind::Keys, true) => "odict_keys",
                (DictViewKind::Items, true) => "odict_items",
                (DictViewKind::Values, true) => "odict_values",
            },
            Self::ClassMethod(_) => "classmethod",
            Self::ThreadLocal(_) => super::thread_local::CLASS,
            Self::ThreadHandle(_) => "_thread._ThreadHandle",
            Self::HandlerLock(_) => super::thread_rlock::CLASS,
            Self::PrimitiveLock(_) => super::thread_lock::CLASS,
            Self::FormatIterator { kind, .. } => match kind {
                FormatKind::Markup => "formatteriterator",
                FormatKind::Field => "fieldnameiterator",
            },
            Self::StaticMethod { .. } => "staticmethod",
            Self::Property(_) => "property",
            Self::GetSetDescriptor { .. } | Self::WeakrefDescriptor(_) => "getset_descriptor",
            Self::Traceback { .. } => "traceback",
            Self::PythonFrame { .. } => "frame",
            _ => return None,
        })
    }

    pub(super) fn io_kind(&self) -> Option<&'static str> {
        match self {
            Self::Class { io_kind, .. } => *io_kind,
            _ => None,
        }
    }

    pub(super) fn instance_class(&self) -> Option<Id> {
        match self {
            Self::Instance { class, .. } => Some(*class),
            Self::Class { metaclass, .. } => *metaclass,
            Self::Module { constructor, .. } => *constructor,
            Self::Raised(error) => error.class(),
            _ => None,
        }
    }

    fn visit_bindings(bindings: &HashMap<Id, Id>, visit: &mut MarkQueue<'_>) {
        for (key, value) in bindings {
            visit.extend([*key, *value]);
        }
    }

    /// Count Python heap objects held by cycles, excluding atomic values and
    /// private activation, iterator and control-flow bookkeeping.
    fn gc_object(&self) -> bool {
        match self {
            Self::None
            | Self::Bool(_)
            | Self::Ellipsis
            | Self::NotImplemented
            | Self::Int(_)
            | Self::Float(_)
            | Self::Complex(_)
            | Self::Bytes(_)
            | Self::ByteArray(_)
            | Self::Str(_)
            | Self::SurrogateStr(_)
            | Self::Range(_)
            | Self::RangeIterator(_)
            | Self::Scandir(_)
            | Self::DirEntry(_)
            | Self::Code(_)
            | Self::Native(_)
            | Self::Scope(_)
            | Self::ClassCell(_)
            | Self::LoopIterator { .. }
            | Self::ProcessHandle(_)
            | Self::Yielded(_) => false,
            Self::Tuple(items) => !items.is_empty(),
            _ => true,
        }
    }

    fn children(&self, visit: &mut MarkQueue<'_>) {
        match self {
            Self::Deque(ring) => visit.extend(&ring.items),
            Self::SequenceIterator { source, .. } | Self::ReverseSequence { source, .. } => {
                visit.extend(*source)
            }
            Self::DequeIterator { deque, .. } => visit.push(*deque),
            Self::List(v) | Self::Tuple(v) => visit.extend(v.iter().rev()),
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
            Self::TupleGetter { doc, .. } => visit.extend(doc),
            Self::ItemGetter(keys) => visit.push(*keys),
            Self::KeyWrapper { comparator, object } => {
                visit.push(*comparator);
                visit.extend(object);
            }
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
                    visit.extend([*value, *key]);
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
            Self::MemoryView { owner, .. } => visit.extend(owner),
            Self::Template {
                strings,
                interpolations,
            }
            | Self::TemplateIter {
                strings,
                interpolations,
                ..
            } => visit.extend([*strings, *interpolations]),
            Self::Interpolation {
                value,
                expression,
                conversion,
                format_spec,
            } => visit.extend([*value, *expression, *conversion, *format_spec]),
            Self::Range(_) | Self::RangeIterator(_) => {}
            Self::AstWalk { queue, .. } => visit.extend(queue),
            Self::Enumerate { input, .. } => visit.push(*input),
            Self::Calendar { tzinfo, tzname, .. } => visit.extend([*tzinfo, *tzname]),
            Self::DefaultDict { factory, entries } => {
                visit.push(*factory);
                for (key, value) in entries {
                    visit.extend([*value, *key]);
                }
            }
            Self::Scandir(scan) => visit.push(scan.source.get()),
            Self::DirEntry(entry) => {
                visit.extend([entry.name, entry.path]);
                visit.extend(entry.cached_objects());
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
                    visit.extend(class.annotation_dict_cell);
                    visit.extend(class.annotation_scratch);
                    if let Some(sites) = &class.annotation_sites {
                        for &(name, thunk) in sites.values() {
                            visit.extend([name, thunk]);
                        }
                    }
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
                locale_error,
                ..
            } => {
                visit.push(*scope);
                visit.extend(locale_error);
                visit.extend(origin);
                visit.extend(constructor);
            }
            Self::Cell(value) => visit.extend(value),
            Self::Coroutine(frame) | Self::CoroutineWrapper(frame) => visit.push(*frame),
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
                frame,
                code,
                scope,
                iterators,
                protected,
                ..
            } => {
                visit.extend([*code, *scope, owner.root()]);
                visit.extend(*frame);
                visit.extend(iterators);
                visit.extend(
                    protected
                        .iter()
                        .flat_map(super::source_code::FinallyBlock::roots),
                );
            }
            Self::Class {
                scope,
                ancestors,
                metaclass,
                ..
            } => {
                visit.extend(*metaclass);
                visit.push(*scope);
                visit.extend(ancestors);
            }
            Self::TypeVarTuple {
                name,
                module,
                default,
            } => visit.extend([*name, *module, *default]),
            Self::TypeVar(var) => visit.extend([var.name, var.module, var.bound, var.constraints]),
            Self::GenericAlias(alias) => {
                visit.extend([alias.origin, alias.args, alias.parameters]);
                visit.extend(alias.name);
                if let super::typing::AliasKind::Literal(members) = alias.kind {
                    visit.push(members);
                }
            }
            Self::Union(union) => {
                visit.extend([union.args, union.members, union.parameters]);
                visit.extend(union.unhashable);
            }
            Self::SlotMember { owner, name } => visit.extend([*owner, *name]),
            Self::Instance {
                class,
                fields,
                member_values,
                namespace,
                dictionary,
                scalar,
                weak_reference,
                cycle,
                memory_stream,
                property,
            } => {
                if let Some(property) = property {
                    property.children(visit);
                }
                if let Some(reference) = weak_reference {
                    visit.push(reference.callback);
                }
                if let Some(cycle) = cycle {
                    cycle.children(visit);
                }
                match memory_stream {
                    Some(super::stdlib::io::MemoryStream::BufferedReader(file)) => {
                        visit.push(*file)
                    }
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
                if let Some(values) = member_values {
                    for (&member, &value) in values.iter() {
                        visit.extend([member, value]);
                    }
                }
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
                dictionary,
            } => {
                visit.extend([*function, *args, *keywords]);
                visit.extend(dictionary);
            }
            Self::CacheDecorator { maxsize, typed } | Self::CacheParameters { maxsize, typed } => {
                visit.extend([*maxsize, *typed]);
            }
            Self::Cached {
                function,
                dictionary,
                entries,
                table,
                marker,
                ..
            } => {
                visit.extend([*function, *dictionary, *table, *marker]);
                visit.extend(entries.iter().map(|call| call.key));
            }
            Self::Raised(error) => visit.extend(error.roots()),
            Self::Traceback { frame, next, .. } => visit.extend([*frame, *next]),
            Self::PythonFrame {
                scope,
                code,
                globals,
                back,
                ..
            } => {
                visit.extend([*scope, *code, *globals]);
                visit.extend(*back);
            }
            Self::IoMethod { owner, .. } => visit.push(*owner),
            Self::CodecInfo { decode, .. } => visit.push(*decode),
            Self::RegexScanner {
                pattern, subject, ..
            } => visit.extend([*pattern, *subject]),
            Self::RegexPattern(pattern) => visit.push(pattern.source),
            Self::RegexMatch(m) => visit.extend([m.pattern, m.subject]),
            Self::File(file) => {
                if let super::stdlib::BinaryFile::Descriptor(fd) = file.as_ref() {
                    visit.push(fd.raw);
                } else if let Some((path, _)) = file.path_metadata() {
                    visit.push(path);
                }
            }
            Self::None
            | Self::Ellipsis
            | Self::NotImplemented
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Complex(_)
            | Self::Str(_)
            | Self::SurrogateStr(_)
            | Self::Bytes(_)
            | Self::Array(_)
            | Self::ByteArray(_)
            | Self::CodecDecoder(_)
            | Self::Struct { .. }
            | Self::Hash(_)
            | Self::LoggerHandle(_)
            | Self::MetadataArchive(_)
            | Self::HandlerLock(_)
            | Self::ThreadHandle(_)
            | Self::PrimitiveLock(_)
            | Self::Poll(_)
            | Self::Socket(_)
            | Self::Native(_)
            | Self::AttrGetter(_)
            | Self::TupleGetterDoc
            | Self::ReadOnlyMember(_, _)
            | Self::NoDefault
            | Self::PartialPlaceholder
            | Self::TokenMissing
            | Self::TextFile(_)
            | Self::ProcessHandle(_)
            | Self::NamedFileCloser(_) => {}
            Self::BufferedRandom(_, name) => visit.push(*name),
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
    const KEYS: [&str; 2] = ["dict_keyiterator", "dict_reversekeyiterator"];
    const VALUES: [&str; 2] = ["dict_valueiterator", "dict_reversevalueiterator"];
    const ITEMS: [&str; 2] = ["dict_itemiterator", "dict_reverseitemiterator"];
    const ORDERED: &str = "odict_iterator";

    pub(super) fn methods(class: &str) -> &'static [&'static str] {
        if class == Self::ORDERED {
            &["__iter__", "__next__", "__reduce__"]
        } else {
            &["__iter__", "__next__"]
        }
    }

    pub(super) fn class_name(&self) -> &'static str {
        if self.order_state.is_some() {
            return Self::ORDERED;
        }
        let classes = match self.kind {
            DictViewKind::Keys => Self::KEYS,
            DictViewKind::Values => Self::VALUES,
            DictViewKind::Items => Self::ITEMS,
        };
        classes[usize::from(self.reverse)]
    }

    pub(super) fn class(name: &str) -> Option<&'static str> {
        Self::KEYS
            .into_iter()
            .chain(Self::VALUES)
            .chain(Self::ITEMS)
            .chain([Self::ORDERED])
            .find(|&class| class == name)
    }

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

    /// Advance the cursor before a value lookup can call back into the iterator.
    pub(super) fn next(mut self, ctx: &mut Context, owner: Id) -> Result<Option<Id>> {
        let entry = self.next_entry(ctx, owner);
        *ctx.heap.get_mut(owner) = Value::DictIterator(self);
        let Some((storage, key, mut value)) = entry? else {
            return Ok(None);
        };
        if self.order_state.is_some() && !matches!(self.kind, DictViewKind::Keys) {
            let index = match Self::ordered_index(ctx, storage, key) {
                Ok(index) => index,
                Err(error) => {
                    self.dict = None;
                    *ctx.heap.get_mut(owner) = Value::DictIterator(self);
                    return Err(error);
                }
            };
            value = ctx.stored_entries(storage)[index].1;
        }
        Ok(Some(match self.kind {
            DictViewKind::Keys => key,
            DictViewKind::Values => value,
            DictViewKind::Items => ctx.alloc(Value::Tuple(vec![key, value])),
        }))
    }

    /// Resolve the retained key through the dictionary, including user hash and equality hooks.
    fn ordered_index(ctx: &mut Context, storage: Id, key: Id) -> Result<usize> {
        let (_, index) = ctx.find_dictionary_key(storage, key, None)?;
        index.ok_or_else(|| ctx.lookup_error("KeyError", key))
    }

    /// Read a logical order rank or a plain dictionary slot, in the cursor's direction.
    fn entry_at(
        &self,
        entries: &super::dictionary::Entries,
        position: usize,
    ) -> Option<(usize, Id, Id)> {
        if self.order_state.is_some() {
            let rank = if self.reverse {
                position.checked_sub(1)
            } else {
                Some(position)
            };
            rank.filter(|&rank| rank < entries.len()).map(|rank| {
                let (key, value) = entries[entries.ordered_dense(rank)];
                (rank, key, value)
            })
        } else if self.reverse {
            entries.before(position)
        } else {
            entries.at(position)
        }
    }

    /// Check the live table before advancing. Ordered cursors look their resident key up again.
    fn next_entry(&mut self, ctx: &mut Context, owner: Id) -> Result<Option<(Id, Id, Id)>> {
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
        let entry = self.entry_at(entries, self.position);
        let Some((position, mut key, value)) = entry else {
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
        if self.order_state.is_some() {
            if let Err(error) = Self::ordered_index(ctx, storage, key) {
                self.dict = None;
                return Err(error);
            }
            // Hashing can advance this same iterator. It returns the key now at
            // the cursor, then advances from the entry selected before hashing.
            let Value::DictIterator(live) = ctx.heap.get(owner) else {
                unreachable!()
            };
            if live.position != self.position || live.dict != self.dict {
                let entries = ctx.stored_entries(storage);
                let current = live.entry_at(entries, live.position);
                let Some((_, current_key, _)) =
                    current.filter(|_| live.dict.is_some() && live.remaining != 0)
                else {
                    self.dict = None;
                    return Ok(None);
                };
                key = current_key;
            }
        }
        self.position = if self.reverse { position } else { position + 1 };
        self.remaining = self.remaining.saturating_sub(1);
        Ok(Some((storage, key, value)))
    }
}

// A slot's trace link is UNMARKED until queued. END terminates the queue and marks a
// visited slot. Both sentinels exceed the largest possible Vec index.
const UNMARKED: usize = usize::MAX;
const END: usize = usize::MAX - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Finalization {
    None,
    Pending,
    Done,
}

struct Slot {
    value: Option<Value>,
    trace: Cell<usize>,
    // Next in collection order while occupied, or next free slot while vacant.
    gc_next: Cell<usize>,
    untracked_tuple: Cell<bool>,
    finalization: Finalization,
}

impl Slot {
    fn new(value: Value) -> Self {
        Self {
            value: Some(value),
            trace: Cell::new(UNMARKED),
            gc_next: Cell::new(UNMARKED),
            untracked_tuple: Cell::new(false),
            finalization: Finalization::None,
        }
    }

    fn gc_tracked(&self, heap: &Heap) -> bool {
        match self.value.as_ref() {
            Some(Value::Instance { class, .. }) if matches!(heap.get(*class), Value::Native(name) if name == "object") => {
                false
            }
            Some(value) => !self.untracked_tuple.get() && value.gc_object(),
            None => false,
        }
    }

    fn marked(&self) -> bool {
        self.trace.get() != UNMARKED
    }
}

/// The collector preserves survivor order across full collections and slot reuse.
struct GcOrder {
    head: usize,
    tail: usize,
}

impl GcOrder {
    const EMPTY: Self = Self {
        head: END,
        tail: END,
    };

    fn push(&mut self, id: usize, slots: &[Slot]) {
        debug_assert_eq!(slots[id].gc_next.get(), UNMARKED);
        if self.tail == END {
            self.head = id;
        } else {
            slots[self.tail].gc_next.set(id);
        }
        self.tail = id;
        slots[id].gc_next.set(END);
    }

    fn pop(&mut self, slots: &[Slot]) -> Option<usize> {
        let id = self.head;
        if id == END {
            return None;
        }
        self.head = slots[id].gc_next.replace(UNMARKED);
        if self.head == END {
            self.tail = END;
        }
        Some(id)
    }
}

/// The pending trace lives in heap slots, so looking up a weak intern entry needs no scratch allocation.
pub(super) struct MarkQueue<'a> {
    slots: &'a [Slot],
    next: usize,
    ordered: Option<&'a mut GcOrder>,
}

impl MarkQueue<'_> {
    fn push(&mut self, id: Id) {
        let slot = &self.slots[id as usize];
        if !slot.marked() {
            if let Some(order) = &mut self.ordered {
                slot.trace.set(END);
                if slot.gc_next.get() == UNMARKED {
                    order.push(id as usize, self.slots);
                }
            } else {
                slot.trace.set(self.next);
                self.next = id as usize;
            }
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

/// Callback and weak reference pairs retained for dispatch after a sweep.
pub(super) type WeakCallbacks = Vec<(Id, Id)>;

/// Nonzero incoming edges identify unreachable cycles and their descendants.
/// The workspace is reserved before marking, then reused after callbacks.
struct CycleSet {
    incoming: Vec<usize>,
    work: Vec<usize>,
    cyclic_callbacks: usize,
}

impl CycleSet {
    fn new(len: usize) -> Result<Self> {
        let mut incoming = Vec::new();
        let mut work = Vec::new();
        incoming
            .try_reserve_exact(len)
            .map_err(|_| Error::new("MemoryError", ""))?;
        work.try_reserve_exact(len)
            .map_err(|_| Error::new("MemoryError", ""))?;
        incoming.resize(len, 0);
        Ok(Self {
            incoming,
            work,
            cyclic_callbacks: 0,
        })
    }

    fn members(&self) -> impl Iterator<Item = Id> + '_ {
        self.incoming
            .iter()
            .enumerate()
            .filter_map(|(id, &n)| (n != 0).then_some(id as Id))
    }
}

/// Native frames hold temporaries until the next Python statement. A caller's
/// partial expression remains rooted while evaluating a nested native function.
pub(super) struct Heap {
    slots: Vec<Slot>,
    gc_order: GcOrder,
    linked_string_constants: HashMap<i64, Id>,
    full_collection: Option<CycleSet>,
    // Native owners identify spellings. Runtime-created strings keep only a weak boxed handle.
    interned: HashMap<crate::StrOwner, InternedString>,
    // ABC registrations are weak edges. Sweep them before recycling a dead class's slot.
    pub(super) abc_registrations: Vec<[Id; 2]>,
    // Protocol subclass results use weak keys just like ABC registrations.
    pub(super) protocol_subclass_cache: HashMap<[Id; 2], bool>,
    // Collecting weak registration pairs must not roll back the invalidation counter.
    pub(super) abc_cache_token: u64,
    free_head: usize,
    free_count: usize,
    pub(super) immutable_hashes: HashMap<Id, i64>,
    // Call-history markers belong to list storage and expire when it is collected.
    pub(super) mock_call_lists: HashSet<Id>,
    frames: Vec<Frame>,
    next_collection: usize,
    // Worker heaps share the interpreter’s automatic cyclic collection setting.
    pub(super) automatic_gc: Arc<AtomicBool>,
    weak: HashMap<Id, Vec<Id>>,
    // A dead proxy is still observable after its registry entry disappears.
    pub(super) has_weak_values: bool,
    collect_next_statement: bool,
    // Weak handles to buffered output files returned by open().
    file_writers: Vec<Id>,
    // Weak registrations. The slot remembers prior finalization across resurrection.
    ordinary_finalizers: Vec<Id>,
    // Non-owning positions of a frame, its bound-method operand, and a receiver field.
    caller_finalizer_field: Option<(usize, usize, usize)>,
}

#[derive(Clone, Copy)]
struct InternedString {
    id: Id,
    literal: bool,
}

/// A Python call observes its mutable function, while bare code keeps its entry module.
#[derive(Clone, Copy)]
pub(super) enum FrameOwner {
    Generator(Id),
    Function(Id),
    Module(Id),
}

impl FrameOwner {
    fn root(self) -> Id {
        match self {
            Self::Function(id) | Self::Module(id) | Self::Generator(id) => id,
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
    created_finalizer: bool,
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
            created_finalizer: false,
        }
    }
}

impl Heap {
    pub fn new() -> Self {
        let mut heap = Self {
            slots: vec![
                Slot::new(Value::None),
                Slot::new(Value::Bool(false)),
                Slot::new(Value::Bool(true)),
                Slot::new(Value::Ellipsis),
                Slot::new(Value::NotImplemented),
                Slot::new(Value::Tuple(Vec::new())),
            ],
            gc_order: GcOrder::EMPTY,
            full_collection: None,
            linked_string_constants: HashMap::new(),
            interned: HashMap::new(),
            abc_registrations: Vec::new(),
            protocol_subclass_cache: HashMap::new(),
            abc_cache_token: 0,
            free_head: END,
            free_count: 0,
            immutable_hashes: HashMap::new(),
            mock_call_lists: HashSet::new(),
            frames: vec![Frame::new(NONE, NONE, FrameOwner::Module(NONE), NONE)],
            next_collection: 4096,
            automatic_gc: Arc::new(AtomicBool::new(true)),
            weak: HashMap::new(),
            has_weak_values: false,
            collect_next_statement: false,
            file_writers: Vec::new(),
            ordinary_finalizers: Vec::new(),
            caller_finalizer_field: None,
        };
        for id in 0..heap.slots.len() {
            heap.gc_order.push(id, &heap.slots);
        }
        heap
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
    ) -> Result<Id> {
        let references = self.weak_references(target);
        if subclass.is_none() && callback == NONE
            && let Some(&id) = references.iter().find(|&&id|
                matches!(self.get(id), Value::Weak(wr) if wr.kind == kind && wr.callback == NONE))
        {
            return Ok(id);
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
        let mut new_references = Vec::new();
        if let Some(references) = self.weak.get_mut(&target) {
            references.try_reserve(1)
        } else {
            self.weak
                .try_reserve(1)
                .and_then(|()| new_references.try_reserve(1))
        }
        .map_err(|_| Error::new("MemoryError", ""))?;
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
        self.weak
            .entry(target)
            .or_insert(new_references)
            .insert(index, id);
        self.has_weak_values = true;
        // The caller may hold either the target or the new wrapper only as a temporary.
        self.collect_next_statement = true;
        Ok(id)
    }

    fn weak_edge(&self, value: Id) -> bool {
        self.weak.contains_key(&value)
            || matches!(self.get(value), Value::Method { callable, receiver, .. }
                if self.weak.contains_key(receiver) || self.weak.contains_key(callable))
            || self
                .get(value)
                .weak_reference()
                .is_some_and(|wr| wr.target.is_some())
    }

    /// A name in a surviving frame keeps this object alive until that binding changes.
    fn bound_in_active_frame(&self, value: Id) -> bool {
        self.active_binding_matches(|bound| bound == value)
    }

    /// Inspect names in surviving frames without invoking Python attribute or mapping hooks.
    fn active_binding_matches(&self, mut predicate: impl FnMut(Id) -> bool) -> bool {
        self.frames.iter().rev().any(|frame| {
            let Value::Scope(scope) = self.get(frame.scope) else {
                return false;
            };
            if let Some(namespace) = scope.namespace {
                return matches!(self.get(namespace), Value::Dict(entries)
                    if entries.iter().any(|&(_, bound)| predicate(bound)));
            }
            scope.names.values().copied().any(&mut predicate)
        })
    }

    /// A short strong path proves the writer survives. A miss falls back to a full heap trace.
    fn keeps_finalizer(&self, bound: Id, writer: Id, budget: &mut usize) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        if bound == writer {
            return true;
        }
        match self.get(bound) {
            Value::List(items) | Value::Tuple(items) => items
                .iter()
                .take(*budget)
                .any(|&item| self.keeps_finalizer(item, writer, budget)),
            Value::Dict(items) => items.iter().take(*budget).any(|&(key, value)| {
                self.keeps_finalizer(key, writer, budget)
                    || self.keeps_finalizer(value, writer, budget)
            }),
            _ => false,
        }
    }

    pub(super) fn replace_cell(&mut self, cell: Id, value: Option<Id>) -> Option<Id> {
        let (Value::Cell(contents) | Value::ClassCell(contents)) = self.get_mut(cell) else {
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

    /// An interpreter root (such as packed callback operands) can own weak targets indirectly.
    pub(super) fn observes_released_roots(&self) -> bool {
        self.has_weak_values || self.tracks_finalizers()
    }

    pub fn weak_edge_removed(&mut self, value: Id) {
        if self.has_weak_values
            && (self.weak_edge(value)
                || matches!(
                    self.get(value),
                    Value::Function { .. } | Value::Cell(Some(_)) | Value::ClassCell(Some(_))
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

    /// Linked code owns these immutable constants for the lifetime of its context.
    pub(super) fn linked_string_constant(
        &mut self,
        slot: Id,
        build: impl FnOnce(&mut Self) -> Id,
    ) -> Id {
        let Value::Int(key) = self.get(slot) else {
            unreachable!("compiler constant slot")
        };
        let key = key.to_i64().expect("compiler constant slot fits in i64");
        if let Some(&value) = self.linked_string_constants.get(&key) {
            return value;
        }
        let value = build(self);
        self.linked_string_constants.insert(key, value);
        value
    }

    /// Packing an array skips tracking when its elements all have non-GC types.
    /// Even an untracked child tuple has a GC type. A folded literal belongs to
    /// the code constants and cannot itself be garbage owned by a cycle.
    pub(super) fn packed_tuple(&mut self, items: Vec<Id>, constant: bool) -> Id {
        if items.is_empty() {
            return EMPTY_TUPLE;
        }
        let untracked = constant
            || items.iter().all(|&child| {
                !matches!(self.get(child), Value::Tuple(_) | Value::Native(_))
                    && !self.slots[child as usize].gc_tracked(self)
            });
        let id = self.alloc_slot(Value::Tuple(items));
        self.slots[id as usize].untracked_tuple.set(untracked);
        id
    }

    /// Allocate a distinct string result even when its value is a cached Latin-1 character.
    pub(super) fn fresh_string(&mut self, value: Value) -> Result<Id> {
        debug_assert!(matches!(value, Value::Str(_) | Value::SurrogateStr(_)));
        if self.free_head == END {
            self.slots
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        self.frames
            .last_mut()
            .unwrap()
            .live
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        Ok(self.alloc_slot(value))
    }

    fn alloc_slot(&mut self, value: Value) -> Id {
        let index = if self.free_head != END {
            let index = self.free_head;
            self.free_head = self.slots[index].gc_next.replace(UNMARKED);
            self.free_count -= 1;
            self.slots[index].value = Some(value);
            index
        } else {
            if self.slots.len() == self.slots.capacity() && self.slots.try_reserve(1).is_err() {
                // Keep space for the next value when doubling the heap fails.
                // The next statement can collect with a complete set of live handles.
                self.slots.reserve_exact(1);
                self.next_collection = self.slots.len();
            }
            self.slots.push(Slot::new(value));
            self.slots.len() - 1
        };
        self.gc_order.push(index, &self.slots);
        self.slots[index].untracked_tuple.set(false);
        self.slots[index].finalization = Finalization::None;
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

    pub fn string_literal(&mut self, text: &str) -> Result<Id> {
        let intern = text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if intern && let Some(entry) = self.interned.get_mut(text) {
            entry.literal = true;
            let id = entry.id;
            self.keep(id);
            return Ok(id);
        }
        let mut value = String::new();
        value
            .try_reserve_exact(text.len())
            .map_err(|_| crate::Error::memory_error())?;
        value.push_str(text);
        if !intern {
            return Ok(self.alloc(Value::Str(value)));
        }
        let owner = crate::objects::try_literal_string(text)?;
        let owner = crate::sys::try_intern_owner(&owner).ok_or_else(crate::Error::memory_error)?;
        self.interned
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
        let id = self.alloc_slot(Value::Str(value));
        self.interned
            .insert(owner, InternedString { id, literal: true });
        Ok(id)
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
    /// Borrow a read source and a distinct writable target without copying either payload.
    pub(super) fn source_and_target(
        &mut self,
        source: Id,
        target: Id,
    ) -> Option<(&Value, &mut Value)> {
        let [source, target] = self
            .slots
            .get_disjoint_mut([source as usize, target as usize])
            .ok()?;
        Some((source.value.as_ref()?, target.value.as_mut()?))
    }

    pub fn get_mut(&mut self, id: Id) -> &mut Value {
        self.slots[id as usize]
            .value
            .as_mut()
            .expect("live native handle")
    }
    pub fn enter(&mut self, scope: Id, code: Id, owner: FrameOwner) {
        let mut activation = Frame::new(scope, code, owner, self.active_exception());
        if let FrameOwner::Generator(id) = owner {
            let Value::Generator { frame, .. } = self.get(id) else {
                unreachable!()
            };
            activation.object = *frame;
        }
        self.frames.push(activation);
    }
    /// Release the activation and report whether a returning function may have dropped a writer.
    pub fn leave(&mut self, value: Id, returning: bool) -> bool {
        let frame = self.frames.pop().expect("returning native activation");
        let opened = frame.created_finalizer && self.tracks_finalizers();
        if opened {
            self.frames.last_mut().unwrap().created_finalizer = true;
        }
        let finalizer_released = opened && returning && self.has_unbound_finalizer(value);
        if (self.has_weak_values || finalizer_released)
            && let Value::Scope(scope) = self.get(frame.scope)
        {
            let weak_released = self.has_weak_values
                && frame.live.iter().chain(scope.names.values()).any(|&id| {
                    id != value && self.weak_edge(id) && !self.bound_in_active_frame(id)
                });
            if weak_released || finalizer_released {
                // A normal function allocates its scope in the caller before entering.
                // Once it returns, only an escaped closure or frame can keep that scope alive.
                if scope.parent.is_some() && scope.class_body.is_none() {
                    self.release_scope_temporary(frame.scope);
                }
                self.collect_next_statement |= weak_released;
            }
        }
        self.keep(value);
        finalizer_released
    }

    /// Drop the caller's temporary reference to a completed function or generator scope.
    pub(super) fn release_scope_temporary(&mut self, scope: Id) {
        let caller = &mut self.frames.last_mut().unwrap().live;
        if let Some(index) = caller.iter().position(|&id| id == scope) {
            caller[index] = NONE;
        }
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

    /// A scalar and a container of scalars cannot hold an output file.
    pub(super) fn could_own_finalizer(&self, id: Id, skip_bound: bool) -> bool {
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

    /// Release an expression while preserving its result and earlier operands.
    pub(super) fn discard_operands(&mut self, mark: usize, result: Id) -> bool {
        if !self.tracks_finalizers() {
            return false;
        }
        let could_own_finalizer = self
            .frames
            .last()
            .unwrap()
            .live
            .iter()
            .skip(mark)
            .any(|&id| {
                // A one-argument call roots its operand through a private list.
                if let Value::List(items) = self.get(id)
                    && let [item] = items.as_slice()
                {
                    return self.could_own_finalizer(*item, false);
                }
                self.could_own_finalizer(id, false)
            });
        if !could_own_finalizer {
            return false;
        }
        self.release_temporaries_since(mark);
        if result != NONE {
            self.keep(result);
        }
        self.file_writers
            .iter()
            .chain(&self.ordinary_finalizers)
            .any(|&writer| {
                writer != result
                    && !self.active_binding_matches(|bound| {
                        let mut budget = 8;
                        self.keeps_finalizer(bound, writer, &mut budget)
                    })
            })
    }

    /// A consumed truth value cannot retain its receiver into the next operand.
    /// Earlier operands of enclosing expressions stay below this mark.
    pub(super) fn discard_truth_operands(&mut self, mark: usize) -> bool {
        let weak_release = self.has_weak_values
            && self
                .frames
                .last()
                .unwrap()
                .live
                .iter()
                .skip(mark)
                .any(|&id| self.weak_edge(id) && !self.bound_in_active_frame(id));
        let finalizer_release = self.discard_operands(mark, NONE);
        self.release_temporaries_since(mark);
        weak_release || finalizer_release
    }

    pub fn release_temporaries_since(&mut self, mark: usize) {
        self.frames.last_mut().unwrap().live.truncate(mark);
    }

    /// Drop an upgraded proxy's temporary root without discarding results returned by its hooks.
    pub(super) fn release_weak_target(&mut self, mark: usize, target: Id) {
        let root = &mut self.frames.last_mut().unwrap().live[mark];
        debug_assert_eq!(*root, target);
        *root = NONE;
        self.weak_edge_removed(target);
    }

    pub fn active_exception(&self) -> Id {
        self.frames.last().unwrap().active
    }

    /// Use the caller's exception when no lexical handler encloses this operation.
    pub(super) fn caught_exception(&self, caught: Id) -> Id {
        if caught == NONE {
            self.frames.last().unwrap().inherited
        } else {
            caught
        }
    }

    pub fn replace_active(&mut self, exception: Id) -> Id {
        std::mem::replace(&mut self.frames.last_mut().unwrap().active, exception)
    }

    /// Drop the frame's ownership of completed locals without changing captured closure cells.
    pub(super) fn clear_frame(&mut self, value: Id) -> Result<()> {
        let Value::PythonFrame { scope, .. } = *self.get(value) else {
            return Err(Error::new("TypeError", "expected a frame"));
        };
        if scope == NONE {
            return Ok(());
        }
        if self.frames.iter().any(|frame| frame.scope == scope) {
            return Err(Error::new(
                "RuntimeError",
                "cannot clear an executing frame",
            ));
        }
        if self.slots.iter().any(|slot| {
            matches!(&slot.value,
            Some(Value::Generator { scope: owner, offset: Some(_), .. }) if *owner == scope)
        }) {
            return Err(Error::new("RuntimeError", "cannot clear a suspended frame"));
        }
        let Value::PythonFrame { scope, .. } = self.get_mut(value) else {
            unreachable!()
        };
        *scope = NONE;
        self.collect_next_statement = true;
        Ok(())
    }

    pub fn caller_module(&self) -> Id {
        self.caller_module_at(0).expect("current frame")
    }

    pub fn caller_module_at(&self, depth: usize) -> Option<Id> {
        let mut owner = self.frames.iter().rev().nth(depth)?.owner;
        if let FrameOwner::Generator(id) = owner {
            let Value::Generator {
                owner: function, ..
            } = self.get(id)
            else {
                unreachable!()
            };
            owner = *function;
        }
        Some(match owner {
            FrameOwner::Generator(_) => unreachable!("a generator belongs to a function or module"),
            FrameOwner::Module(module) => module,
            FrameOwner::Function(function) => {
                let Value::Function { fields, .. } = self.get(function) else {
                    unreachable!("executing Python function")
                };
                fields.module
            }
        })
    }

    pub fn current_code(&self) -> Id {
        self.frames.last().unwrap().code
    }

    pub fn current_scope(&self) -> Id {
        self.frames.last().unwrap().scope
    }

    /// Python stack levels start at one and exclude the native root frame.
    pub fn warning_frame(&self, level: i32) -> Option<(Location, Id)> {
        self.frames
            .iter()
            .rev()
            .filter_map(|frame| {
                frame
                    .location
                    .clone()
                    .map(|location| (location, frame.scope))
            })
            .nth((level.max(1) - 1) as usize)
    }

    pub fn current_location(&self) -> Option<Location> {
        self.frames.last()?.location.clone()
    }

    /// Trace the same roots for collection and for a weak interning lookup between collections.
    fn mark(&self, globals: impl Iterator<Item = Id>) {
        let mut pending = MarkQueue {
            slots: &self.slots,
            next: END,
            ordered: None,
        };
        self.mark_roots(globals, &mut pending);
        while let Some(id) = pending.pop() {
            self.get(id).children(&mut pending);
        }
    }

    fn mark_roots(&self, globals: impl Iterator<Item = Id>, pending: &mut MarkQueue<'_>) {
        pending.extend(globals);
        pending.extend(self.full_collection_members());
        pending.extend(self.linked_string_constants.values().copied());
        pending.extend([NONE, FALSE, TRUE, ELLIPSIS, NOT_IMPLEMENTED, EMPTY_TUPLE]);
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
    }

    fn reachable(&self, id: Id, roots: impl Iterator<Item = Id>) -> bool {
        self.mark(roots);
        let reachable = self.slots[id as usize].marked();
        for slot in &self.slots {
            slot.trace.set(UNMARKED);
        }
        reachable
    }

    pub(super) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn ordinary_instance(&self, id: Id) -> bool {
        matches!(self.slots[id as usize].value, Some(Value::Instance { .. }))
    }

    pub(super) fn register_finalizer(&mut self, id: Id) {
        if self.slots[id as usize].finalization == Finalization::None {
            self.slots[id as usize].finalization = Finalization::Pending;
            self.ordinary_finalizers.push(id);
            self.frames.last_mut().unwrap().created_finalizer = true;
        }
    }

    /// Running generators have a live frame. Rearm cleanup only if they suspend again.
    pub(super) fn disarm_generator_finalizer(&mut self, id: Id) {
        if self.slots[id as usize].finalization == Finalization::Pending {
            self.take_finalizer(id, false);
        }
    }

    pub(super) fn has_ordinary_finalizers(&self) -> bool {
        !self.ordinary_finalizers.is_empty()
    }

    pub(super) fn pending_full_finalizer(&self) -> Option<Id> {
        if !self.has_ordinary_finalizers() {
            return None;
        }
        let cohort = self.full_collection.as_ref()?;
        let mut index = self.gc_order.head;
        while index != END {
            let slot = &self.slots[index];
            if slot.finalization == Finalization::Pending
                && cohort.incoming.get(index).is_some_and(|&count| count != 0)
            {
                return Some(index as Id);
            }
            index = slot.gc_next.get();
        }
        None
    }

    pub(super) fn take_finalizer(&mut self, id: Id, found: bool) {
        let index = self
            .ordinary_finalizers
            .iter()
            .rposition(|&owner| owner == id)
            .unwrap();
        self.ordinary_finalizers.swap_remove(index);
        self.slots[id as usize].finalization = if found {
            Finalization::Done
        } else {
            Finalization::None
        };
    }

    pub(super) fn acyclic_finalizer(
        &self,
        roots: impl Iterator<Item = Id>,
        stdin: Option<&[Id; 3]>,
    ) -> Result<Option<Id>> {
        // Input's wrapper and raw file usually survive every allocation checkpoint.
        if let Some(chain) = stdin
            && self
                .ordinary_finalizers
                .iter()
                .all(|owner| chain.contains(owner))
        {
            return Ok(None);
        }
        // A suspended caller keeps earlier operands live across a nested call.
        // Scan the callers once when only one finalizer needs a reachability check.
        if let [owner] = self.ordinary_finalizers.as_slice()
            && self
                .frames
                .iter()
                .rev()
                .skip(1)
                .any(|frame| frame.live.contains(owner))
        {
            return Ok(None);
        }
        // If the roots keep every pending finalizer alive, no finalizer can run.
        // Keep the visited links until the search ends so resetting a partial
        // trace costs only the slots it touched, even after a large heap peak.
        let mut remaining = self.ordinary_finalizers.len();
        let mut visited = END;
        let mut pending = MarkQueue {
            slots: &self.slots,
            next: END,
            ordered: None,
        };
        self.mark_roots(roots, &mut pending);
        while remaining != 0
            && let Some(id) = pending.pop()
        {
            let slot = &self.slots[id as usize];
            slot.trace.set(visited);
            visited = id as usize;
            if slot.finalization == Finalization::Pending {
                remaining -= 1;
            }
            if remaining != 0 {
                self.get(id).children(&mut pending);
            }
        }
        let next = if remaining == 0 {
            Ok(None)
        } else {
            CycleSet::new(self.slots.len()).map(|mut cycles| self.identify_cycles(&mut cycles))
        };
        while let Some(id) = pending.pop() {
            self.slots[id as usize].trace.set(UNMARKED);
        }
        while visited != END {
            visited = self.slots[visited].trace.replace(UNMARKED);
        }
        next
    }

    /// Track a disk writer in the activation whose locals or return value may own it.
    pub(super) fn alloc_file_writer(&mut self, file: Value) -> Result<Id> {
        self.file_writers
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        let id = self.alloc(file);
        self.file_writers.push(id);
        self.frames.last_mut().unwrap().created_finalizer = true;
        Ok(id)
    }

    /// Recheck the strong path from a caller-held method to the sole pending finalizer.
    pub(super) fn caller_keeps_only_finalizer(&mut self) -> bool {
        let [owner] = self.ordinary_finalizers.as_slice() else {
            return false;
        };
        let receiver_fields = |operand: &Id| {
            let Value::Method { receiver, .. } = self.get(*operand) else {
                return None;
            };
            let Value::Instance { fields, .. } = self.get(*receiver) else {
                return None;
            };
            Some(fields)
        };
        // Frames, operand slots, and fields can all be removed or reused.
        if let Some((frame, operand, field)) = self.caller_finalizer_field
            && self
                .frames
                .get(frame)
                .and_then(|frame| frame.live.get(operand))
                .and_then(receiver_fields)
                .and_then(|fields| fields.get_index(field))
                .is_some_and(|(_, value)| value == owner)
        {
            return true;
        }

        // Earlier operands hold outer expressions such as pending context manager exits.
        // Learn the position while its frame is the immediate caller, before recursion.
        if let Some((frame_index, caller)) = self.frames.iter().enumerate().rev().nth(1) {
            for (operand_index, operand) in caller.live.iter().take(8).enumerate() {
                if let Some(fields) = receiver_fields(operand)
                    && let Some(field) = fields.values().take(8).position(|value| value == owner)
                {
                    self.caller_finalizer_field = Some((frame_index, operand_index, field));
                    return true;
                }
            }
        }
        self.caller_finalizer_field = None;
        false
    }

    /// A returned writer or a surviving name keeps a direct owner.
    pub(super) fn has_unbound_finalizer(&self, retained: Id) -> bool {
        self.file_writers
            .iter()
            .chain(&self.ordinary_finalizers)
            .any(|&id| id != retained && !self.bound_in_active_frame(id))
    }

    /// Return whether a buffered file could need collecting at a target boundary.
    pub(super) fn tracks_finalizers(&self) -> bool {
        !self.file_writers.is_empty() || self.has_ordinary_finalizers()
    }

    /// Recheck unbound writers after control flow discards temporary roots.
    pub fn release_unbound_finalizers(&mut self) {
        self.collect_next_statement |= self.has_unbound_finalizer(NONE);
    }

    /// Keep the operands needed by later targets after a source assignment releases its old value.
    pub(super) fn assignment_target(&mut self, live: &[Id], replaced: bool) -> bool {
        // The private unpack list still owns all items waiting for a target.
        if !replaced || !self.has_unbound_finalizer(NONE) {
            return false;
        }
        let temporaries = &mut self.frames.last_mut().unwrap().live;
        temporaries.clear();
        temporaries.extend_from_slice(live);
        true
    }

    /// A named expression retains earlier operands and its result after replacing the binding.
    pub(super) fn named_assignment(&mut self, mark: usize, value: Id) -> bool {
        if !self.has_unbound_finalizer(value) {
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
    ) -> Option<WeakCallbacks> {
        self.publish_statement(live, location, caught);
        self.collect_if_due(globals, self.automatic_gc.load(Ordering::Relaxed))
    }

    pub(super) fn publish_statement(&mut self, live: &[Id], location: Location, caught: Id) {
        let active = self.caught_exception(caught);
        let frame = self.frames.last_mut().unwrap();
        frame.active = active;
        let line = location.line;
        frame.location = Some(location);
        frame.live.clear();
        frame.live.extend(live);
        self.set_statement_line(line);
    }

    /// A match statement can execute separate pattern and guard expressions in one step.
    pub(super) fn set_statement_line(&mut self, line: u32) {
        let frame = self.frames.last_mut().unwrap();
        frame
            .location
            .as_mut()
            .expect("active source location")
            .line = line;
        if let Some(id) = frame.object
            && let Some(Value::PythonFrame { lineno, .. }) = &mut self.slots[id as usize].value
        {
            *lineno = line;
        }
    }

    /// A discarded owner or the allocation threshold has requested a sweep.
    pub(super) fn collection_requested(&self) -> bool {
        self.collect_next_statement || self.cycles_due()
    }

    /// Collect when a discarded owner or the heap threshold requests a sweep.
    pub(super) fn collect_if_due(
        &mut self,
        globals: impl Iterator<Item = Id>,
        automatic_gc: bool,
    ) -> Option<WeakCallbacks> {
        let cycles_due = self.cycles_due();
        if !std::mem::take(&mut self.collect_next_statement) && !cycles_due {
            return None;
        }
        if cycles_due && automatic_gc {
            self.mark(globals);
            self.sweep()
        } else {
            self.collect(globals)
        }
    }

    /// Collect early when a streaming consumer cannot reserve its next heap slot.
    pub(super) fn reserve_slot_or_request_collection(&mut self) {
        if self.slots.len() < self.next_collection
            && self.free_head == END
            && self.slots.try_reserve(1).is_err()
        {
            self.next_collection = self.slots.len();
        }
    }

    /// Size the next streaming collection in proportion to the surviving heap.
    pub(super) fn streaming_collection_limit(&self) -> usize {
        (self.slots.len() - self.free_count).saturating_mul(2)
    }

    /// Request collection before a streaming callback exhausts the heap.
    pub(super) fn prepare_streaming_collection(&mut self, limit: usize) -> bool {
        if self.free_head == END
            && (self.slots.len() >= limit || self.slots.try_reserve(1).is_err())
        {
            self.next_collection = self.slots.len();
            return true;
        }
        false
    }

    pub(super) fn cycles_due(&self) -> bool {
        self.slots.len() >= self.next_collection
            && self.free_head == END
            && !self.full_collection_active()
    }

    /// A prompt collection models a reference decrement. Preserve cycles until
    /// an automatic or explicit full collection, including cycles created in callbacks.
    pub(super) fn collect(&mut self, globals: impl Iterator<Item = Id>) -> Option<WeakCallbacks> {
        let Ok(mut cycles) = CycleSet::new(self.slots.len()) else {
            self.collect_next_statement = true;
            return None;
        };
        // A weakref callback can drop an unrelated owner. Keep its graph intact
        // for Python dispatch when this sweep releases temporary callback roots.
        self.mark(globals.chain(self.ordinary_finalizers.iter().copied()));
        self.identify_cycles(&mut cycles);
        // Peeling leaves both cycles and their unreachable descendants. Every
        // strong child of a member is therefore already live or itself a member.
        for id in cycles.members() {
            self.slots[id as usize].trace.set(END);
        }
        self.sweep()
    }

    pub(super) fn full_collection_active(&self) -> bool {
        self.full_collection.is_some()
    }

    pub(super) fn full_collection_members(&self) -> impl Iterator<Item = Id> + '_ {
        self.full_collection.iter().flat_map(CycleSet::members)
    }

    /// Visit each distinct strong child outside the live graph. The queue skips live
    /// slots automatically; restore its scratch marks before visiting another owner.
    fn unreachable_children(&self, value: &Value, mut visit: impl FnMut(usize)) {
        let mut children = MarkQueue {
            slots: &self.slots,
            next: END,
            ordered: None,
        };
        value.children(&mut children);
        while let Some(id) = children.pop() {
            self.slots[id as usize].trace.set(UNMARKED);
            visit(id as usize);
        }
    }

    /// CPython's full collection visits survivors in their existing GC order.
    /// A child found after its position was passed moves to the end. In
    /// particular, a tuple reachable only through its parent can remain
    /// tracked for one more collection than the same tuple held in a local.
    fn mark_full(&mut self, globals: impl Iterator<Item = Id>, passed: &mut Vec<usize>) {
        let mut pending = MarkQueue {
            slots: &self.slots,
            next: END,
            ordered: None,
        };
        self.mark_roots(globals, &mut pending);
        while pending.pop().is_some() {}
        let mut scan = std::mem::replace(&mut self.gc_order, GcOrder::EMPTY);
        while let Some(id) = scan.pop(&self.slots) {
            let slot = &self.slots[id];
            if !slot.marked() {
                passed.push(id);
                continue;
            }
            // Restore previously passed children in tp_traverse order. A LIFO
            // mark queue would reverse dictionary values and reorder aliases.
            self.get(id as Id).children(&mut MarkQueue {
                slots: &self.slots,
                next: END,
                ordered: Some(&mut scan),
            });
            if !slot.untracked_tuple.get()
                && let Value::Tuple(items) = self.get(id as Id)
            {
                slot.untracked_tuple
                    .set(items.iter().all(|&id| match self.get(id) {
                        // A cached builtin function still keeps its tuple tracked.
                        Value::Native(name) => super::builtins::is_static_gc_type(name),
                        _ => !self.slots[id as usize].gc_tracked(self),
                    }));
            }
            self.gc_order.push(id, &self.slots);
        }
        // A finalizer can resurrect these objects. Keep their encounter order until
        // the final sweep decides which slots to retire.
        for &id in passed.iter() {
            if !self.slots[id].marked() {
                self.gc_order.push(id, &self.slots);
            }
        }
    }

    /// A tracing sweep also releases ordinary expression temporaries. Exclude the
    /// dead objects that refcounting alone would release from the Python GC count.
    fn identify_cycles(&self, cycles: &mut CycleSet) -> Option<Id> {
        let mut first_finalizer = None;
        let CycleSet {
            incoming,
            work: unowned,
            ..
        } = cycles;
        unowned.clear();
        incoming.fill(0);
        for (id, slot) in self.slots.iter().enumerate() {
            if !slot.marked()
                && let Some(value) = &slot.value
            {
                // Count the candidate itself so that zero denotes a vacant or reachable slot.
                incoming[id] += 1;
                self.unreachable_children(value, |child| incoming[child] += 1);
            }
        }
        for (id, count) in incoming.iter_mut().enumerate() {
            if *count != 0 {
                *count -= 1;
                if *count == 0 {
                    unowned.push(id);
                }
            }
        }
        while let Some(id) = unowned.pop() {
            if first_finalizer.is_none() && self.slots[id].finalization == Finalization::Pending {
                first_finalizer = Some(id as Id);
            }
            self.unreachable_children(self.slots[id].value.as_ref().unwrap(), |child| {
                incoming[child] -= 1;
                if incoming[child] == 0 {
                    unowned.push(child);
                }
            });
        }
        first_finalizer
    }

    /// Clear weakrefs to the initial cyclic isolate before running Python callbacks.
    pub(super) fn begin_full_collection(
        &mut self,
        globals: impl Iterator<Item = Id>,
    ) -> Result<WeakCallbacks> {
        let mut cycles = CycleSet::new(self.slots.len())?;
        self.mark_full(globals, &mut cycles.work);
        self.identify_cycles(&mut cycles);
        let mut callbacks = self.clear_weak_targets(Some(&cycles.incoming));
        cycles.cyclic_callbacks = callbacks.len();
        self.mark(cycles.members());
        if let Some(acyclic) = self.sweep() {
            callbacks.extend(acyclic);
        }
        self.full_collection = Some(cycles);
        Ok(callbacks)
    }

    fn clear_weak_targets(&mut self, cyclic: Option<&[usize]>) -> Vec<(Id, Id)> {
        // Clear non-owning handles before any dead target slot can be reused. Dead wrappers
        // never receive callbacks, and every live wrapper is cleared before any callback runs.
        let mut callbacks = Vec::new();
        let live = &mut self.frames.last_mut().unwrap().live;
        self.weak.retain(|&target, refs| {
            if let Some(cyclic) = cyclic {
                if cyclic[target as usize] == 0 {
                    // A weakref in the isolate must lose even a live referent. A
                    // later finalizer can resurrect the wrapper and observe it.
                    refs.retain(|&id| {
                        if cyclic[id as usize] == 0 {
                            return true;
                        }
                        let weak = self.slots[id as usize]
                            .value
                            .as_mut()
                            .unwrap()
                            .weak_reference_mut()
                            .unwrap();
                        weak.target = None;
                        false
                    });
                    return !refs.is_empty();
                }
            } else {
                refs.retain(|&id| self.slots[id as usize].marked());
                if self.slots[target as usize].marked() {
                    return !refs.is_empty();
                }
            }
            let count = refs
                .iter()
                .filter(|&&id| {
                    self.slots[id as usize]
                        .value
                        .as_ref()
                        .unwrap()
                        .weak_reference()
                        .unwrap()
                        .callback
                        != NONE
                })
                .count();
            let dispatch = callbacks.try_reserve(count).is_ok()
                && live.try_reserve(count.saturating_mul(2)).is_ok();
            if !dispatch {
                eprintln!("Exception ignored while clearing object weakrefs: MemoryError");
            }
            for &id in refs.iter() {
                let reachable = self.slots[id as usize].marked();
                let wr = self.slots[id as usize]
                    .value
                    .as_mut()
                    .unwrap()
                    .weak_reference_mut()
                    .unwrap();
                wr.target = None;
                // Cyclic GC severs only the weak target. The callback remains a
                // strong edge until the wrapper dies and is observable via __callback__.
                let callback = if cyclic.is_some() {
                    wr.callback
                } else {
                    std::mem::replace(&mut wr.callback, NONE)
                };
                if dispatch && callback != NONE && reachable {
                    callbacks.push((callback, id));
                    live.extend([callback, id]);
                }
            }
            false
        });
        callbacks
    }

    /// The GC temporarily owns each weakref with a callback. CPython also counts
    /// the wrapper if releasing that temporary reference destroys it.
    pub(super) fn release_callback_refs(
        &mut self,
        globals: impl Iterator<Item = Id>,
        callbacks: &WeakCallbacks,
    ) -> (usize, Option<WeakCallbacks>) {
        let n = self
            .full_collection
            .as_ref()
            .expect("active collection")
            .cyclic_callbacks;
        let deferred = self.collect(globals);
        let released = callbacks[..n]
            .iter()
            .filter(|(_, id)| self.slots[*id as usize].value.is_none())
            .count();
        (released, deferred)
    }

    /// Recompute reachability after resurrection and count only the original isolate.
    pub(super) fn finish_full_collection(
        &mut self,
        globals: impl Iterator<Item = Id>,
    ) -> (usize, Option<WeakCallbacks>) {
        let mut cycles = self
            .full_collection
            .take()
            .expect("finishing active collection");
        // A callback can release other roots or allocate a new cycle. Those objects are
        // outside this collection, but their strong edges can keep original candidates alive.
        let outside = self.slots.iter().enumerate().filter_map(|(id, slot)| {
            (slot.value.is_some() && cycles.incoming.get(id).copied().unwrap_or(0) == 0)
                .then_some(id as Id)
        });
        self.mark(globals.chain(outside));
        self.identify_cycles(&mut cycles);
        let count = cycles
            .members()
            .filter(|&id| self.slots[id as usize].gc_tracked(self))
            .count();
        let callbacks = self.sweep();
        // Slots can now be reused. Keep the collection guard until any callbacks
        // from the final sweep return, without retaining stale candidate IDs.
        cycles.incoming.clear();
        self.full_collection = Some(cycles);
        (count, callbacks)
    }

    pub(super) fn end_full_collection(&mut self) {
        self.full_collection = None;
    }

    fn sweep(&mut self) -> Option<WeakCallbacks> {
        let mut scan = std::mem::replace(&mut self.gc_order, GcOrder::EMPTY);
        while let Some(id) = scan.pop(&self.slots) {
            if self.slots[id].marked() {
                self.gc_order.push(id, &self.slots);
            }
        }
        self.ordinary_finalizers
            .retain(|&id| self.slots[id as usize].marked());
        self.file_writers.retain(|&id| {
            let slot = &self.slots[id as usize];
            slot.marked()
                && match &slot.value {
                    Some(Value::NamedFileCloser(closer)) => closer.borrow().needs_cleanup(),
                    Some(Value::TextFile(file)) => !file.closed(),
                    Some(Value::File(file)) => !file.closed(),
                    _ => false,
                }
        });
        self.interned
            .retain(|_, entry| self.slots[entry.id as usize].marked());
        let callbacks = self.clear_weak_targets(None);
        self.mock_call_lists
            .retain(|&list| self.slots[list as usize].marked());
        self.abc_registrations.retain(|&[owner, member]| {
            self.slots[owner as usize].marked() && self.slots[member as usize].marked()
        });
        self.protocol_subclass_cache.retain(|&[owner, member], _| {
            self.slots[owner as usize].marked() && self.slots[member as usize].marked()
        });
        // Vacant slots supply their own free-list links. A sweep can therefore
        // retire the entire unreachable graph even if the allocator refuses growth.
        for (id, slot) in self.slots.iter_mut().enumerate() {
            if slot.trace.replace(UNMARKED) == UNMARKED && slot.value.is_some() {
                slot.value = None;
                self.immutable_hashes.remove(&(id as Id));
                debug_assert_eq!(slot.gc_next.get(), UNMARKED);
                slot.gc_next.set(self.free_head);
                self.free_head = id;
                self.free_count += 1;
            }
        }
        // A refused growth request can lower the usual collection floor.
        let floor = self.next_collection.min(4096);
        self.next_collection = (self.slots.len() - self.free_count)
            .saturating_mul(2)
            .max(floor);
        (!callbacks.is_empty()).then_some(callbacks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuse_vacant_slots_without_disturbing_survivors() {
        let mut heap = Heap::new();
        let keep = heap.alloc(Value::List(Vec::new()));
        let mark = heap.temporary_mark();
        let child = heap.alloc(Value::List(vec![keep]));
        let parent = heap.alloc(Value::List(vec![child]));
        heap.release_temporaries_since(mark);
        heap.collect(std::iter::empty());
        // A second sweep must leave the free chain intact.
        heap.collect(std::iter::empty());
        assert_eq!(heap.alloc(Value::Tuple(vec![keep])), parent);
        assert_eq!(heap.alloc(Value::List(vec![parent])), child);
        heap.release_temporaries_since(mark);
        heap.collect([child].into_iter());
        assert!(matches!(heap.get(parent), Value::Tuple(items) if items == &[keep]));
        assert_eq!(heap.free_count, 0);
    }

    #[test]
    fn prompt_sweep_preserves_cycle_descendants_and_pending_owners() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let leaf = heap.alloc(Value::List(Vec::new()));
        let cycle = heap.alloc(Value::List(vec![leaf, leaf]));
        let Value::List(items) = heap.get_mut(cycle) else {
            unreachable!()
        };
        items.push(cycle);
        let child = heap.alloc(Value::List(Vec::new()));
        let owner = heap.alloc(Value::List(vec![child]));
        heap.register_finalizer(owner);
        let discarded = heap.alloc(Value::List(Vec::new()));
        heap.release_temporaries_since(mark);

        heap.collect(std::iter::empty());
        for id in [leaf, cycle, child, owner] {
            assert!(heap.slots[id as usize].value.is_some());
        }
        assert!(heap.slots[discarded as usize].value.is_none());

        heap.take_finalizer(owner, true);
        heap.collect(std::iter::empty());
        assert!(heap.slots[owner as usize].value.is_none());
        assert!(heap.slots[child as usize].value.is_none());
    }

    #[test]
    fn collected_mock_call_list_does_not_mark_a_reused_list_slot() {
        let mut heap = Heap::new();
        let list = heap.alloc(Value::List(Vec::new()));
        heap.mock_call_lists.insert(list);
        heap.frames.last_mut().unwrap().live.clear();

        heap.collect([list].into_iter());
        assert!(heap.mock_call_lists.contains(&list));
        heap.collect(std::iter::empty());
        let ordinary = heap.alloc(Value::List(Vec::new()));
        assert_eq!(ordinary, list);
        assert!(!heap.mock_call_lists.contains(&ordinary));
    }

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
    fn full_collection_rechecks_resurrected_roots_and_excludes_new_cycles() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let survivor = heap.alloc(Value::List(Vec::new()));
        let child = heap.alloc(Value::List(vec![survivor]));
        let doomed = heap.alloc(Value::List(Vec::new()));
        let Value::List(items) = heap.get_mut(survivor) else {
            unreachable!()
        };
        items.push(child);
        let Value::List(items) = heap.get_mut(doomed) else {
            unreachable!()
        };
        items.push(doomed);
        heap.release_temporaries_since(mark);

        assert!(
            heap.begin_full_collection(std::iter::empty())
                .unwrap()
                .is_empty()
        );
        assert_eq!(heap.full_collection_members().count(), 3);
        let new_cycle = heap.alloc(Value::List(Vec::new()));
        let Value::List(items) = heap.get_mut(new_cycle) else {
            unreachable!()
        };
        items.push(new_cycle);
        heap.release_temporaries_since(mark);
        heap.collect(std::iter::empty());

        // The callback publishes one original cycle and creates a different one.
        assert_eq!(heap.finish_full_collection([survivor].into_iter()).0, 1);
        heap.end_full_collection();
        assert!(heap.slots[doomed as usize].value.is_none());
        for owner in [survivor, child, new_cycle] {
            assert!(heap.slots[owner as usize].value.is_some());
        }
        heap.begin_full_collection(std::iter::empty()).unwrap();
        assert_eq!(heap.finish_full_collection(std::iter::empty()).0, 3);
    }

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
