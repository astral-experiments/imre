//! Values shared by native functions loading Python modules through `sys.path`.

mod weak_roots;

use super::gc_schedule::{GcSchedule, GcSettings};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Weak};

use super::Context;

pub(super) type Id = u64;
pub(super) type Code = unsafe extern "C" fn(*mut Context, Id) -> Id;

/// Immutable executable and argument layout shared by functions from one Python definition.
#[derive(Clone)]
pub(super) struct CodeObject {
    pub body: Body,
    pub parameters: Id,
    /// Decoded argument metadata is shared by calls using this immutable code object.
    pub parsed_parameters: std::cell::OnceCell<std::rc::Rc<serde_json::Value>>,
    pub name: Id,
    pub qualname: Id,
    pub doc: Id,
    pub filename: Id,
    pub first_line: u32,
    pub module_definition: bool,
    pub coroutine: bool,
    /// CPython flags for boxed Python function definitions.
    pub flags: u32,
    /// Runtime source retains execution flags even when its complete co_flags is unknown.
    pub flags_complete: bool,
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
pub(super) const EMPTY_TUPLE: Id = 5;
const SMALL_INT_MIN: i64 = -5;
const SMALL_INT_MAX: i64 = 256;
const FIRST_SMALL_INT: Id = EMPTY_TUPLE + 1;
const FIRST_DYNAMIC_SLOT: usize =
    FIRST_SMALL_INT as usize + (SMALL_INT_MAX - SMALL_INT_MIN + 1) as usize;

/// Two frame-bound text streams can each retain a wrapper, buffer, and raw file.
pub(super) const MAX_FRAME_FINALIZERS: usize = 2 * 3;

pub(super) const LZMA_COMPRESS_TYPE: &str = "_lzma.LZMACompressor";
pub(super) const LZMA_DECOMPRESS_TYPE: &str = "_lzma.LZMADecompressor";
pub(super) const LZMA_COMPRESS_METHODS: &[&str] = &["compress", "flush"];
pub(super) const LZMA_DECOMPRESS_METHODS: &[&str] = &["decompress"];
pub(super) const LZMA_DECOMPRESS_FIELDS: &[&str] = &["check", "eof", "needs_input", "unused_data"];

pub(super) const ZLIB_DECOMPRESS_TYPE: &str = "zlib.Decompress";
pub(super) const ZLIB_DECOMPRESS_COPY_METHODS: &[&str] = &["copy", "__copy__", "__deepcopy__"];
pub(super) const SSL_ERROR_TYPE: &str = "ssl.SSLError";

pub(super) const STRUCT_UNPACK_ITERATOR: &str = "_struct.unpack_iterator";
pub(super) const REGEX_PATTERN_TYPE: &str = "re.Pattern";
pub(super) const REGEX_MATCH_TYPE: &str = "re.Match";
pub(super) const REGEX_SCANNER_TYPE: &str = "_sre.SRE_Scanner";
pub(super) const REGEX_COPY_METHODS: &[&str] = &["__copy__", "__deepcopy__"];
pub(super) const ZLIB_COMPRESS_TYPE: &str = "zlib.Compress";
pub(super) const ZLIB_COMPRESS_METHODS: &[&str] =
    &["compress", "flush", "copy", "__copy__", "__deepcopy__"];
pub(super) const TEMPLATE_TYPE: &str = "string.templatelib.Template";
pub(super) const TEMPLATE_ITER_TYPE: &str = "string.templatelib.TemplateIter";
pub(super) const INTERPOLATION_TYPE: &str = "string.templatelib.Interpolation";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Location {
    pub file: std::borrow::Cow<'static, str>,
    pub line: u32,
    pub span: Option<[u32; 3]>,
}

/// Immutable construction-time fields independent of BaseException.args.
#[derive(Clone, Copy, Debug)]
pub(super) struct GroupData {
    pub(super) message: Id,
    pub(super) exceptions: Id,
    pub(super) sequence_repr: Option<Id>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Trace {
    // Pending activations precede the already exposed, mutable Python traceback.
    pending_frames: Vec<(Location, Id)>,
    pub instance: Option<Id>,
    pub cause: Option<Id>,
    pub import_message: Option<Id>,
    /// Live members belong to the exception instance across traceback clones.
    pub source_syntax: Option<crate::shared::Shared<[std::cell::Cell<Id>; 9]>>,
    pub import_names: Option<[Id; 3]>,
    pub group: Option<GroupData>,
    pub name_error_name: Option<Id>,
    pub dictionary: Option<Id>,
    stop_value: Id,
    head: Id,
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
    /// Python object held by the writable SystemExit.code member.
    pub exit_code: Id,
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
            exit_code: NONE,
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

    /// Read an exception group's retained message and immutable children.
    pub(super) fn group(&self) -> Option<[Id; 2]> {
        let group = self.trace.as_ref()?.group?;
        Some([group.message, group.exceptions])
    }

    /// Reuse the custom input sequence's representation captured before iteration.
    pub(super) fn group_sequence_repr(&self) -> Option<Id> {
        self.trace.as_ref()?.group?.sequence_repr
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

    /// SystemExit.__init__ keeps the previous code when called without arguments.
    pub(super) fn initialize_exit_code(&mut self, args: &[Id]) {
        match args {
            [] => {}
            [value] => self.exit_code = *value,
            _ => self.exit_code = self.args.expect("initialized exception argument tuple"),
        }
    }

    pub(super) fn roots(&self) -> impl Iterator<Item = Id> + '_ {
        self.args
            .into_iter()
            .chain(self.class())
            .chain((self.exit_code != NONE).then_some(self.exit_code))
            .chain((self.context != NONE).then_some(self.context))
            .chain(
                self.decode
                    .iter()
                    .flat_map(|d| [d.encoding, d.object, d.reason]),
            )
            .chain(self.os_args.iter().flat_map(|args| args.roots()))
            .chain(self.trace.iter().flat_map(|trace| {
                trace
                    .pending_frames
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
                    .chain(trace.group.iter().flat_map(|group| {
                        [group.message, group.exceptions]
                            .into_iter()
                            .chain(group.sequence_repr)
                    }))
                    .chain(trace.name_error_name)
                    .chain(trace.dictionary)
                    .chain((trace.head != NONE).then_some(trace.head))
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
            trace.pending_frames.clear();
            trace.head = NONE;
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

    /// Snapshot active Python code without materializing Python frame objects.
    pub(super) fn publish_fault_frames(&self) {
        self.heap
            .publish_fault_frames(crate::fault_handler::generation());
    }

    /// Capture code metadata at creation without extending the life of any caller frame.
    pub(super) fn coroutine_origin(&mut self) -> Id {
        let depth = self.threads.coroutine_origin_depth as usize;
        if depth == 0 {
            return NONE;
        }
        let mut frames = Vec::new();
        for index in (0..self.heap.frames.len()).rev() {
            let frame = &self.heap.frames[index];
            let Some(location) = &frame.location else {
                continue;
            };
            if frame.code == NONE {
                continue;
            }
            let code = self.code_object(frame.code);
            let line = frame.line(&self.heap, location);
            let (file, name) = (code.filename, code.name);
            let line = self.int(i64::from(line));
            frames.push(self.alloc(Value::Tuple(vec![file, line, name])));
            if frames.len() == depth {
                break;
            }
        }
        self.alloc(Value::Tuple(frames))
    }

    /// Resolve the executing operation only when a Python caller inspects this frame.
    pub(super) fn frame_position(&self, object: Id) -> Option<[u32; 4]> {
        let frame = self
            .heap
            .frames
            .iter()
            .rev()
            .find(|frame| frame.object == Some(object))?;
        if let Some(position) = frame.call_position {
            return Some(position);
        }
        match (frame.expression, &self.code_object(frame.code).body) {
            (ExpressionPosition::Source { range, .. }, Body::Source(program)) => {
                program.expression_range(range)
            }
            _ => None,
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
            let unoptimized_locals = super::frame_locals::unoptimized_namespace(self, scope, code)?;
            let class_proxy_scope = super::frame_locals::class_cells(self, scope, code);
            let object = self.alloc(Value::PythonFrame {
                unoptimized_locals,
                class_proxy_scope,
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

    /// Reuse the activation's Python frame even when first observed between resumptions.
    pub(super) fn generator_frame(&mut self, id: Id) -> Result<Id> {
        let Value::Generator {
            frame,
            scope,
            code,
            lineno,
            ..
        } = *self.heap.get(id)
        else {
            unreachable!("generator frame owner")
        };
        if let Some(frame) = frame {
            return Ok(frame);
        }
        if scope == NONE {
            return Ok(NONE);
        }
        if let Some(index) =
            self.heap.frames.iter().rposition(
                |frame| matches!(frame.owner, FrameOwner::Generator(owner) if owner == id),
            )
            && let Some((_, frame)) = self.materialize_frame(index)?
        {
            return Ok(frame);
        }
        let globals = self.module_globals(scope)?;
        let object = self.alloc(Value::PythonFrame {
            unoptimized_locals: None,
            class_proxy_scope: None,
            scope,
            code,
            globals,
            back: None,
            lineno,
        });
        let Value::Generator { frame, .. } = self.heap.get_mut(id) else {
            unreachable!()
        };
        *frame = Some(object);
        Ok(object)
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
                        trace.head == NONE
                            && trace.pending_frames.is_empty()
                            && trace.instance != Some(active)
                    })
                {
                    error.context = active;
                }
                if let Some(trace) = error.try_tracing()
                    && trace.pending_frames.try_reserve(1).is_ok()
                {
                    trace.pending_frames.push((loc, frame));
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
        let Value::Raised(error) = self.heap.get_mut(exception) else {
            return Err(Error::new("TypeError", "expected exception"));
        };
        let Some(trace) = error.trace.as_mut() else {
            return Ok(NONE);
        };
        let mut head = trace.head;
        if !trace.pending_frames.is_empty() {
            for (at, frame) in std::mem::take(&mut trace.pending_frames) {
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
            error.tracing().head = head;
        }
        Ok(head)
    }

    /// Store an explicit cause, including None, using the same state as raise-from.
    pub(super) fn set_exception_cause(&mut self, exception: Id, cause: Id) -> Result<()> {
        if cause != NONE && !matches!(self.heap.get(cause), Value::Raised(_)) {
            return Err(Error::new(
                "TypeError",
                "exception cause must be None or derive from BaseException",
            ));
        }
        let Value::Raised(error) = self.heap.get_mut(exception) else {
            unreachable!("cause assignment requires an exception instance")
        };
        if let Some(old) = error.tracing().cause.replace(cause)
            && old != NONE
            && old != cause
        {
            self.heap.collect_next_statement = true;
        }
        Ok(())
    }

    /// Adopt the exact Python traceback chain and replace any previously retained frames.
    pub(super) fn set_traceback(&mut self, exception: Id, tb: Id) -> Result<()> {
        if !matches!(self.heap.get(exception), Value::Raised(_)) {
            return Err(Error::new("TypeError", "expected a BaseException instance"));
        }
        if tb != NONE && !matches!(self.heap.get(tb), Value::Traceback { .. }) {
            return Err(Error::new(
                "TypeError",
                "__traceback__ must be a traceback or None",
            ));
        }
        let Value::Raised(error) = self.heap.get_mut(exception) else {
            unreachable!()
        };
        error.clear_traceback();
        if tb != NONE {
            error.tracing().head = tb;
        }
        self.heap.collect_next_statement = true;
        Ok(())
    }

    /// Link traceback nodes without introducing a cycle in their shared tail.
    pub(super) fn set_traceback_next(&mut self, tb: Id, next: Id) -> Result<()> {
        let mut node = next;
        while node != NONE {
            if node == tb {
                return Err(Error::new("ValueError", "traceback loop detected"));
            }
            let Value::Traceback { next, .. } = self.heap.get(node) else {
                return Err(Error::new(
                    "TypeError",
                    format!("expected traceback object, got '{}'", self.type_name(node)),
                ));
            };
            node = *next;
        }
        let Value::Traceback { next: link, .. } = self.heap.get_mut(tb) else {
            unreachable!("traceback instance")
        };
        let previous = std::mem::replace(link, next);
        if previous != NONE && previous != next {
            self.heap.collect_next_statement = true;
        }
        Ok(())
    }

    pub(super) fn type_name(&self, value: Id) -> &str {
        match self.heap.get(value) {
            Value::Calendar { value, .. } => value.class(),
            Value::Poll(_) => "select.poll",
            Value::Count { .. } => "count",
            Value::Repeat(_) => "repeat",
            Value::Struct { .. } => "Struct",
            Value::BufferedRandom(..) => "_io.BufferedRandom",
            Value::NamedTuple {
                class: Some(class), ..
            } => class,
            Value::NamedTuple { .. } => "tuple",
            Value::DefaultDict { .. } => "defaultdict",
            Value::TupleGetter { .. } => "_tuplegetter",
            Value::ParamSpec(_) => "ParamSpec",
            Value::ParamSpecMarker(kind, _) => kind.class().rsplit_once('.').unwrap().1,
            Value::ConstEvaluator(_) => "constevaluator",
            Value::FractionHash(_) => "builtin_function_or_method",
            Value::TypeVar(_) => "TypeVar",
            Value::TypeVarTuple { .. } => "TypeVarTuple",
            Value::GenericAlias(alias) => alias.kind.type_name(),
            Value::Union(_) => "Union",
            Value::Method { .. } => "method",
            Value::Cached { .. } => "_lru_cache_wrapper",
            Value::Partial { .. } => "partial",
            Value::PartialPlaceholder => "_PlaceholderType",
            Value::KeyWrapper { .. } => "functools.KeyWrapper",
            Value::Raised(error) => error
                .class()
                .and_then(|class| self.heap.get(class).heap_type())
                .unwrap_or(&error.kind),
            Value::Module {
                constructor: None, ..
            } => "module",
            Value::Instance { class, .. } if let Some(name) = self.heap.get(*class).heap_type() => {
                name
            }
            Value::Instance { class, .. }
                if self.classes.get(super::namespace::CLASS) == Some(class) =>
            {
                super::namespace::CLASS
            }
            Value::Instance { class, .. }
            | Value::Class {
                metaclass: Some(class),
                ..
            }
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

    /// Preserve a native Unicode error's original arguments and its mutable member fields.
    fn codec_exception(&mut self, native: crate::Error, state: Option<DecodeState>) -> Error {
        let mut error = Error::from(native);
        if matches!(&*error.kind, "UnicodeDecodeError" | "UnicodeEncodeError")
            && let Some(state) = state
        {
            let start = self.int(state.start);
            let end = self.int(state.end);
            error.args = Some(self.alloc(Value::Tuple(vec![
                state.encoding,
                state.object,
                start,
                end,
                state.reason,
            ])));
            error.decode = Some(Box::new(state));
        }
        error
    }

    pub(super) fn decode_error(&mut self, native: crate::Error) -> Error {
        let state = native.decode_args().map(|args| DecodeState {
            encoding: self.text(args.encoding),
            object: self.alloc(Value::Bytes(args.object.to_vec())),
            start: args.start,
            end: args.end,
            reason: self.text(args.reason),
        });
        self.codec_exception(native, state)
    }

    pub(super) fn encode_error(&mut self, native: crate::Error, input: Id) -> Error {
        let state = native.encode_args().map(|args| DecodeState {
            encoding: self.text(args.encoding),
            object: input,
            start: args.start,
            end: args.end,
            reason: self.text(args.reason),
        });
        self.codec_exception(native, state)
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
            error.initialize_exit_code(args);
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
        let kind = match (kind, os_args.as_deref()) {
            ("OSError", Some(super::os_error::Args::Native { source, .. })) => {
                super::os_error::subclass(source.errno).unwrap_or(kind)
            }
            _ => kind,
        };
        match message {
            Ok(message) => Self {
                os_args,
                ..Self::new(kind, message.to_string())
            },
            Err(_) => Self::new("MemoryError", String::new()),
        }
    }
}

impl Error {
    /// Format a native fallback diagnostic using the current Python traceback links.
    pub(super) fn report<'a>(&'a self, heap: &'a Heap) -> impl fmt::Display + 'a {
        fmt::from_fn(move |f| self.fmt_trace(f, Some(heap)))
    }

    fn fmt_trace(&self, f: &mut fmt::Formatter<'_>, heap: Option<&Heap>) -> fmt::Result {
        if let Some(trace) = &self.trace
            && (!trace.pending_frames.is_empty() || (heap.is_some() && trace.head != NONE))
        {
            writeln!(f, "Traceback (most recent call last):")?;
            for (loc, _) in trace.pending_frames.iter().rev() {
                writeln!(f, "  File {:?}, line {}", loc.file, loc.line)?;
            }
            if let Some(heap) = heap {
                let mut current = trace.head;
                while let Value::Traceback {
                    frame,
                    lineno,
                    next,
                    ..
                } = heap.get(current)
                {
                    let Value::PythonFrame { code, .. } = heap.get(*frame) else {
                        unreachable!()
                    };
                    let Value::Code(code) = heap.get(*code) else {
                        unreachable!()
                    };
                    let Value::Str(file) = heap.get(code.filename) else {
                        unreachable!()
                    };
                    writeln!(f, "  File {file:?}, line {lineno}")?;
                    current = *next;
                }
            }
        }
        if self.message.is_empty() {
            f.write_str(&self.kind)
        } else {
            write!(f, "{}: {}", self.kind, self.message)
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_trace(f, None)
    }
}

pub(super) type Result<T> = std::result::Result<T, Error>;

/// Data retained by class namespaces after their body has run.
#[derive(Clone, Default)]
pub(super) struct ClassNamespace {
    /// The Decimal class created by an execution of the registered bundled provider.
    pub bundled_decimal: bool,
    /// Qualified name of an iterator type whose instances require private native cursor state.
    pub itertools_layout: Option<&'static str>,
    /// Attribute writes to the type object itself are forbidden, independently of its instance layout.
    pub immutable_type: bool,
    pub bases: Id,
    /// The tuple passed to type.__new__ when it differs from the direct bases' native storage.
    pub supplied_bases: Option<Id>,
    pub mro: Id,
    /// The defining extension module of a heap type created with module state.
    pub defining_module: Option<Id>,
    /// A name assigned through type's __name__ descriptor, including a str subclass.
    pub name_override: Option<Id>,
    /// Backing dictionary shared by live class namespace proxies.
    pub dictionary: Option<Id>,
    /// Slot replacement removed the original type's own instance layout descriptors.
    pub layout_descriptors_cleared: bool,
    /// Closure cell shared by method annotation providers.
    pub annotation_dict_cell: Option<Id>,
    /// Private closure cell shared by a runtime-source class and its annotation provider.
    pub conditional_cell: Option<Id>,
    /// The field dictionary (NONE if unbound) and executed offsets while the class body runs.
    pub annotation_scratch: Option<(Id, HashSet<u32>)>,
    /// Class annotation attributes are served by the installed provider.
    pub provider_annotations: bool,
    /// Run pending instance finalizers after user exit callbacks, while the heap is live.
    pub finalize_at_shutdown: bool,
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

impl Scope {
    /// The class body's live dictionary, once exposed through Python code.
    pub(super) fn class_body_mapping(&self) -> Option<Id> {
        self.class_body.as_ref().and(self.namespace)
    }

    /// type.__qualname__ lives outside the finished class dictionary.
    pub(super) fn binding_namespace(&self, name: &str) -> Option<Id> {
        self.namespace.filter(|_| {
            name != "__qualname__" || self.class_namespace.is_none() || self.class_body.is_some()
        })
    }
}

#[derive(Clone, Default)]
pub(super) struct ClosureScope {
    pub declared: HashSet<String>,
    pub declared_complete: bool,
    pub nonlocals: HashSet<String>,
    pub cells: HashMap<String, Id>,
    /// Keys added by frame introspection that are not lexical variables.
    pub extra_locals: Option<Id>,
    pub cleared: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ExecGlobals {
    pub dict: Id,
    pub builtins: Id,
}

/// Metadata stays readable while a Python source callback owns the decoder.
pub(super) struct BoxedGzipDecoder {
    pub(super) stream: RefCell<Option<crate::objects::GzipDecoder>>,
    pub(super) last_mtime: Cell<Option<u32>>,
}

#[derive(Clone)]
pub(super) struct BoxedTextFile {
    pub(super) file: crate::objects::TextFile,
    pub(super) finalizing: bool,
    pub(super) line_buffering: bool,
    pub(super) buffer: Option<Id>,
}

impl std::ops::Deref for BoxedTextFile {
    type Target = crate::objects::TextFile;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

/// State that belongs to a single initialization of a native module.
#[derive(Clone, Copy)]
pub(super) enum NativeModuleState {
    LocaleError(Id),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    TermiosError(Id),
    IsoCalendarDate(Id),
    SslError(Id),
    PickleWriter {
        pickling_error: Id,
        dispatch_table: Id,
    },
}

impl NativeModuleState {
    fn references(self, visit: &mut MarkQueue<'_>) {
        match self {
            Self::LocaleError(class) | Self::IsoCalendarDate(class) | Self::SslError(class) => {
                visit.push(class)
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Self::TermiosError(class) => visit.push(class),
            Self::PickleWriter {
                pickling_error,
                dispatch_table,
            } => visit.extend([pickling_error, dispatch_table]),
        }
    }
}

/// Private file constructors associated with one low-level I/O module.
#[derive(Clone, Copy)]
pub(super) struct IoOpenTypes {
    pub file: Id,
    pub reader: Id,
    pub writer: Id,
    pub random: Id,
    pub wrapper: Id,
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
        // Preserve a fixed zone's Python constructor objects, including subclasses.
        tzoffset: Id,
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
    RawReadWindow(super::raw_read_window::Window),
    BytesIoStorage(Option<std::rc::Rc<crate::objects::Memory>>),
    BytesIoBuffer {
        stream: Id,
        storage: Id,
    },
    /// An opaque Python buffer acquisition shared by the views derived from it.
    PythonBufferLease {
        exporter: Id,
        view: Id,
        wrapper: Id,
    },
    BufferWrapper,
    MemoryView {
        owner: Option<Id>,
        lease: Option<Id>,
        // An ownerless C buffer has private backing storage but exposes obj as None.
        anonymous: bool,
        // A native export's Python release callback cannot derive another view.
        restricted: bool,
        layout: Option<super::memoryview::ViewLayout>,
        readonly: bool,
        hash: Option<i64>,
    },
    ByteArray(Vec<u8>),
    CodecEncoder(crate::objects::Codec),
    CodecDecoder(crate::objects::Codec),
    Hash(super::stdlib::hashlib::digest::SharedHash),
    Hmac(Hmac),
    Struct {
        format: String,
        layout: Option<std::rc::Rc<crate::objects::interpreter_stdlib::StructFormat>>,
    },
    MetadataArchive(std::rc::Rc<crate::objects::ZipArchive>),
    ExpatParser(std::rc::Rc<super::stdlib::pyexpat::NativeParser>),
    Random(std::rc::Rc<std::cell::RefCell<crate::objects::Generator>>),
    HandlerLock(std::sync::Arc<crate::objects::logging::HandlerLock>),
    #[cfg(unix)]
    MemoryBio(std::rc::Rc<std::cell::RefCell<super::stdlib::ssl::provider::bio::Bio>>),
    #[cfg(unix)]
    TlsContext(std::rc::Rc<std::cell::RefCell<super::stdlib::ssl::provider::Context>>),
    #[cfg(unix)]
    TlsSession(std::rc::Rc<std::cell::RefCell<Option<super::stdlib::ssl::provider::Session>>>),
    #[cfg(unix)]
    TlsListener(crate::ObjectOwner),
    #[cfg(unix)]
    TlsBioSession(std::rc::Rc<std::cell::RefCell<super::stdlib::ssl::provider::bio::BioSession>>),
    Socket(crate::ObjectOwner),
    Deque(Box<crate::objects::DequeRing<Id>>),
    GenericAliasIterator(Option<Id>),
    StructUnpackIterator {
        record: Option<Id>,
        buffer: Option<Id>,
        exporter: Option<Id>,
        offset: usize,
        length: usize,
    },
    MemoryIterator {
        view: Option<Id>,
        index: usize,
        length: usize,
    },
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
    FrameLocalsProxy {
        class: Id,
        frame: Id,
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
        // Private state retained by this generation of a native module.
        module_state: Option<NativeModuleState>,
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
    Coroutine {
        frame: Id,
        origin: Id,
    },
    CoroutineWrapper(Id),
    AsyncGenerator {
        frame: Id,
        running: bool,
        closed: bool,
        finalizer: Option<Id>,
    },
    AsyncGeneratorOperation {
        generator: Id,
        kind: super::async_generator::Operation,
        state: super::async_generator::State,
    },
    AsyncGeneratorYield(Id),
    ANextAwaitable {
        wrapped: Id,
        default: Id,
    },
    Generator {
        owner: FrameOwner,
        frame: Option<Id>,
        name: Id,
        qualname: Id,
        lineno: u32,
        code: Id,
        scope: Id,
        iterators: Vec<Id>,
        protected: Vec<super::source_code::FinallyBlock>,
        offset: Option<usize>,
        running: bool,
    },
    Class {
        /// Type-level sequence and mapping pattern eligibility.
        pattern_flags: super::collection_pattern::PatternFlags,
        /// Private layout tag for native I/O class generations.
        io_kind: Option<super::stdlib::io::NativeClass>,
        /// Qualified name identifying a native heap type and its module generation.
        heap_type: Option<&'static str>,
        metaclass: Option<Id>,
        name: String,
        scope: Id,
        ancestors: Vec<Id>,
        namedtuple_fields: Option<std::rc::Rc<[String]>>,
    },
    ParamSpec(super::paramspec::ParamSpec),
    ParamSpecMarker(super::paramspec::Marker, Id),
    ConstEvaluator(Id),
    FractionHash(Id),
    TypeAlias {
        name: Id,
        parameters: Id,
        definition: super::typing::AliasDefinition,
    },
    TypeVar(super::typing::TypeVariable),
    TypeVarTuple {
        name: Id,
        module: Id,
        default: Id,
        declared: bool,
    },
    NoDefault,
    GenericAlias(super::typing::Alias),
    Union(super::union::Union),
    Instance {
        class: Id,
        fields: indexmap::IndexMap<String, Id>,
        /// Storage addressed by the declaring member, independent of the instance dictionary.
        member_values: Option<hashbrown::HashMap<Id, Id, std::collections::hash_map::RandomState>>,
        /// Exact ordinary instance dictionary once exposed or replaced.
        namespace: Option<Id>,
        dictionary: Option<Id>,
        scalar: Option<Id>,
        property: Option<Box<super::property::Property>>,
        weak_reference: Option<Box<super::weakref::Reference>>,
        itertools: Option<Box<super::stdlib::itertools::Cursor>>,
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
    ThreadHandle(std::sync::Arc<super::thread_worker::Handle>),
    PrimitiveLock(std::sync::Arc<crate::objects::locks::PrimitiveLock>),
    Raised(Error),
    Traceback {
        frame: Id,
        next: Id,
        lineno: i64,
        span: Option<[u32; 3]>,
    },
    PythonFrame {
        // The original namespace outlives execution of a module or class body.
        unoptimized_locals: Option<Id>,
        // A class proxy sees lexical cells, even when its namespace shadows their names.
        class_proxy_scope: Option<Id>,
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
    ClassMethod {
        callable: Id,
        dictionary: Id,
    },
    StaticMethod {
        callable: Id,
        dictionary: Id,
    },
    Property(super::property::Property),
    LayoutDescriptor(Id, &'static str),
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
        fast: bool,
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
    IoOpen {
        types: IoOpenTypes,
        unsupported: Id,
    },
    HeapTypeMethod {
        owner: Id,
        name: &'static str,
    },
    IoMethod {
        owner: Id,
        name: &'static str,
        unsupported: Id,
    },
    ProcessPreparation(std::rc::Rc<std::cell::RefCell<crate::objects::BoxedPreparation>>),
    ProcessHandle {
        child: Option<crate::objects::BoxedPopen>,
        reported: bool,
    },
    File(crate::shared::Shared<super::stdlib::BinaryFile>),
    TextFile(BoxedTextFile),
    GzipDecoder(crate::Shared<BoxedGzipDecoder>),
    ZlibDecompressor {
        state: crate::Shared<std::cell::RefCell<super::stdlib::zlib::Decompressor>>,
        dictionary: Option<Id>,
    },
    LzmaCompressor(crate::Shared<std::cell::RefCell<super::stdlib::lzma::Compressor>>),
    LzmaDecompressor(crate::Shared<std::cell::RefCell<super::stdlib::lzma::Decompressor>>),
    ZlibCompressor(crate::Shared<std::cell::RefCell<super::stdlib::zlib::Compressor>>),
    RegexScanner {
        pattern: Id,
        subject: Id,
        buffer_view: Option<Id>,
        initial_start: usize,
        start: Option<usize>,
        end: usize,
        retry: bool,
    },
    RegexPattern(std::rc::Rc<super::stdlib::regex::Pattern>),
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
            Self::BufferedRandom(..) => Some("_io.BufferedRandom"),
            Self::TextFile(_) => Some("_io.TextIOWrapper"),
            Self::File(file) => Some(match file.as_ref() {
                super::stdlib::BinaryFile::Descriptor(fd) if fd.reading && fd.writing => {
                    "_io.BufferedRandom"
                }
                super::stdlib::BinaryFile::Descriptor(fd) if fd.reading => "_io.BufferedReader",
                super::stdlib::BinaryFile::Descriptor(_) => "_io.BufferedWriter",
                super::stdlib::BinaryFile::Reader { .. } => "_io.BufferedReader",
                super::stdlib::BinaryFile::Writer { .. }
                | super::stdlib::BinaryFile::Standard { .. } => "_io.BufferedWriter",
            }),
            _ => None,
        }
    }

    /// Returns the name used for both builtin class lookup and diagnostic messages.
    pub(super) fn intrinsic_type_name(&self) -> Option<&str> {
        Some(match self {
            Self::Selector(handle) => handle.borrow().class_name(),
            #[cfg(unix)]
            Self::MemoryBio(_) => "_ssl.MemoryBIO",
            #[cfg(unix)]
            Self::TlsContext(_) => "_imre_ssl.Context",
            #[cfg(unix)]
            Self::TlsSession(_) => "_imre_ssl.Session",
            #[cfg(unix)]
            Self::TlsListener(_) => "_imre_ssl.Listener",
            #[cfg(unix)]
            Self::TlsBioSession(_) => "_imre_ssl.BioSession",
            Self::Context(_) => "_contextvars.Context",
            Self::ContextVar { .. } => "_contextvars.ContextVar",
            Self::ContextToken { .. } => "_contextvars.Token",
            Self::Super { .. } => "super",
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
            Self::HeapTypeMethod {
                name: "__repr__" | "__str__",
                ..
            } => "wrapper_descriptor",
            Self::HeapTypeMethod { .. } => "method_descriptor",
            Self::Zip { .. } => "zip",
            Self::Array(_) => "array.array",
            Self::Bytes(_) => "bytes",
            Self::BytesIoStorage(_) => "_imre_bytesio_storage",
            Self::BytesIoBuffer { .. } => "_io._BytesIOBuffer",
            Self::BufferWrapper => "_buffer_wrapper",
            Self::RawReadWindow(_) => "_imre_raw_read_window",
            Self::MemoryView { .. } => "memoryview",
            Self::ByteArray(_) => "bytearray",
            Self::File(_) | Self::TextFile(_) => return self.native_io_class_name(),
            Self::ProcessHandle { .. } => "_imre_subprocess.Handle",
            Self::ProcessPreparation(_) => "_imre_subprocess.Prepared",
            Self::BufferedRandom(..) => "_io.BufferedRandom",
            Self::GzipDecoder(_) => "object",
            Self::ZlibDecompressor { .. } => ZLIB_DECOMPRESS_TYPE,
            Self::LzmaCompressor(_) => LZMA_COMPRESS_TYPE,
            Self::LzmaDecompressor(_) => LZMA_DECOMPRESS_TYPE,
            Self::ZlibCompressor(_) => ZLIB_COMPRESS_TYPE,
            Self::GenericAliasIterator(_) => "generic_alias_iterator",
            Self::Struct { .. } => "struct.Struct",
            Self::StructUnpackIterator { .. } => STRUCT_UNPACK_ITERATOR,
            Self::MemoryIterator { .. } => "memory_iterator",
            Self::SequenceIterator { .. } => "iterator",
            Self::ReverseSequence { .. } => "reversed",
            Self::Deque(_) => super::deque::CLASS,
            Self::DequeIterator { reverse: true, .. } => super::deque::REVERSE_ITERATOR,
            Self::DequeIterator { .. } => super::deque::ITERATOR,
            Self::List(_) => "list",
            Self::CallableIterator(_) => "callable_iterator",
            Self::RegexPattern(_) => REGEX_PATTERN_TYPE,
            Self::RegexMatch(_) => REGEX_MATCH_TYPE,
            Self::RegexScanner { .. } => REGEX_SCANNER_TYPE,
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
            Self::FrameLocalsProxy { .. } => "FrameLocalsProxy",
            Self::Function { .. } => "function",
            Self::TupleGetterDoc | Self::ReadOnlyMember(_, _) | Self::SlotMember { .. } => {
                "member_descriptor"
            }
            Self::ItemGetter(_) => "operator.itemgetter",
            Self::Code(_) => "code",
            Self::Cell(_) => "cell",
            Self::Generator { .. } => "generator",
            Self::Coroutine { .. } => "coroutine",
            Self::CoroutineWrapper(_) => "coroutine_wrapper",
            Self::AsyncGenerator { .. } => "async_generator",
            Self::AsyncGeneratorOperation { kind, .. } => kind.type_name(),
            Self::AsyncGeneratorYield(_) => "async_generator_wrapped_value",
            Self::ANextAwaitable { .. } => "anext_awaitable",
            Self::NoDefault => "NoDefaultType",
            Self::TypeAlias { .. } => "typing.TypeAliasType",
            Self::Class { .. } => "type",
            Self::DictView { kind, ordered, .. } => match (kind, ordered) {
                (DictViewKind::Keys, false) => "dict_keys",
                (DictViewKind::Items, false) => "dict_items",
                (DictViewKind::Values, false) => "dict_values",
                (DictViewKind::Keys, true) => "odict_keys",
                (DictViewKind::Items, true) => "odict_items",
                (DictViewKind::Values, true) => "odict_values",
            },
            Self::ClassMethod { .. } => "classmethod",
            Self::ThreadLocal(_) => super::thread_local::CLASS,
            Self::ThreadHandle(_) => "_thread._ThreadHandle",
            Self::Random(_) => "Random",
            Self::HandlerLock(_) => super::thread_rlock::CLASS,
            Self::PrimitiveLock(_) => super::thread_lock::CLASS,
            Self::FormatIterator { kind, .. } => match kind {
                FormatKind::Markup => "formatteriterator",
                FormatKind::Field => "fieldnameiterator",
            },
            Self::StaticMethod { .. } => "staticmethod",
            Self::Property(_) => "property",
            Self::GetSetDescriptor { .. } | Self::LayoutDescriptor(..) => "getset_descriptor",
            Self::Traceback { .. } => "traceback",
            Self::PythonFrame { .. } => "frame",
            _ => return None,
        })
    }

    pub(super) fn hash_type(&self) -> Option<&'static str> {
        self.heap_type()
            .filter(|name| *name == super::stdlib::hashlib::md5::CLASS)
    }

    pub(super) fn heap_type(&self) -> Option<&'static str> {
        match self {
            Self::Class { heap_type, .. } => *heap_type,
            _ => None,
        }
    }

    pub(super) fn io_kind(&self) -> Option<&'static str> {
        match self {
            Self::Class {
                io_kind: Some(super::stdlib::io::NativeClass::Named(name)),
                ..
            } => Some(name),
            Self::Class {
                io_kind: Some(super::stdlib::io::NativeClass::Pair { .. }),
                ..
            } => Some("BufferedRWPair"),
            _ => None,
        }
    }

    /// Only Python instances and exception subclasses have instance finalizers.
    pub(super) fn finalizer_class(&self) -> Option<Id> {
        match self {
            Self::Instance { class, .. } => Some(*class),
            Self::Raised(error) => error.class(),
            _ => None,
        }
    }

    pub(super) fn instance_class(&self) -> Option<Id> {
        match self {
            Self::Instance { class, .. } | Self::FrameLocalsProxy { class, .. } => Some(*class),
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
            | Self::LoopIterator { .. }
            | Self::PythonBufferLease { .. }
            | Self::ProcessPreparation(_)
            | Self::ProcessHandle { .. }
            | Self::Yielded(_) => false,
            Self::Tuple(items) => !items.is_empty(),
            _ => true,
        }
    }

    /// Whether allocating this boxed slot represents a Python object allocated with a GC header.
    fn has_gc_header(&self, heap: &Heap) -> bool {
        use crate::objects::HashState;
        match self {
            // A calendar subclass has its own instance slot. Its scalar payload stays untracked,
            // including when a time or datetime retains a user-defined tzinfo.
            Self::Calendar { .. } => false,
            // The BytesIO instance owns its single GC header. Its byte buffer adds none.
            Self::BytesIoStorage(_) => false,
            Self::Instance { class, .. } => !matches!(
                heap.get(*class),
                Self::Native(name) if matches!(name.as_str(), "object" | super::stdlib::datetime::TZINFO)
            ),
            // OpenSSL hashes have no GC header. The fallback MD5 instance owns a separate boxed
            // state, whereas a BLAKE2 or SHA-3 provider object is represented directly by its state.
            Self::Hash(state) => !matches!(
                state.lock().class_name(),
                HashState::CLASS
                    | HashState::XOF_CLASS
                    | HashState::OPENSSL_CLASS
                    | super::stdlib::hashlib::md5::CLASS
            ),
            _ => self.gc_object(),
        }
    }

    fn scalar_leaf(&self) -> bool {
        matches!(
            self,
            Value::None
                | Value::Bool(_)
                | Value::Int(_)
                | Value::Float(_)
                | Value::Complex(_)
                | Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
                | Value::Native(_)
        )
    }

    fn children(&self, visit: &mut MarkQueue<'_>) {
        match self {
            Self::Deque(ring) => visit.extend(&ring.items),
            Self::StructUnpackIterator {
                record,
                buffer,
                exporter,
                ..
            } => {
                visit.extend(record);
                visit.extend(buffer);
                visit.extend(exporter);
            }
            Self::MemoryIterator { view: source, .. }
            | Self::SequenceIterator { source, .. }
            | Self::ReverseSequence { source, .. }
            | Self::GenericAliasIterator(source) => visit.extend(*source),
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
            Self::ThreadLocal(local) => visit.extend(local.live()),
            Self::Count { current, step, .. } => visit.extend([*current, *step]),
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
                visit.extend(entries.ordered_nodes().iter().map(|&(key, _)| key));
                for (key, value) in entries {
                    visit.push(*value);
                    visit.push(*key);
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
            Self::BytesIoStorage(_) | Self::BufferWrapper => {}
            Self::BytesIoBuffer { stream, storage } => visit.extend([*stream, *storage]),
            Self::RawReadWindow(window) => visit.push(window.backing),
            Self::PythonBufferLease {
                exporter,
                view,
                wrapper,
            } => visit.extend([*exporter, *view, *wrapper]),
            Self::MemoryView { owner, lease, .. } => {
                visit.extend(owner);
                visit.extend(lease);
            }
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
            Self::Calendar {
                tzinfo,
                tzoffset,
                tzname,
                ..
            } => visit.extend([*tzinfo, *tzoffset, *tzname]),
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
            Self::FrameLocalsProxy { class, frame } => visit.extend([*class, *frame]),
            Self::Scope(scope) => {
                visit.extend(scope.closure.extra_locals);
                visit.extend(scope.closure.cells.values().copied());
                visit.extend(scope.class_cell);
                if let Some(state) = &scope.enum_namespace {
                    visit.extend(state.generator);
                    visit.extend(&state.history);
                }
                if let Some(class) = &scope.class_namespace {
                    visit.extend([class.bases, class.mro]);
                    visit.extend(class.supplied_bases);
                    visit.extend(class.name_override);
                    visit.extend(class.annotation_dict_cell);
                    visit.extend(class.conditional_cell);
                    if let Some((fields, _)) = &class.annotation_scratch {
                        visit.push(*fields);
                    }
                    visit.extend(class.dictionary);
                    visit.extend(class.defining_module);
                }
                visit.extend(scope.parent);
                visit.extend(scope.namespace);
                visit.extend(scope.exec_locals);
                if let Some(globals) = scope.exec_globals {
                    visit.extend([globals.dict, globals.builtins]);
                }
                visit.extend(scope.names.values());
            }
            Self::Super {
                start,
                receiver,
                owner,
            } => visit.extend([*start, *receiver, *owner]),
            Self::Module {
                scope,
                origin,
                constructor,
                module_state,
                ..
            } => {
                visit.push(*scope);
                if let Some(state) = module_state {
                    state.references(visit);
                }
                visit.extend(origin);
                visit.extend(constructor);
            }
            Self::Cell(value) => visit.extend(value),
            Self::Coroutine { frame, origin } => visit.extend([*frame, *origin]),
            Self::CoroutineWrapper(coroutine) => visit.push(*coroutine),
            Self::AsyncGenerator {
                frame, finalizer, ..
            } => {
                visit.push(*frame);
                visit.extend(finalizer);
            }
            Self::AsyncGeneratorOperation {
                generator, kind, ..
            } => {
                visit.push(*generator);
                if let Some(argument) = kind.argument() {
                    visit.push(argument);
                }
            }
            Self::AsyncGeneratorYield(item) => visit.push(*item),
            Self::ANextAwaitable { wrapped, default } => visit.extend([*wrapped, *default]),
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
                visit.push(*scope);
                visit.push(*code);
                visit.push(*defaults);
                visit.push(*kwdefaults);
                visit.push(*closure);
                visit.extend(class_cell);
                fields.children(visit);
            }
            Self::Code(code) => {
                visit.push(code.name);
                visit.push(code.qualname);
                visit.push(code.doc);
                visit.push(code.filename);
                visit.push(code.parameters);
                visit.extend(code.nested.iter().map(|&(_, id)| id));
                if let Body::Source(program) = &code.body {
                    visit.push(program.string_roots);
                }
            }
            Self::Generator {
                owner,
                frame,
                name,
                qualname,
                code,
                scope,
                iterators,
                protected,
                ..
            } => {
                visit.extend([*code, *scope, *name, *qualname, owner.root()]);
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
                io_kind,
                ..
            } => {
                if let Some(super::stdlib::io::NativeClass::Pair { reader, writer }) = io_kind {
                    visit.extend([*reader, *writer]);
                }
                visit.extend(*metaclass);
                visit.push(*scope);
                visit.extend(ancestors);
            }
            Self::TypeVarTuple {
                name,
                module,
                default,
                ..
            } => visit.extend([*name, *module, *default]),
            Self::ParamSpec(p) => visit.extend([p.name, p.bound, p.default, p.namespace]),
            Self::ParamSpecMarker(_, origin)
            | Self::ConstEvaluator(origin)
            | Self::FractionHash(origin) => visit.push(*origin),
            Self::TypeAlias {
                name,
                parameters,
                definition,
            } => {
                visit.extend([*name, *parameters]);
                match *definition {
                    super::typing::AliasDefinition::Lazy { evaluator, cached } => {
                        visit.push(evaluator);
                        visit.extend(cached);
                    }
                    super::typing::AliasDefinition::Eager { value, module } => {
                        visit.extend([value, module]);
                    }
                }
            }
            Self::TypeVar(var) => visit.extend([
                var.name,
                var.module,
                var.bound,
                var.constraints,
                var.default,
            ]),
            Self::GenericAlias(alias) => {
                visit.extend([alias.origin, alias.args, alias.parameters]);
                visit.extend(alias.name);
                visit.extend(alias.naming_fields);
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
                itertools,
                memory_stream,
                property,
            } => {
                if let Some(property) = property {
                    property.children(visit);
                }
                if let Some(reference) = weak_reference {
                    visit.push(reference.callback);
                }
                if let Some(cursor) = itertools {
                    cursor.children(visit);
                }
                match memory_stream {
                    Some(super::stdlib::io::MemoryStream::BufferedFile(file)) => visit.push(*file),
                    Some(super::stdlib::io::MemoryStream::Reader(reader)) => {
                        visit.extend(reader.raw.get());
                        visit.push(reader.buffer);
                    }
                    Some(
                        super::stdlib::io::MemoryStream::UninitializedReader(raw)
                        | super::stdlib::io::MemoryStream::UninitializedWriter(raw),
                    ) => visit.extend(raw),
                    Some(super::stdlib::io::MemoryStream::BufferedBytes(writer)) => {
                        visit.extend(writer.raw.get())
                    }
                    Some(super::stdlib::io::MemoryStream::Pair(pair)) => {
                        visit.extend(pair.reader);
                        visit.extend(pair.writer);
                    }
                    Some(super::stdlib::io::MemoryStream::Bytes(stream)) => {
                        visit.push(stream.storage);
                        visit.extend(stream.snapshot.get())
                    }
                    Some(super::stdlib::io::MemoryStream::Wrapper(stream)) => {
                        stream.children(visit)
                    }
                    Some(super::stdlib::io::MemoryStream::Decoder(decoder)) => {
                        visit.extend([decoder.delegate, decoder.errors]);
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
            Self::ClassMethod {
                callable,
                dictionary,
            }
            | Self::StaticMethod {
                callable,
                dictionary,
            } => visit.extend([*callable, *dictionary]),
            Self::Property(property) => property.children(visit),
            Self::Weak(wr) => visit.push(wr.callback),
            Self::LayoutDescriptor(owner, _) => visit.push(*owner),
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
                unoptimized_locals,
                class_proxy_scope,
                scope,
                code,
                globals,
                back,
                ..
            } => {
                visit.extend(*unoptimized_locals);
                visit.extend(*class_proxy_scope);
                visit.extend([*scope, *code, *globals]);
                visit.extend(*back);
            }
            Self::IoOpen { types, unsupported } => visit.extend([
                types.file,
                types.reader,
                types.writer,
                types.random,
                types.wrapper,
                *unsupported,
            ]),
            Self::IoMethod {
                owner, unsupported, ..
            } => visit.extend([*owner, *unsupported]),
            Self::HeapTypeMethod { owner, .. } => visit.push(*owner),
            Self::RegexScanner {
                pattern,
                subject,
                buffer_view,
                ..
            } => {
                visit.extend([*pattern, *subject]);
                visit.extend(*buffer_view);
            }
            Self::RegexPattern(pattern) => visit.push(pattern.source),
            Self::RegexMatch(m) => {
                visit.extend([m.pattern, m.subject]);
                visit.extend(m.cached_regs.get().copied());
            }
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
            | Self::CodecEncoder(_)
            | Self::CodecDecoder(_)
            | Self::Struct { .. }
            | Self::Hash(_)
            | Self::MetadataArchive(_)
            | Self::ExpatParser(_)
            | Self::Random(_)
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
            | Self::ProcessPreparation(_)
            | Self::GzipDecoder(_)
            | Self::LzmaCompressor(_)
            | Self::ZlibCompressor(_)
            | Self::ProcessHandle { .. } => {}
            #[cfg(unix)]
            Self::MemoryBio(_)
            | Self::TlsContext(_)
            | Self::TlsSession(_)
            | Self::TlsListener(_)
            | Self::TlsBioSession(_) => {}
            Self::LzmaDecompressor(state) => visit.push(state.borrow().unused_data),
            Self::ZlibDecompressor { state, dictionary } => {
                visit.extend(*dictionary);
                state.borrow().children(visit);
            }
            Self::TextFile(file) => visit.extend(file.buffer),
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
    order_state: Option<usize>,
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
        ordered: Option<(usize, usize)>,
        reverse_end: Option<usize>,
    ) -> Self {
        Self {
            dict: Some(dict),
            kind,
            expected_len: Some(size),
            position: reverse_end.unwrap_or(0),
            reverse: reverse_end.is_some(),
            remaining: ordered.map_or(size, |(_, count)| count),
            order_state: ordered.map(|(revision, _)| revision),
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
        index
            .filter(|&i| ctx.stored_entries(storage).ordered_rank(i).is_some())
            .ok_or_else(|| ctx.lookup_error("KeyError", key))
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
            let rank = rank?;
            let &(key, _) = entries.ordered_nodes().get(rank)?;
            // Ordered cursors resolve the retained key against the live dictionary.
            Some((rank, key, NONE))
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
            if order != entries.order_state() {
                self.dict = None;
                return Err(Error::new(
                    "RuntimeError",
                    "OrderedDict mutated during iteration",
                ));
            }
        }
        if self.expected_len != Some(entries.len()) {
            self.expected_len = None;
            let message = if self.order_state.is_some() {
                "OrderedDict changed size during iteration"
            } else {
                "dictionary changed size during iteration"
            };
            return Err(Error::new("RuntimeError", message));
        }
        let entry = self.entry_at(entries, self.position);
        let Some((position, mut key, value)) = entry else {
            self.dict = None;
            return Ok(None);
        };
        if self.order_state.is_none() && !self.reverse && self.remaining == 0 {
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
        self.remaining -= 1;
        Ok(Some((storage, key, value)))
    }
}

// A slot's trace link is UNMARKED until queued. END terminates the queue and marks a
// visited slot. Both sentinels exceed the largest possible Vec index.
const UNMARKED: usize = usize::MAX;
const END: usize = usize::MAX - 1;

/// Minimum allocation allowance for separately owned Python bytes buffers.
const BYTE_BUFFER_FLOOR: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Finalization {
    None,
    Pending,
    Done,
}

struct Slot {
    value: Option<Value>,
    // Next in collection order while occupied, or next free slot while vacant.
    gc_next: Cell<usize>,
    // Previous in collection order while occupied.
    gc_prev: Cell<usize>,
    untracked_tuple: Cell<bool>,
    // GC-header allocations still contribute pressure when a tuple becomes untracked.
    has_gc_header: bool,
    may_have_buffer_views: bool,
    finalization: Finalization,
    generation: u8,
}

impl Slot {
    fn new(value: Value) -> Self {
        Self {
            value: Some(value),
            gc_next: Cell::new(UNMARKED),
            gc_prev: Cell::new(UNMARKED),
            untracked_tuple: Cell::new(false),
            has_gc_header: false,
            may_have_buffer_views: false,
            finalization: Finalization::None,
            generation: 0,
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
        slots[id].gc_prev.set(self.tail);
        self.tail = id;
        slots[id].gc_next.set(END);
    }

    fn pop(&mut self, slots: &[Slot]) -> Option<usize> {
        let id = self.head;
        if id == END {
            return None;
        }
        self.remove(id, slots);
        Some(id)
    }

    /// Unlink an occupied slot while keeping survivors in collection order.
    fn remove(&mut self, id: usize, slots: &[Slot]) {
        let previous = slots[id].gc_prev.replace(UNMARKED);
        let next = slots[id].gc_next.replace(UNMARKED);
        if previous == END {
            self.head = next;
        } else {
            slots[previous].gc_next.set(next);
        }
        if next == END {
            self.tail = previous;
        } else {
            slots[next].gc_prev.set(previous);
        }
    }
}

/// The trace links are reserved with each heap slot, so marking needs no scratch allocation.
pub(super) struct MarkQueue<'a> {
    slots: &'a [Slot],
    traces: &'a [Cell<usize>],
    scalar_leaves: &'a [bool],
    next: usize,
    mode: MarkMode<'a>,
}

enum MarkMode<'a> {
    All,
    SkipScalarLeaves,
    Ordered(&'a mut GcOrder),
}

impl MarkQueue<'_> {
    pub(super) fn push(&mut self, id: Id) {
        let trace = &self.traces[id as usize];
        if trace.get() == UNMARKED {
            match &mut self.mode {
                MarkMode::Ordered(order) => {
                    trace.set(END);
                    if self.slots[id as usize].gc_next.get() == UNMARKED {
                        order.push(id as usize, self.slots);
                    }
                }
                // A complete reachability trace needs no pending visit for an exact scalar.
                MarkMode::SkipScalarLeaves if self.scalar_leaves[id as usize] => {
                    trace.set(END);
                }
                _ => {
                    trace.set(self.next);
                    self.next = id as usize;
                }
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
        self.next = self.traces[index].replace(END);
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
    pub(super) allocation_traces: super::allocation_traces::AllocationTraces,
    pub(super) loading_linked_constant: bool,
    // Keep the collector's links contiguous so it can scan marks without loading every Value.
    traces: Vec<Cell<usize>>,
    // Marking reads this compact table before touching the much larger value slots.
    scalar_leaves: Vec<bool>,
    gc_order: GcOrder,
    linked_string_constants: HashMap<i64, Id>,
    short_bytes: HashMap<Option<u8>, Id>,
    full_collection: Option<CycleSet>,
    // Native owners identify spellings. Runtime-created strings keep only a weak boxed handle.
    interned: HashMap<crate::StrOwner, InternedString>,
    // ABC registrations are weak edges. Sweep them before recycling a dead class's slot.
    pub(super) abc_registrations: Vec<[Id; 2]>,
    // ABC subclass results use weak keys just like ABC registrations.
    pub(super) abc_subclass_cache: HashMap<[Id; 2], bool>,
    // Collecting weak registration pairs must not roll back the invalidation counter.
    pub(super) abc_cache_token: u64,
    free_head: usize,
    free_count: usize,
    pub(super) immutable_hashes: HashMap<Id, i64>,
    frames: Vec<Frame>,
    fault_generation: Cell<Option<u32>>,
    fault_workers: RefCell<Option<Weak<()>>>,
    next_collection: usize,
    byte_buffer_allowance: usize,
    pub(super) gc_settings: Arc<GcSettings>,
    pub(super) gc_schedule: GcSchedule,
    weak: HashMap<Id, Vec<Id>>,
    weak_roots: weak_roots::RootPaths,
    finalizer_roots: weak_roots::RootPaths,
    // Callback rollback can borrow a handle without extending the object's lifetime.
    pub(super) borrowed_hooks: Vec<Id>,
    // Detached workers compare against the original stderr without keeping it alive.
    pub(super) default_stderr: Option<Id>,
    // A dead proxy is still observable after its registry entry disappears.
    pub(super) has_weak_values: bool,
    collect_next_statement: bool,
    // Weak handles to buffered output files returned by open().
    file_writers: Vec<Id>,
    // Weak registrations. The slot remembers prior finalization across resurrection.
    ordinary_finalizers: Vec<Id>,
    // Positions are hints. Each check reads the current binding and native stream edges.
    finalizer_bindings: Cell<[Option<(usize, usize)>; 2]>,
    // Weak handles in direct-base attachment order, pruned before recycling class slots.
    classes: Vec<Id>,
    // During shutdown, remember if a sweep discarded unflushed Python-backed output.
    pub(super) discarded_shutdown_write: Option<bool>,
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

#[derive(Clone, Copy, Default)]
pub(super) enum ExpressionPosition {
    #[default]
    Statement,
    Line(u32),
    Source {
        offset: ruff_text_size::TextSize,
        range: ruff_text_size::TextRange,
    },
}

struct Frame {
    owner: FrameOwner,
    scope: Id,
    code: Id,
    live: Vec<Id>,
    location: Option<Location>,
    expression: ExpressionPosition,
    call_position: Option<[u32; 4]>,
    object: Option<Id>,
    inherited: Id,
    active: Id,
    // This activation or one of its calls opened a buffered output file.
    created_finalizer: bool,
}

impl Frame {
    fn line(&self, heap: &Heap, location: &Location) -> u32 {
        match self.expression {
            ExpressionPosition::Line(line) => line,
            ExpressionPosition::Source { offset, .. } => {
                if let Value::Code(code) = heap.get(self.code)
                    && let Body::Source(program) = &code.body
                {
                    return program.expression_line(offset).unwrap_or(location.line);
                }
                location.line
            }
            ExpressionPosition::Statement => location.line,
        }
    }

    fn new(scope: Id, code: Id, owner: FrameOwner, inherited: Id) -> Self {
        Self {
            owner,
            scope,
            code,
            live: Vec::new(),
            location: None,
            expression: ExpressionPosition::Statement,
            call_position: None,
            object: None,
            inherited,
            active: inherited,
            created_finalizer: false,
        }
    }
}

impl Heap {
    /// Builtin and immutable native types stop flag propagation at that node.
    fn needs_pattern_flags(&self, class: Id, new: super::collection_pattern::PatternFlags) -> bool {
        let Value::Class {
            pattern_flags,
            scope,
            io_kind: None,
            heap_type,
            ..
        } = self.get(class)
        else {
            return false;
        };
        if matches!(
            *heap_type,
            Some(super::stdlib::hashlib::md5::CLASS | super::stdlib::datetime::isocalendar::CLASS)
        ) {
            return false;
        }
        let Value::Scope(data) = self.get(*scope) else {
            unreachable!()
        };
        *pattern_flags != new
            && !data
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.immutable_type)
    }

    /// Propagate a successful ABC registration to existing nominal subclasses.
    /// The graph lasts only for this update, so it cannot keep dead classes alive.
    pub(super) fn set_registered_pattern_flags(
        &mut self,
        class: Id,
        flags: super::collection_pattern::PatternFlags,
    ) {
        if !self.needs_pattern_flags(class, flags) {
            return;
        }
        let mut children: HashMap<Id, Vec<Id>> = HashMap::new();
        for &child in &self.classes {
            for &base in self.immediate_bases(child) {
                children.entry(base).or_default().push(child);
            }
        }
        let mut pending = vec![class];
        while let Some(current) = pending.pop() {
            if !self.needs_pattern_flags(current, flags) {
                continue;
            }
            let Value::Class { pattern_flags, .. } = self.get_mut(current) else {
                unreachable!()
            };
            *pattern_flags = flags;
            if let Some(derived) = children.remove(&current) {
                pending.extend(derived);
            }
        }
    }

    /// Bytes reserved for each boxed value's slot metadata and trace link, including alignment.
    pub(super) fn size_overhead() -> usize {
        size_of::<Slot>() + size_of::<Cell<usize>>() - size_of::<Value>()
    }

    pub fn new() -> Self {
        let mut slots = vec![
            Slot::new(Value::None),
            Slot::new(Value::Bool(false)),
            Slot::new(Value::Bool(true)),
            Slot::new(Value::Ellipsis),
            Slot::new(Value::NotImplemented),
            Slot::new(Value::Tuple(Vec::new())),
        ];
        for n in SMALL_INT_MIN..=SMALL_INT_MAX {
            slots.push(Slot::new(Value::Int(crate::Int::from(n))));
        }
        let mut heap = Self {
            traces: vec![Cell::new(END); slots.len()],
            scalar_leaves: slots
                .iter()
                .map(|slot| slot.value.as_ref().is_some_and(Value::scalar_leaf))
                .collect(),
            slots,
            allocation_traces: Default::default(),
            loading_linked_constant: false,
            gc_order: GcOrder::EMPTY,
            full_collection: None,
            linked_string_constants: HashMap::new(),
            short_bytes: HashMap::new(),
            interned: HashMap::new(),
            abc_registrations: Vec::new(),
            abc_subclass_cache: HashMap::new(),
            abc_cache_token: 0,
            free_head: END,
            free_count: 0,
            immutable_hashes: HashMap::new(),
            frames: vec![Frame::new(NONE, NONE, FrameOwner::Module(NONE), NONE)],
            fault_generation: Cell::new(None),
            fault_workers: RefCell::new(None),
            next_collection: FIRST_DYNAMIC_SLOT + 4096,
            byte_buffer_allowance: BYTE_BUFFER_FLOOR,
            gc_settings: Arc::default(),
            gc_schedule: GcSchedule::default(),
            weak: HashMap::new(),
            weak_roots: Default::default(),
            finalizer_roots: Default::default(),
            borrowed_hooks: Vec::new(),
            default_stderr: None,
            has_weak_values: false,
            collect_next_statement: false,
            file_writers: Vec::new(),
            ordinary_finalizers: Vec::new(),
            finalizer_bindings: Cell::new([None; 2]),
            classes: Vec::new(),
            discarded_shutdown_write: None,
            caller_finalizer_field: None,
        };
        for id in 0..FIRST_SMALL_INT as usize {
            heap.gc_order.push(id, &heap.slots);
        }
        heap
    }

    /// The builtin mapping payload is independent of a subclass instance's attributes.
    pub(super) fn dict_storage(&self, value: Id) -> Option<Id> {
        match self.get(value) {
            Value::Dict(_) | Value::DefaultDict { .. } => Some(value),
            Value::Instance { dictionary, .. } => *dictionary,
            _ => None,
        }
    }

    pub(super) fn dict_entries(&self, value: Id) -> Option<&[(Id, Id)]> {
        match self.get(self.dict_storage(value)?) {
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => Some(entries),
            _ => unreachable!("dictionary payload"),
        }
    }

    pub(super) fn dict_named_item(&self, dict: Id, name: &str) -> Option<Id> {
        self.dict_entries(dict)?.iter().find_map(|&(key, value)| {
            matches!(self.get(key), Value::Str(k) if k == name).then_some(value)
        })
    }

    /// Look up native backing fields on either side of ordinary dictionary materialization.
    pub(super) fn raw_instance_field(&self, obj: Id, name: &str) -> Option<Id> {
        let Value::Instance {
            fields, namespace, ..
        } = self.get(obj)
        else {
            return None;
        };
        fields
            .get(name)
            .copied()
            .or_else(|| self.dict_named_item((*namespace)?, name))
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

    /// An interpreter root (such as packed callback operands) can own weak targets indirectly.
    pub(super) fn observes_released_roots(&self) -> bool {
        self.has_weak_values || self.tracks_finalizers()
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
            return self.intern_literal(owner, true);
        }
        if let Value::Int(integer) = &value
            && let Some(n @ SMALL_INT_MIN..=SMALL_INT_MAX) = integer.as_small()
        {
            return FIRST_SMALL_INT + (n - SMALL_INT_MIN) as Id;
        }
        match value {
            Value::Bytes(bytes) if bytes.is_empty() => self.bytes_from_data(bytes),
            Value::Bytes(bytes) => self.alloc_bytes(bytes, true),
            Value::Tuple(items) if items.is_empty() => EMPTY_TUPLE,
            Value::Class { .. } => {
                let id = self.alloc_slot(value);
                self.classes.push(id);
                id
            }
            value => self.alloc_slot(value),
        }
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

    /// Charge buffers larger than a slot to the next statement's collection budget.
    fn alloc_bytes(&mut self, bytes: Vec<u8>, trace: bool) -> Id {
        if bytes.capacity() > std::mem::size_of::<Slot>() {
            self.byte_buffer_allowance =
                self.byte_buffer_allowance.saturating_sub(bytes.capacity());
            if self.byte_buffer_allowance == 0 && !self.full_collection_active() {
                self.collect_next_statement = true;
            }
        }
        self.alloc_slot_with_trace(Value::Bytes(bytes), trace)
    }

    /// Reuse CPython's empty and one-byte singletons when copying known byte data.
    /// Allocating an uninitialized one-byte result (for example, bytes(1)) is distinct.
    pub(super) fn bytes_from_data(&mut self, bytes: impl AsRef<[u8]> + Into<Vec<u8>>) -> Id {
        if bytes.as_ref().len() > 1 {
            return self.alloc_bytes(bytes.into(), true);
        }
        let byte = bytes.as_ref().first().copied();
        if let Some(&id) = self.short_bytes.get(&byte) {
            return id;
        }
        let id = self.alloc_bytes(bytes.into(), false);
        self.short_bytes.insert(byte, id);
        id
    }

    /// Allocate a distinct string result even when its value is a cached Latin-1 character.
    pub(super) fn fresh_string(&mut self, value: Value) -> Result<Id> {
        debug_assert!(matches!(value, Value::Str(_) | Value::SurrogateStr(_)));
        if self.free_head == END {
            self.reserve_slot()
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

    fn reserve_slot(&mut self) -> std::result::Result<(), std::collections::TryReserveError> {
        self.scalar_leaves.try_reserve(1)?;
        self.traces.try_reserve(1)?;
        self.slots.try_reserve(1)
    }

    pub(super) fn gc_thresholds(&self) -> [i32; 3] {
        self.gc_settings.thresholds()
    }

    pub(super) fn set_gc_thresholds(&self, thresholds: &[i32]) {
        self.gc_settings.set_thresholds(thresholds);
    }

    fn alloc_slot(&mut self, value: Value) -> Id {
        self.alloc_slot_with_trace(value, true)
    }

    fn alloc_slot_with_trace(&mut self, value: Value, trace: bool) -> Id {
        let trace = trace && self.allocation_traces.is_tracing();
        let has_gc_header = value.has_gc_header(self);
        let scalar_leaf = value.scalar_leaf();
        let index = if self.free_head != END {
            let index = self.free_head;
            self.free_head = self.slots[index].gc_next.replace(UNMARKED);
            self.free_count -= 1;
            self.scalar_leaves[index] = scalar_leaf;
            self.slots[index].value = Some(value);
            self.slots[index].may_have_buffer_views = false;
            self.slots[index].generation = 0;
            index
        } else {
            if self.reserve_slot().is_err() {
                // Keep space for the next value when doubling the heap fails.
                // The next statement can collect with a complete set of live handles.
                self.traces.reserve_exact(1);
                self.slots.reserve_exact(1);
                self.next_collection = self.slots.len();
            }
            self.scalar_leaves.push(scalar_leaf);
            self.traces.push(Cell::new(UNMARKED));
            self.slots.push(Slot::new(value));
            self.slots.len() - 1
        };
        self.gc_order.push(index, &self.slots);
        self.slots[index].untracked_tuple.set(false);
        self.slots[index].has_gc_header = has_gc_header;
        if has_gc_header {
            let collecting = self.full_collection_active();
            self.gc_schedule.allocated(&self.gc_settings, collecting);
        }
        self.slots[index].finalization = Finalization::None;
        let id = index as Id;
        if trace {
            if self.loading_linked_constant {
                self.allocation_traces.record_linked(id);
            } else {
                self.allocation_traces.record(
                    id,
                    self.frames.iter().rev().filter_map(|frame| {
                        let location = frame.location.as_ref()?;
                        Some((location.file.as_ref(), frame.line(self, location)))
                    }),
                );
            }
        }
        self.keep(id);
        id
    }

    /// A code literal and every occurrence of a Latin-1 character share their Python handle.
    fn intern_literal(&mut self, owner: crate::StrOwner, static_character: bool) -> Id {
        if let Some(entry) = self.interned.get_mut(&owner) {
            entry.literal = true;
            let id = entry.id;
            self.keep(id);
            return id;
        }
        let id = self.alloc_slot_with_trace(Value::Str(owner.to_string()), !static_character);
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
        if let Some(owner) = crate::objects::short_string(text) {
            return self.intern_literal(owner, true);
        }
        self.intern_literal(
            crate::sys::intern_owner(&crate::objects::literal_string(text)),
            false,
        )
    }

    pub fn string_literal(&mut self, text: &str) -> Result<Id> {
        if let Some(owner) = crate::objects::short_string(text) {
            return Ok(self.intern_literal(owner, true));
        }
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
        // The mutable borrow can replace the value with a different variant.
        self.scalar_leaves[id as usize] = false;
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
        if self.fault_frames_current() {
            self.publish_fault_frame(self.python_depth() - 1);
        }
    }
    /// Release the activation and report whether a returning function may have dropped a writer.
    pub fn leave(&mut self, value: Id, returning: bool) -> bool {
        let frame = self.frames.pop().expect("returning native activation");
        if let FrameOwner::Generator(id) = frame.owner
            && let Some(location) = &frame.location
            && let Value::Generator { lineno, .. } = self.get_mut(id)
        {
            *lineno = location.line;
        }
        let depth = self.python_depth();
        if depth == 0 {
            // Explicit dumps can publish a snapshot even while disabled.
            if self.fault_generation.take().is_some() {
                crate::fault_handler::truncate(0);
            }
        } else if self.fault_frames_current() {
            crate::fault_handler::truncate(depth);
            // The popped frame displaced this caller in the bounded native snapshot.
            if let Some(index) = depth.checked_sub(crate::fault_handler::MAX_FRAMES) {
                self.publish_fault_frame(index);
            }
        }
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

    fn finalizer_free_scalar(&self, id: Id) -> bool {
        self.get(id).scalar_leaf()
    }

    /// Regex matches and patterns on exact text or bytes hold only their subject and pattern source.
    fn plain_regex_operand(&self, id: Id) -> bool {
        let weak = |id| self.has_weak_values && self.weak_edge(id);
        let text = |id| {
            matches!(
                self.get(id),
                Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
            ) && !weak(id)
        };
        let pattern =
            |id| matches!(self.get(id), Value::RegexPattern(p) if text(p.source)) && !weak(id);
        let (value, callable) = match self.get(id) {
            Value::Method {
                callable, receiver, ..
            } => (*receiver, Some(*callable)),
            Value::RegexPattern(_) | Value::RegexMatch(_) => (id, None),
            _ => return false,
        };
        !weak(id)
            && callable.is_none_or(|f| matches!(self.get(f), Value::Native(_)) && !weak(f))
            && (pattern(value)
                || matches!(self.get(value), Value::RegexMatch(m)
                    if m.cached_regs.get().is_none() && !weak(value) && text(m.subject) && pattern(m.pattern)))
    }

    /// Release inert call arguments and peers at the same boundary as the general discard.
    pub(super) fn discard_scalar_call(
        &mut self,
        mark: usize,
        result: Id,
        arguments: Id,
        callable: Id,
    ) -> bool {
        if !self.tracks_finalizers() {
            return false;
        }
        let Value::List(args) = self.get(arguments) else {
            return false;
        };
        let tuple_argument = (2..=8).contains(&args.len())
            && args
                .iter()
                .any(|&id| matches!(self.get(id), Value::Tuple(_)));
        let regex_call = matches!(self.get(callable), Value::Method { receiver, .. }
            if matches!(self.get(*receiver), Value::RegexPattern(_) | Value::RegexMatch(_)))
            && self.plain_regex_operand(callable);
        if args.len() > 8 || !(tuple_argument || regex_call) {
            return false;
        }
        let weak = |id| self.has_weak_values && self.weak_edge(id);
        let scalar = |id| self.finalizer_free_scalar(id) && !weak(id);
        let flat = |items: &[Id]| items.len() <= 8 && items.iter().copied().all(scalar);
        if weak(arguments)
            || !args.iter().all(|&id| match self.get(id) {
                Value::Tuple(items) => !weak(id) && flat(items),
                _ => scalar(id),
            })
        {
            return false;
        }
        let Some(dropped) = self.frames.last().unwrap().live.get(mark..) else {
            return false;
        };
        if dropped.len() > 64 {
            return false;
        }
        let mut saw_arguments = false;
        let mut would_truncate = tuple_argument;
        for &id in dropped {
            if id == arguments {
                saw_arguments = true;
                continue;
            }
            match self.get(id) {
                Value::Tuple(items) | Value::List(items) if !weak(id) && flat(items) => {}
                Value::Dict(items) if !weak(id) && items.is_empty() => {}
                _ if scalar(id) => {}
                _ if regex_call && self.plain_regex_operand(id) => would_truncate = true,
                _ => return false,
            }
        }
        // Without a nested tuple argument or a discarded regex operand, keep the original roots.
        if !saw_arguments || !would_truncate {
            return false;
        }
        self.keep_call_result(mark, result);
        true
    }

    /// A scalar and a container of scalars cannot hold an output file.
    pub(super) fn could_own_finalizer(&self, id: Id, skip_bound: bool) -> bool {
        self.could_release(id, skip_bound, |id| self.finalizer_free_scalar(id))
    }

    /// Dropping a callback result can also clear a weak reference to a native callable.
    pub(super) fn could_release_callback_result(&self, id: Id) -> bool {
        self.could_release(id, true, |id| match self.get(id) {
            Value::Native(_) => self.weak_references(id).is_empty(),
            _ => self.finalizer_free_scalar(id),
        })
    }

    fn could_release(&self, id: Id, skip_bound: bool, inert: impl Fn(Id) -> bool) -> bool {
        if inert(id) || (skip_bound && self.bound_in_active_frame(id)) {
            return false;
        }
        match self.get(id) {
            Value::List(values) | Value::Tuple(values) => values.iter().any(|&item| !inert(item)),
            Value::Dict(entries) => !entries.is_empty(),
            _ => true,
        }
    }

    /// Release an expression while preserving its result and earlier operands.
    pub(super) fn discard_operands(&mut self, mark: usize, result: Id, modules: Id) -> bool {
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
                let operand = match self.get(id) {
                    Value::List(items) if items.len() == 1 => items[0],
                    _ => id,
                };
                // The import registry already keeps this exact module and its globals alive.
                if matches!(self.get(operand), Value::Module { .. })
                    && let Value::Dict(entries) = self.get(modules)
                    && entries.iter().any(|&(_, module)| module == operand)
                {
                    return false;
                }
                self.could_own_finalizer(operand, false)
            });
        if !could_own_finalizer {
            return false;
        }
        self.keep_call_result(mark, result);
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
    pub(super) fn discard_truth_operands(&mut self, mark: usize, modules: Id) -> bool {
        let weak_release = self.has_weak_values
            && self
                .frames
                .last()
                .unwrap()
                .live
                .iter()
                .skip(mark)
                .any(|&id| self.weak_edge(id) && !self.bound_in_active_frame(id));
        let finalizer_release = self.discard_operands(mark, NONE, modules);
        self.release_temporaries_since(mark);
        weak_release || finalizer_release
    }

    /// Drop a completed call's temporary roots while preserving its returned value.
    pub(super) fn keep_call_result(&mut self, mark: usize, result: Id) {
        self.release_temporaries_since(mark);
        if result != NONE {
            self.keep(result);
        }
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

    /// Discard a never-entered frame or a compiled comprehension with no exception handlers.
    pub(super) fn discard_generator_frame(&mut self, id: Id) {
        let Value::Generator {
            offset,
            owner,
            frame,
            scope,
            iterators,
            ..
        } = self.get_mut(id)
        else {
            unreachable!("checked generator");
        };
        *offset = None;
        *owner = FrameOwner::Module(NONE);
        *frame = None;
        *scope = NONE;
        iterators.clear();
        self.disarm_generator_finalizer(id);
    }

    /// Drop the frame's ownership of completed locals without changing captured closure cells.
    pub(super) fn clear_frame(&mut self, value: Id) -> Result<()> {
        let Value::PythonFrame {
            scope,
            unoptimized_locals,
            class_proxy_scope,
            ..
        } = *self.get(value)
        else {
            return Err(Error::new("TypeError", "expected a frame"));
        };
        if scope == NONE && unoptimized_locals.is_none() {
            return Ok(());
        }
        if let Value::Scope(data) = self.get_mut(scope)
            && data.closure.cleared
            && unoptimized_locals.is_none()
        {
            // Clearing again releases extras but keeps local slots rebound after the first clear.
            self.collect_next_statement |= data.closure.extra_locals.take().is_some();
            return Ok(());
        }
        if scope != NONE && self.frames.iter().any(|frame| frame.scope == scope) {
            return Err(Error::new(
                "RuntimeError",
                "cannot clear an executing frame",
            ));
        }
        if let Some((id, offset)) = self
            .slots
            .iter()
            .enumerate()
            .find_map(|(id, slot)| match &slot.value {
                Some(Value::Generator {
                    frame: Some(frame),
                    offset: Some(offset),
                    ..
                }) if *frame == value => Some((id as Id, *offset)),
                _ => None,
            })
        {
            if offset != 0 {
                return Err(Error::new("RuntimeError", "cannot clear a suspended frame"));
            }
            // The surviving Python frame keeps its locals after closing the unstarted generator.
            self.discard_generator_frame(id);
            return Ok(());
        }
        if let Some(cells) = class_proxy_scope {
            let Value::Scope(data) = self.get_mut(cells) else {
                unreachable!()
            };
            data.closure.extra_locals = None;
            if !data.closure.cleared {
                let closure = &mut *data.closure;
                closure
                    .declared
                    .extend(closure.cells.drain().map(|(name, _)| name));
                closure.cleared = true;
            }
        }
        // Held namespaces survive a clear; subsequent access gets a fresh empty dict.
        let namespace = unoptimized_locals.map(|_| self.alloc(Value::Dict(Default::default())));
        let Value::PythonFrame {
            scope,
            unoptimized_locals,
            ..
        } = self.get_mut(value)
        else {
            unreachable!()
        };
        *scope = NONE;
        *unoptimized_locals = namespace;
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

    /// Python workers share this lifetime so a dump can see a blocked peer even before enable.
    pub(super) fn begin_fault_worker(&self) -> Arc<()> {
        let workers = self
            .fault_workers
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(()));
        self.observe_fault_workers(&workers);
        workers
    }

    pub(super) fn observe_fault_workers(&self, workers: &Arc<()>) {
        self.fault_workers.replace(Some(Arc::downgrade(workers)));
        self.publish_fault_frames(crate::fault_handler::generation());
    }

    fn publish_fault_frame(&self, index: usize) {
        let frame = &self.frames[index + 1];
        let Value::Code(code) = self.get(frame.code) else {
            unreachable!("Python frame owns its code");
        };
        let (Value::Str(filename), Value::Str(name)) =
            (self.get(code.filename), self.get(code.name))
        else {
            unreachable!("code locations and names come from source text");
        };
        let line = frame
            .location
            .as_ref()
            .map_or(code.first_line, |loc| loc.line);
        crate::fault_handler::frame(index, filename, name, line);
    }

    fn publish_fault_frames(&self, generation: u32) {
        let depth = self.python_depth();
        for index in depth.saturating_sub(crate::fault_handler::MAX_FRAMES)..depth {
            self.publish_fault_frame(index);
        }
        crate::fault_handler::truncate(depth);
        self.fault_generation.set(Some(generation));
    }

    /// A new enable period needs all callers, including those already on the stack.
    fn fault_frames_current(&mut self) -> bool {
        let generation = crate::fault_handler::generation();
        if generation == 0 {
            let Some(workers) = self.fault_workers.get_mut() else {
                return false;
            };
            if workers.strong_count() == 0 {
                *self.fault_workers.get_mut() = None;
                return false;
            }
        }
        if self.fault_generation.get() != Some(generation) {
            self.publish_fault_frames(generation);
            return false;
        }
        true
    }

    /// The first activation is a sentinel outside the Python frame stack.
    pub(super) fn python_depth(&self) -> usize {
        self.frames.len() - 1
    }

    pub fn current_code(&self) -> Id {
        self.frames.last().unwrap().code
    }

    pub fn current_scope(&self) -> Id {
        self.frames.last().unwrap().scope
    }

    /// Iterate over live Python warning locations, starting at the innermost caller.
    pub(super) fn warning_frames(&self) -> impl Iterator<Item = (&Location, Id)> {
        self.frames.iter().rev().filter_map(|frame| {
            frame
                .location
                .as_ref()
                .map(|location| (location, frame.scope))
        })
    }

    /// Python stack levels start at one and exclude the native root frame.
    pub fn warning_frame(&self, level: i32) -> Option<(Location, Id)> {
        self.warning_frames()
            .nth((level.max(1) - 1) as usize)
            .map(|(location, scope)| (location.clone(), scope))
    }

    pub(super) fn set_call_position(&mut self, position: Option<[u32; 4]>) {
        self.frames.last_mut().unwrap().call_position = position;
    }

    pub(super) fn replace_expression(
        &mut self,
        position: ExpressionPosition,
    ) -> ExpressionPosition {
        std::mem::replace(&mut self.frames.last_mut().unwrap().expression, position)
    }

    pub fn current_location(&self) -> Option<Location> {
        self.frames.last()?.location.clone()
    }

    /// Trace the same roots for collection and for a weak interning lookup between collections.
    fn mark(&self, globals: impl Iterator<Item = Id>) {
        let mut pending = MarkQueue {
            slots: &self.slots,
            traces: &self.traces,
            scalar_leaves: &self.scalar_leaves,
            next: END,
            mode: MarkMode::SkipScalarLeaves,
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
        pending.extend(self.short_bytes.values().copied());
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

    /// The singleton and small-integer prefix is permanently marked and has no children.
    fn clear_marks(&self) {
        for trace in &self.traces[FIRST_DYNAMIC_SLOT..] {
            trace.set(UNMARKED);
        }
    }

    fn reachable(&self, id: Id, roots: impl Iterator<Item = Id>) -> bool {
        self.mark(roots);
        let reachable = self.traces[id as usize].get() != UNMARKED;
        self.clear_marks();
        reachable
    }

    pub(super) fn note_buffer_view(&mut self, storage: Id) {
        self.slots[storage as usize].may_have_buffer_views = true;
    }

    pub(super) fn may_have_buffer_views(&self, storage: Id) -> bool {
        self.slots[storage as usize].may_have_buffer_views
    }

    /// A view can be unreachable before the next sweep. Check its actual lifetime
    /// before resizing a previously exported buffer, without running Python callbacks.
    pub(super) fn live_buffer_export_count(
        &mut self,
        storage: Id,
        roots: impl Iterator<Item = Id>,
    ) -> Result<usize> {
        if !self.may_have_buffer_views(storage) {
            return Ok(0);
        }
        let mut cycles = CycleSet::new(self.slots.len())?;
        self.mark_reference_survivors(roots, &mut cycles, None);
        let exports = |slot: &Slot| match slot.value {
            Some(
                Value::MemoryView {
                    owner: Some(owner), ..
                }
                | Value::StructUnpackIterator {
                    buffer: Some(owner),
                    ..
                },
            ) => {
                owner == storage
                    || matches!(self.get(owner),
                    Value::Instance { scalar: Some(backing), .. } | Value::BytesIoBuffer { storage: backing, .. }
                        if *backing == storage)
            }
            // Derived views share one acquisition of the returned memoryview.
            Some(Value::PythonBufferLease { view, .. }) => view == storage,
            _ => false,
        };
        let live = self
            .slots
            .iter()
            .zip(&self.traces)
            .filter(|(slot, trace)| trace.get() != UNMARKED && exports(slot))
            .count();
        self.clear_marks();
        self.slots[storage as usize].may_have_buffer_views = live != 0;
        Ok(live)
    }

    fn immediate_bases(&self, class: Id) -> &[Id] {
        let Value::Class {
            scope, ancestors, ..
        } = self.get(class)
        else {
            unreachable!("class registry entry")
        };
        let Value::Scope(data) = self.get(*scope) else {
            unreachable!()
        };
        let Some(ns) = &data.class_namespace else {
            // Synthetic AST classes store their single direct base first in the lineage.
            return ancestors.get(..1).unwrap_or_default();
        };
        match self.get(ns.bases) {
            Value::Tuple(bases) => bases,
            _ => &[],
        }
    }

    pub(super) fn immediate_subclasses(&self, class: Id) -> Vec<Id> {
        self.classes
            .iter()
            .copied()
            .filter(|&child| self.immediate_bases(child).contains(&class))
            .collect()
    }

    pub(super) fn reattach_class(&mut self, class: Id) {
        self.classes.retain(|&child| child != class);
        self.classes.push(class);
    }

    /// Every live descendant, including unbound local classes, with parents before children.
    pub(super) fn class_hierarchy(&self, class: Id) -> Vec<Id> {
        let mut affected = Vec::new();
        for &id in &self.classes {
            if let Value::Class { ancestors, .. } = self.get(id)
                && (id == class || ancestors.contains(&class))
            {
                affected.push((ancestors.len(), id));
            }
        }
        affected.sort_unstable();
        affected.into_iter().map(|(_, class)| class).collect()
    }

    pub(super) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn finalizer_class(&self, id: Id) -> Option<Id> {
        self.slots[id as usize].value.as_ref()?.finalizer_class()
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

    pub(super) fn is_pending_finalizer(&self, id: Id) -> bool {
        self.slots[id as usize].finalization == Finalization::Pending
    }

    pub(super) fn pending_finalizers(&self) -> &[Id] {
        &self.ordinary_finalizers
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

    /// Recheck the stored strong edges, as instance fields and generator exits can change.
    fn short_path_keeps_owner(&self, root: Id, owner: Id) -> bool {
        fn keeps(heap: &Heap, root: Id, owner: Id, budget: &mut usize) -> bool {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            if root == owner {
                return true;
            }
            match heap.get(root) {
                Value::Method { receiver, .. } => keeps(heap, *receiver, owner, budget),
                Value::Instance { fields, .. } => fields
                    .values()
                    .take(*budget)
                    .any(|&value| keeps(heap, value, owner, budget)),
                Value::Generator { protected, .. }
                    if matches!(heap.get(owner), Value::Generator { .. }) =>
                {
                    protected
                        .iter()
                        .take(*budget)
                        .flat_map(super::source_code::FinallyBlock::roots)
                        .take(*budget)
                        .any(|value| keeps(heap, value, owner, budget))
                }
                _ => false,
            }
        }

        let mut budget = 16;
        keeps(self, root, owner, &mut budget)
    }

    /// A suspended caller keeps its operands when the current frame releases temporaries.
    fn caller_keeps_generator(&self, owner: Id) -> bool {
        matches!(self.get(owner), Value::Generator { .. })
            && self.frames.iter().rev().take(24).skip(1).any(|frame| {
                // Context manager exits survive at the front of a caller's operands,
                // even when recent expression temporaries fill the other end.
                frame.live.iter().take(8).any(|id| {
                    matches!(self.get(*id), Value::Method { .. })
                        && self.short_path_keeps_owner(*id, owner)
                })
            })
    }

    /// Look for a short strong path from a current frame to an owner.
    fn frame_keeps_instance_owner(&self, owner: Id) -> bool {
        self.frames.iter().rev().take(8).any(|frame| {
            let locals = match self.get(frame.scope) {
                Value::Scope(scope) => Some(&scope.names),
                _ => None,
            };
            frame
                .live
                .iter()
                .rev()
                .take(8)
                .chain(locals.into_iter().flat_map(|names| names.values().take(8)))
                .any(|&root| self.short_path_keeps_owner(root, owner))
        }) || self.caller_keeps_generator(owner)
    }

    /// Check the active bindings at previously successful positions and trace their live stream edges.
    fn bound_finalizer_chains(&self) -> [Option<[Id; 3]>; 2] {
        self.finalizer_bindings.get().map(|position| {
            let (frame, slot) = position?;
            let Value::Scope(scope) = self.get(self.frames.get(frame)?.scope) else {
                return None;
            };
            let (_, &owner) = scope.names.get_index(slot)?;
            Some(super::stdlib::io::text_descriptor_chain(self, owner).unwrap_or([owner; 3]))
        })
    }

    /// Remember where a surviving local keeps the owner so subsequent checks can validate it directly.
    fn find_finalizer_binding(&self, owner: Id) -> bool {
        for (frame_index, frame) in self.frames.iter().enumerate().rev() {
            let Value::Scope(scope) = self.get(frame.scope) else {
                continue;
            };
            if let Some(slot) = scope.names.values().position(|&bound| bound == owner) {
                let position = Some((frame_index, slot));
                let [first, _] = self.finalizer_bindings.get();
                if first != position {
                    self.finalizer_bindings.set([position, first]);
                }
                return true;
            }
            if let Some(namespace) = scope.namespace
                && matches!(self.get(namespace), Value::Dict(entries)
                    if entries.iter().any(|&(_, bound)| bound == owner))
            {
                return true;
            }
        }
        false
    }

    /// Check each registration against the surviving frame bindings and native stream edges.
    fn frame_keeps_stream_finalizers(&self, stdin: Option<&[Id; 3]>, writers: &[Id]) -> bool {
        let chains = self.bound_finalizer_chains();
        self.ordinary_finalizers.iter().chain(writers).all(|owner| {
            stdin.is_some_and(|input| input.contains(owner))
                || chains.iter().flatten().any(|chain| chain.contains(owner))
                || self.find_finalizer_binding(*owner)
                || self.ordinary_finalizers.iter().any(|&stream| {
                    super::stdlib::io::text_descriptor_chain(self, stream).is_some_and(|chain| {
                        chain.contains(owner) && self.find_finalizer_binding(stream)
                    })
                })
        })
    }

    pub(super) fn acyclic_finalizer(
        &mut self,
        globals: [Id; 2],
        roots: impl Iterator<Item = Id>,
        stdin: Option<&[Id; 3]>,
    ) -> Result<Option<Id>> {
        // These owners are reachable through the current sys.__stdin__ even in a mixed set.
        let mut remaining = self.ordinary_finalizers.len();
        if let Some(chain) = stdin {
            for (index, &id) in chain.iter().enumerate() {
                if self.slots[id as usize].finalization == Finalization::Pending
                    && !chain[..index].contains(&id)
                {
                    remaining -= 1;
                }
            }
        }
        if remaining == 0 {
            return Ok(None);
        }
        if remaining <= MAX_FRAME_FINALIZERS && self.frame_keeps_stream_finalizers(stdin, &[]) {
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
        let mut owners = self
            .ordinary_finalizers
            .iter()
            .copied()
            .filter(|id| !stdin.is_some_and(|chain| chain.contains(id)));
        if (remaining == 1
            || remaining <= MAX_FRAME_FINALIZERS
                && owners
                    .clone()
                    .all(|id| matches!(self.get(id), Value::Generator { .. })))
            && owners.all(|id| self.frame_keeps_instance_owner(id))
        {
            return Ok(None);
        }
        if remaining > 1
            && (self.cached_finalizers_kept(globals, stdin, false)
                || (self.learn_finalizer_paths(globals, stdin)
                    && self.cached_finalizers_kept(globals, stdin, false)))
        {
            return Ok(None);
        }
        // If the roots keep every pending finalizer alive, no finalizer can run.
        // Keep the visited links until the search ends so resetting a partial
        // trace costs only the slots it touched, even after a large heap peak.
        let mut visited = END;
        let mut pending = MarkQueue {
            slots: &self.slots,
            traces: &self.traces,
            scalar_leaves: &self.scalar_leaves,
            next: END,
            mode: MarkMode::All,
        };
        self.mark_roots(roots, &mut pending);
        while remaining != 0
            && let Some(id) = pending.pop()
        {
            let slot = &self.slots[id as usize];
            self.traces[id as usize].set(visited);
            visited = id as usize;
            if slot.finalization == Finalization::Pending
                && !stdin.is_some_and(|chain| chain.contains(&id))
            {
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
            self.traces[id as usize].set(UNMARKED);
        }
        while visited != END {
            visited = self.traces[visited].replace(UNMARKED);
        }
        next
    }

    /// Track a file that must flush or close its descriptor when its last owner disappears.
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

    /// A global input stream and a frame-owned stream can keep all registered owners alive.
    pub(super) fn finalizers_kept_by(&self, input: &[Id; 3]) -> bool {
        self.file_writers
            .iter()
            .chain(&self.ordinary_finalizers)
            .all(|owner| input.contains(owner) || self.caller_keeps_generator(*owner))
            || self.frame_keeps_stream_finalizers(Some(input), &self.file_writers)
    }

    /// Both the weak reference and its target must keep a strong path from a surviving root.
    pub(super) fn weak_values_kept_by(&self, modules: Id, reference: Id) -> bool {
        if self.cached_weak_roots([modules, reference]) {
            return true;
        }
        if self.weak.len() != 1 {
            return self.weak.is_empty();
        }
        let Value::Weak(weak) = self.get(reference) else {
            return false;
        };
        weak.target.is_some_and(|target| {
            self.weak_references(target) == [reference]
                && self
                    .dict_entries(modules)
                    .is_some_and(|entries| entries.iter().any(|&(_, module)| module == target))
        })
    }

    pub(super) fn tracks_finalizers(&self) -> bool {
        !self.file_writers.is_empty() || self.has_ordinary_finalizers()
    }

    /// Count the registrations a root set must cover to avoid a finalizer sweep.
    pub(super) fn tracked_finalizer_count(&self) -> usize {
        self.file_writers.len() + self.ordinary_finalizers.len()
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

    #[cfg(test)]
    pub fn statement(
        &mut self,
        globals: impl Iterator<Item = Id>,
        live: &[Id],
        location: Location,
        caught: Id,
    ) -> Option<WeakCallbacks> {
        self.publish_statement(live, location, caught);
        if self.take_sweep_request() {
            self.collect(globals)
        } else {
            None
        }
    }

    pub(super) fn publish_statement(&mut self, live: &[Id], location: Location, caught: Id) {
        let active = self.caught_exception(caught);
        let frame = self.frames.last_mut().unwrap();
        frame.active = active;
        let line = location.line;
        frame.location = Some(location);
        frame.expression = ExpressionPosition::Statement;
        frame.live.clear();
        frame.live.extend(live);
        self.set_statement_line(line);
    }

    /// Update the current source line without crossing a statement boundary.
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
        if self.fault_frames_current()
            && let Some(index) = self.python_depth().checked_sub(1)
        {
            crate::fault_handler::line(index, line);
        }
    }

    /// A discarded owner or the allocation threshold has requested a sweep.
    pub(super) fn collection_requested(&self) -> bool {
        self.collect_next_statement || self.watermark_due() || self.gc_schedule.requested()
    }

    /// Consume a pending heap sweep before assembling its root iterators.
    pub(super) fn take_sweep_request(&mut self) -> bool {
        std::mem::take(&mut self.collect_next_statement) || self.watermark_due()
    }

    /// Collect early when a streaming consumer cannot reserve its next heap slot.
    pub(super) fn reserve_slot_or_request_collection(&mut self) {
        if self.slots.len() < self.next_collection
            && self.free_head == END
            && self.reserve_slot().is_err()
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
        if self.free_head == END && (self.slots.len() >= limit || self.reserve_slot().is_err()) {
            self.next_collection = self.slots.len();
            return true;
        }
        false
    }

    fn watermark_due(&self) -> bool {
        self.slots.len() >= self.next_collection
            && self.free_head == END
            && !self.full_collection_active()
    }

    /// A prompt collection models a reference decrement. Preserve cycles until
    /// an automatic or explicit full collection, including cycles created in callbacks.
    pub(super) fn collect(&mut self, globals: impl Iterator<Item = Id>) -> Option<WeakCallbacks> {
        self.mark_reference_roots(globals, None);
        let small = self.small_unreachable();
        let size = small.as_ref().map_or(self.slots.len(), |slots| slots.len());
        let Ok(mut cycles) = CycleSet::new(size) else {
            self.clear_marks();
            self.collect_next_statement = true;
            return None;
        };
        if let Some(slots) = small {
            self.mark_cycles(
                &mut cycles,
                |index| slots[index],
                |slot| {
                    slots
                        .binary_search(&slot)
                        .expect("unmarked child is a candidate")
                },
            );
        } else {
            self.mark_cycles(&mut cycles, std::convert::identity, std::convert::identity);
        }
        self.sweep()
    }

    /// Keep cycle scratch proportional to the garbage when most occupied slots are reachable.
    fn small_unreachable(&self) -> Option<smallvec::SmallVec<[usize; 256]>> {
        let mut slots = smallvec::SmallVec::new();
        for (id, trace) in self.traces.iter().enumerate() {
            if trace.get() == UNMARKED && self.slots[id].value.is_some() {
                if slots.len() == 256 {
                    return None;
                }
                slots.push(id);
            }
        }
        Some(slots)
    }

    fn mark_reference_roots(&self, globals: impl Iterator<Item = Id>, releasing: Option<Id>) {
        // A weakref callback can drop an unrelated owner. Keep its graph intact
        // for Python dispatch when this sweep releases temporary callback roots.
        self.mark(
            globals.chain(
                self.ordinary_finalizers
                    .iter()
                    .copied()
                    .filter(|id| Some(*id) != releasing),
            ),
        );
    }

    fn mark_reference_survivors(
        &self,
        globals: impl Iterator<Item = Id>,
        cycles: &mut CycleSet,
        releasing: Option<Id>,
    ) {
        self.mark_reference_roots(globals, releasing);
        self.mark_cycles(cycles, std::convert::identity, std::convert::identity);
    }

    fn mark_cycles(
        &self,
        cycles: &mut CycleSet,
        slot_at: impl Fn(usize) -> usize,
        index_of: impl Fn(usize) -> usize,
    ) {
        self.identify_indexed_cycles(cycles, &slot_at, index_of);
        // Peeling leaves both cycles and their unreachable descendants. Every
        // strong child of a member is therefore already live or itself a member.
        for index in cycles.members() {
            self.traces[slot_at(index as usize)].set(END);
        }
    }

    /// Test a single just-released strong edge. Other pending finalizers, roots and
    /// uncollected cycles can still retain the child or return it to Python.
    pub(super) fn released_last_finalizer_ref(
        &self,
        child: Id,
        roots: impl Iterator<Item = Id>,
    ) -> Result<bool> {
        if self.slots[child as usize].finalization != Finalization::Pending {
            return Ok(false);
        }
        let mut cycles = CycleSet::new(self.slots.len())?;
        self.mark_reference_survivors(roots, &mut cycles, Some(child));
        let last = self.traces[child as usize].get() == UNMARKED;
        self.clear_marks();
        Ok(last)
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
            traces: &self.traces,
            scalar_leaves: &self.scalar_leaves,
            next: END,
            mode: MarkMode::All,
        };
        value.children(&mut children);
        while let Some(id) = children.pop() {
            self.traces[id as usize].set(UNMARKED);
            visit(id as usize);
        }
    }

    /// CPython's full collection visits survivors in their existing GC order.
    /// A child found after its position was passed moves to the end. In
    /// particular, a tuple reachable only through its parent can remain
    /// tracked for one more collection than the same tuple held in a local.
    fn mark_full(
        &mut self,
        globals: impl Iterator<Item = Id>,
        passed: &mut Vec<usize>,
        generation: u8,
        newly_reachable: Option<&[usize]>,
    ) {
        let mut pending = MarkQueue {
            slots: &self.slots,
            traces: &self.traces,
            scalar_leaves: &self.scalar_leaves,
            next: END,
            mode: MarkMode::All,
        };
        self.mark_roots(globals, &mut pending);
        while pending.pop().is_some() {}
        let mut scan = std::mem::replace(&mut self.gc_order, GcOrder::EMPTY);
        while let Some(id) = scan.pop(&self.slots) {
            let slot = &self.slots[id];
            if self.traces[id].get() == UNMARKED {
                passed.push(id);
                continue;
            }
            // Restore previously passed children in tp_traverse order. A LIFO
            // mark queue would reverse dictionary values and reorder aliases.
            self.get(id as Id).children(&mut MarkQueue {
                slots: &self.slots,
                traces: &self.traces,
                scalar_leaves: &self.scalar_leaves,
                next: END,
                mode: MarkMode::Ordered(&mut scan),
            });
            if slot.generation <= generation
                && newly_reachable.is_none_or(|cohort| cohort[id] != 0)
                && !slot.untracked_tuple.get()
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
            if self.traces[id].get() == UNMARKED {
                self.gc_order.push(id, &self.slots);
            }
        }
    }

    /// A tracing sweep also releases ordinary expression temporaries. Exclude the
    /// dead objects that refcounting alone would release from the Python GC count.
    /// The incoming counts must start at zero.
    fn identify_cycles(&self, cycles: &mut CycleSet) -> Option<Id> {
        self.identify_indexed_cycles(cycles, std::convert::identity, std::convert::identity)
    }

    fn identify_indexed_cycles(
        &self,
        cycles: &mut CycleSet,
        slot_at: impl Fn(usize) -> usize,
        index_of: impl Fn(usize) -> usize,
    ) -> Option<Id> {
        let mut first_finalizer = None;
        let CycleSet {
            incoming,
            work: unowned,
            ..
        } = cycles;
        unowned.clear();
        for index in 0..incoming.len() {
            let id = slot_at(index);
            if self.traces[id].get() == UNMARKED
                && let Some(value) = &self.slots[id].value
            {
                // Count the candidate itself so that zero denotes a vacant or reachable slot.
                incoming[index] += 1;
                self.unreachable_children(value, |child| incoming[index_of(child)] += 1);
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
        while let Some(index) = unowned.pop() {
            let id = slot_at(index);
            if first_finalizer.is_none() && self.slots[id].finalization == Finalization::Pending {
                first_finalizer = Some(id as Id);
            }
            self.unreachable_children(self.slots[id].value.as_ref().unwrap(), |child| {
                let child = index_of(child);
                incoming[child] -= 1;
                if incoming[child] == 0 {
                    unowned.push(child);
                }
            });
        }
        first_finalizer
    }

    /// Age participating slots before Python callbacks can allocate into a recycled slot.
    fn promote_generation(&mut self, generation: u8) {
        let next = (generation + 1).min(2);
        for slot in &mut self.slots {
            if slot.generation <= generation {
                slot.generation = next;
            }
        }
    }

    /// Clear weakrefs to the initial cyclic isolate before running Python callbacks.
    pub(super) fn begin_collection(
        &mut self,
        globals: impl Iterator<Item = Id>,
        generation: u8,
    ) -> Result<WeakCallbacks> {
        let mut cycles = CycleSet::new(self.slots.len())?;
        self.gc_schedule.start_collection(generation);
        self.mark_full(globals, &mut cycles.work, generation, None);
        self.identify_cycles(&mut cycles);
        if generation < 2 {
            // A cycle outside the collected generations can own younger objects. An
            // unreachable acyclic owner would already have been released by refcounting.
            let mut found_older = false;
            for id in cycles.members() {
                let index = id as usize;
                let slot = &self.slots[index];
                if slot.generation > generation && slot.gc_tracked(self) {
                    self.traces[index].set(END);
                    found_older = true;
                }
            }
            if found_older {
                // Younger tuples held only by these older objects can now be untracked.
                // Tuples reached from ordinary roots were already considered once.
                self.mark_full(
                    std::iter::empty(),
                    &mut cycles.work,
                    generation,
                    Some(&cycles.incoming),
                );
                for (count, trace) in cycles.incoming.iter_mut().zip(&self.traces) {
                    if trace.get() != UNMARKED {
                        *count = 0;
                    }
                }
            }
        }
        if generation != 0 {
            let survivors = self
                .slots
                .iter()
                .zip(&self.traces)
                .filter(|(slot, trace)| {
                    trace.get() != UNMARKED
                        && slot.generation <= generation
                        && slot.has_gc_header
                        && slot.gc_tracked(self)
                })
                .count();
            self.gc_schedule.record_survivors(generation, survivors);
        }
        self.promote_generation(generation);
        let mut callbacks = self.clear_weak_targets(Some(&cycles.incoming), None);
        cycles.cyclic_callbacks = callbacks.len();
        self.mark(cycles.members());
        if let Some(acyclic) = self.sweep() {
            callbacks.extend(acyclic);
        }
        self.full_collection = Some(cycles);
        Ok(callbacks)
    }

    /// Expire one native deallocator's weakrefs before it releases any children.
    /// The caller roots the owner, including weakrefs stored only in its own dict.
    pub(super) fn clear_dealloc_weakrefs(
        &mut self,
        owner: Id,
        roots: impl Iterator<Item = Id>,
    ) -> Result<WeakCallbacks> {
        self.clear_owner_weakrefs(owner, roots, true)
    }

    /// After a strong edge is released, expire this target only if it stayed unreachable.
    /// A finalizer can resurrect it, and an uncollected cycle can still own it.
    pub(super) fn clear_released_weakrefs(
        &mut self,
        owner: Id,
        roots: impl Iterator<Item = Id>,
    ) -> Result<WeakCallbacks> {
        self.clear_owner_weakrefs(owner, roots, false)
    }

    fn clear_owner_weakrefs(
        &mut self,
        owner: Id,
        roots: impl Iterator<Item = Id>,
        deallocating: bool,
    ) -> Result<WeakCallbacks> {
        if !self.weak.contains_key(&owner) {
            return Ok(Vec::new());
        }
        let mut cycles = CycleSet::new(self.slots.len())?;
        self.mark_reference_survivors(roots, &mut cycles, None);
        let reachable = self.traces[owner as usize].get() != UNMARKED;
        let callbacks = if deallocating || !reachable {
            if !reachable {
                // Weakrefs in the owner's dictionary are still live during its deallocation.
                self.mark(std::iter::once(owner));
            }
            self.clear_weak_targets(None, Some(owner))
        } else {
            Vec::new()
        };
        self.clear_marks();
        Ok(callbacks)
    }

    fn clear_weak_targets(
        &mut self,
        cyclic: Option<&[usize]>,
        dealloc: Option<Id>,
    ) -> WeakCallbacks {
        // Clear non-owning handles before any dead target slot can be reused. Dead wrappers
        // never receive callbacks, and every live wrapper is cleared before any callback runs.
        let mut callbacks = Vec::new();
        let live = &mut self.frames.last_mut().unwrap().live;
        self.weak.retain(|&target, refs| {
            if dealloc.is_some_and(|owner| owner != target) {
                return true;
            }
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
                refs.retain(|&id| self.traces[id as usize].get() != UNMARKED);
                if dealloc.is_none() && self.traces[target as usize].get() != UNMARKED {
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
                let reachable = self.traces[id as usize].get() != UNMARKED;
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
        cycles.incoming.fill(0);
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

    pub(super) fn has_pending_callback_write(&self) -> bool {
        self.discarded_shutdown_write == Some(true)
            || self
                .slots
                .iter()
                .filter_map(|slot| slot.value.as_ref())
                .any(|value| super::stdlib::io::pending_callback_write(self, value))
    }

    /// Revisit buffer sizes only when the buffers themselves requested a sweep.
    #[cold]
    fn retained_byte_allowance(&self) -> usize {
        self.slots
            .iter()
            .filter_map(|slot| match &slot.value {
                Some(Value::Bytes(bytes)) => Some(bytes.capacity()),
                _ => None,
            })
            .fold(0, usize::saturating_add)
            .max(BYTE_BUFFER_FLOOR)
    }

    fn sweep(&mut self) -> Option<WeakCallbacks> {
        // Finalizers have had their chance to flush or resurrect the writer. Check its
        // actual buffer before recycling any slot, including its raw stream or class.
        if self.discarded_shutdown_write == Some(false)
            && self.slots.iter().zip(&self.traces).any(|(slot, mark)| {
                mark.get() == UNMARKED
                    && slot
                        .value
                        .as_ref()
                        .is_some_and(|value| super::stdlib::io::pending_callback_write(self, value))
            })
        {
            self.discarded_shutdown_write = Some(true);
        }
        self.default_stderr = self
            .default_stderr
            .filter(|&id| self.traces[id as usize].get() != UNMARKED);
        for id in &mut self.borrowed_hooks {
            if self.traces[*id as usize].get() == UNMARKED {
                *id = NONE;
            }
        }
        self.ordinary_finalizers
            .retain(|&id| self.traces[id as usize].get() != UNMARKED);
        self.classes
            .retain(|&id| self.traces[id as usize].get() != UNMARKED);
        self.file_writers.retain(|&id| {
            let slot = &self.slots[id as usize];
            self.traces[id as usize].get() != UNMARKED
                && match &slot.value {
                    Some(Value::TextFile(file)) => !file.closed(),
                    Some(Value::File(file)) => !file.closed(),
                    _ => false,
                }
        });
        self.interned
            .retain(|_, entry| self.traces[entry.id as usize].get() != UNMARKED);
        let callbacks = self.clear_weak_targets(None, None);
        self.abc_registrations.retain(|&[owner, member]| {
            self.traces[owner as usize].get() != UNMARKED
                && self.traces[member as usize].get() != UNMARKED
        });
        self.abc_subclass_cache.retain(|&[owner, member], _| {
            self.traces[owner as usize].get() != UNMARKED
                && self.traces[member as usize].get() != UNMARKED
        });
        self.allocation_traces
            .retain_live(|id| self.traces[id as usize].get() != UNMARKED);
        // Vacant slots supply their own free-list links. A sweep can therefore
        // retire the entire unreachable graph even if the allocator refuses growth.
        for id in FIRST_DYNAMIC_SLOT..self.slots.len() {
            let slot = &self.slots[id];
            if self.traces[id].replace(UNMARKED) == UNMARKED && slot.value.is_some() {
                self.gc_order.remove(id, &self.slots);
                let slot = &mut self.slots[id];
                if slot.has_gc_header {
                    self.gc_schedule.freed();
                }
                slot.value = None;
                self.immutable_hashes.remove(&(id as Id));
                slot.gc_next.set(self.free_head);
                self.free_head = id;
                self.free_count += 1;
            }
        }
        self.byte_buffer_allowance = if self.byte_buffer_allowance == 0 {
            self.retained_byte_allowance()
        } else {
            BYTE_BUFFER_FLOOR
        };
        // A refused growth request can lower the usual collection floor.
        let floor = self.next_collection.min(FIRST_DYNAMIC_SLOT + 4096);
        let live = self.slots.len() - FIRST_DYNAMIC_SLOT - self.free_count;
        self.next_collection = FIRST_DYNAMIC_SLOT
            .saturating_add(live.saturating_mul(2))
            .max(floor);
        (!callbacks.is_empty()).then_some(callbacks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_integer_identity_survives_collection() {
        let mut heap = Heap::new();
        let values = SMALL_INT_MIN..=SMALL_INT_MAX;
        let ids: Vec<_> = values
            .clone()
            .map(|n| heap.alloc(Value::Int(crate::Int::from(n))))
            .collect();
        heap.collect(std::iter::empty());
        for (n, id) in values.zip(ids) {
            assert!(matches!(heap.get(id), Value::Int(value) if value.as_small() == Some(n)));
            assert_eq!(heap.alloc(Value::Int(crate::Int::from(n))), id);
        }
    }

    #[test]
    fn byte_buffer_pressure_tracks_retained_capacity() {
        let mut heap = Heap::new();
        let mut buffer = Vec::with_capacity(BYTE_BUFFER_FLOOR * 2);
        buffer.extend_from_slice(b"live");
        let capacity = buffer.capacity();
        let live = heap.alloc(Value::Bytes(buffer));
        assert!(heap.collection_requested());
        heap.frames.last_mut().unwrap().live.clear();
        assert!(heap.take_sweep_request());
        heap.collect([live].into_iter());
        assert_eq!(heap.byte_buffer_allowance, capacity);

        let mut buffer = Vec::with_capacity(capacity);
        buffer.extend_from_slice(b"dead");
        let dead = heap.bytes_from_data(buffer);
        heap.frames.last_mut().unwrap().live.clear();
        assert!(heap.take_sweep_request());
        heap.collect([live].into_iter());
        assert!(heap.slots[dead as usize].value.is_none());
        assert!(matches!(heap.get(live), Value::Bytes(bytes) if bytes == b"live"));
        assert_eq!(heap.byte_buffer_allowance, capacity);
    }

    #[test]
    fn reusing_an_object_slot_replaces_its_allocation_trace() {
        let mut heap = Heap::new();
        heap.allocation_traces.start(1);
        let mut location = Location {
            file: "test.py".into(),
            line: 1,
            span: None,
        };
        heap.publish_statement(&[], location.clone(), NONE);
        let first = heap.alloc(Value::List(Vec::new()));
        assert_eq!(heap.allocation_traces.get(first).unwrap()[0].1, 1);
        location.line = 2;
        heap.publish_statement(&[], location, NONE);
        heap.collect(std::iter::empty());
        assert!(heap.allocation_traces.get(first).is_none());
        let next = heap.alloc(Value::List(Vec::new()));
        assert_eq!(next, first);
        assert_eq!(heap.allocation_traces.get(next).unwrap()[0].1, 2);
    }

    #[test]
    fn tracing_a_replaced_scalar_visits_its_new_children() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let parent = heap.alloc_slot(Value::Float(1.0));
        let child = heap.alloc_slot(Value::List(Vec::new()));
        *heap.get_mut(parent) = Value::List(vec![child]);
        heap.release_temporaries_since(mark);

        heap.collect(std::iter::once(parent));
        assert!(heap.slots[child as usize].value.is_some());
        heap.collect(std::iter::empty());
        assert!(heap.slots[child as usize].value.is_none());
    }

    #[test]
    fn removing_heap_slots_preserves_collection_order() {
        let original = [4, 2, 0, 3, 1, 5];
        for mask in 0u8..(1 << original.len()) {
            let slots = original.map(|_| Slot::new(Value::None));
            let mut order = GcOrder::EMPTY;
            for id in original {
                order.push(id, &slots);
            }
            // Sweeping retires slots by index, independent of collection order.
            for id in 0..slots.len() {
                if mask & (1 << id) == 0 {
                    order.remove(id, &slots);
                }
            }
            let kept: Vec<_> = original
                .into_iter()
                .filter(|&id| mask & (1 << id) != 0)
                .collect();
            assert_eq!(order.tail, kept.last().copied().unwrap_or(END));
            let remaining: Vec<_> = std::iter::from_fn(|| order.pop(&slots)).collect();
            assert_eq!(remaining, kept);
            assert!(
                slots.iter().all(|slot| {
                    slot.gc_prev.get() == UNMARKED && slot.gc_next.get() == UNMARKED
                })
            );
        }
    }

    #[test]
    fn hash_state_pressure_depends_on_the_python_allocation() {
        use super::super::stdlib::hashlib::{digest::SharedHash, md5};
        use crate::objects::HashState;
        let heap = Heap::new();
        for (algorithm, class, expected) in [
            ("sha256", None, false),
            ("sha256", Some(HashState::OPENSSL_CLASS), false),
            ("md5", Some(md5::CLASS), false), // Its separate Python instance owns the header.
            ("sha3_256", Some("_sha3.sha3_256"), true),
        ] {
            let provider = HashState::provider(algorithm)
                .unwrap_or_else(|_| panic!("missing hash provider for {algorithm}"));
            let hash = Value::Hash(SharedHash::new(provider.create(class, &[])));
            assert_eq!(hash.has_gc_header(&heap), expected);
        }
    }

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
    fn scalar_call_releases_peers_and_preserves_uncertain_owners() {
        let mut heap = Heap::new();
        let owner = heap.alloc(Value::List(Vec::new()));
        heap.register_finalizer(owner);
        let int_type = heap.alloc(Value::Native("int".into()));
        let types = heap.alloc(Value::Tuple(vec![int_type]));
        let owning = heap.alloc(Value::Tuple(vec![owner]));
        let arguments = heap.alloc(Value::List(vec![TRUE, types]));
        let mark = heap.temporary_mark();
        assert!(!heap.discard_scalar_call(mark, TRUE, arguments, NONE));
        heap.keep(arguments);
        heap.keep(owning);
        assert!(!heap.discard_scalar_call(mark, TRUE, arguments, NONE));
        heap.release_temporaries_since(mark);
        heap.keep(arguments);
        heap.keep(types);
        assert!(heap.discard_scalar_call(mark, TRUE, arguments, NONE));
        assert_eq!(&heap.frames.last().unwrap().live[mark..], &[TRUE]);

        let Value::List(items) = heap.get_mut(arguments) else {
            unreachable!()
        };
        items[1] = owning;
        heap.keep(arguments);
        assert!(!heap.discard_scalar_call(mark, NONE, arguments, NONE));
    }

    #[test]
    fn regex_call_releases_only_plain_unobserved_operands() {
        use super::super::{ffi, stdlib, weakref};

        let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let bytes = ctx.alloc(Value::Bytes(b"a".into()));
        let mutable = ctx.alloc(Value::ByteArray(b"a".into()));
        let pattern = stdlib::call(&mut ctx, "re.compile", &[bytes], &[]).unwrap();
        let method = ctx.lookup_attr(pattern, "match").unwrap();
        let matched = ctx.call(method, vec![bytes], Vec::new()).unwrap();
        let mutable_match = ctx.call(method, vec![mutable], Vec::new()).unwrap();
        let owner = ctx.alloc(Value::List(Vec::new()));
        ctx.heap.register_finalizer(owner);

        for (subject, result, observe_method, release) in [
            (bytes, matched, false, true),
            (mutable, mutable_match, false, false),
            (bytes, matched, true, false),
        ] {
            assert!(matches!(ctx.heap.get(result), Value::RegexMatch(_)));
            if observe_method {
                ctx.heap
                    .new_weak_reference(method, NONE, weakref::Kind::Reference, None)
                    .unwrap();
            }
            let mark = ctx.heap.temporary_mark();
            let args = ctx.alloc(Value::List(vec![subject]));
            ctx.heap.keep(result);
            assert_eq!(
                ctx.heap.discard_scalar_call(mark, result, args, method),
                release
            );
            let live = &ctx.heap.frames.last().unwrap().live[mark..];
            if release {
                assert_eq!(live, &[result]);
            } else {
                assert_eq!(live, &[args, result]);
            }
            ctx.heap.release_temporaries_since(mark);
        }
    }

    #[test]
    fn releasing_a_cached_module_cannot_finalize_its_globals() {
        let mut heap = Heap::new();
        let owner = heap.alloc(Value::List(Vec::new()));
        heap.register_finalizer(owner);
        let module = heap.alloc(Value::Module {
            name: "sample".into(),
            scope: owner,
            origin: None,
            initializing: false,
            module_state: None,
            constructor: None,
        });
        let modules = heap.alloc(Value::Dict(vec![(NONE, module)].into()));
        let mark = heap.temporary_mark();
        heap.keep(module);
        assert!(!heap.discard_operands(mark, NONE, modules));
        *heap.get_mut(modules) = Value::Dict(Default::default());
        assert!(heap.discard_operands(mark, NONE, modules));
    }

    #[test]
    fn every_finalizer_must_survive_the_released_operands() {
        use super::super::{ffi, stdlib};

        let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let scope = ctx.scope("__main__", None);
        ctx.heap.frames.last_mut().unwrap().scope = scope;
        let owner = ctx.alloc(Value::List(Vec::new()));
        let writer = ctx.alloc(Value::List(Vec::new()));
        ctx.heap.register_finalizer(owner);
        let input = stdlib::io::rooted_input_chain(&ctx);

        let Value::Scope(bindings) = ctx.heap.get_mut(scope) else {
            unreachable!()
        };
        bindings.names.insert("owner".into(), owner);
        bindings.names.insert("writer".into(), writer);
        assert!(
            ctx.heap
                .frame_keeps_stream_finalizers(input.as_ref(), &[writer])
        );

        let Value::Scope(bindings) = ctx.heap.get_mut(scope) else {
            unreachable!()
        };
        bindings.names.shift_remove("writer");
        bindings.names.insert("replacement".into(), owner);
        assert!(
            !ctx.heap
                .frame_keeps_stream_finalizers(input.as_ref(), &[writer])
        );

        let Value::Scope(bindings) = ctx.heap.get_mut(scope) else {
            unreachable!()
        };
        bindings.names.insert("renamed".into(), writer);
        assert!(
            ctx.heap
                .frame_keeps_stream_finalizers(input.as_ref(), &[writer])
        );
        let replacement = ctx.scope("replacement", None);
        ctx.heap.frames.last_mut().unwrap().scope = replacement;
        assert!(
            !ctx.heap
                .frame_keeps_stream_finalizers(input.as_ref(), &[writer])
        );
    }

    #[test]
    fn released_child_stays_live_through_external_owners_and_cycles() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let child = heap.alloc(Value::List(Vec::new()));
        let owner = heap.alloc(Value::List(vec![child]));
        heap.register_finalizer(child);
        heap.release_temporaries_since(mark);
        assert!(
            !heap
                .released_last_finalizer_ref(child, [owner].into_iter())
                .unwrap()
        );
        heap.register_finalizer(owner);
        assert!(
            !heap
                .released_last_finalizer_ref(child, [].into_iter())
                .unwrap()
        );
        heap.take_finalizer(owner, false);
        *heap.get_mut(owner) = Value::List(vec![owner, child]);
        assert!(
            !heap
                .released_last_finalizer_ref(child, [].into_iter())
                .unwrap()
        );
        *heap.get_mut(owner) = Value::List(Vec::new());
        assert!(
            heap.released_last_finalizer_ref(child, [].into_iter())
                .unwrap()
        );
    }

    #[test]
    fn prompt_sweep_preserves_cycle_descendants_and_pending_owners() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let scalar = heap.alloc(Value::Str("leaf".into()));
        let leaf = heap.alloc(Value::List(vec![scalar]));
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
        for id in [scalar, leaf, cycle, child, owner] {
            assert!(heap.slots[id as usize].value.is_some());
        }
        assert!(heap.slots[discarded as usize].value.is_none());

        heap.take_finalizer(owner, true);
        heap.collect(std::iter::empty());
        assert!(heap.slots[owner as usize].value.is_none());
        assert!(heap.slots[child as usize].value.is_none());
    }

    #[test]
    fn mixed_input_owners_leave_unreachable_finalizers_eligible() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let indirect = heap.alloc(Value::List(Vec::new()));
        let buffer = heap.alloc(Value::List(vec![indirect]));
        let input = heap.alloc(Value::List(vec![buffer]));
        let dead = heap.alloc(Value::List(Vec::new()));
        for owner in [input, indirect, dead] {
            heap.register_finalizer(owner);
        }
        heap.release_temporaries_since(mark);

        // Only one distinct pending owner is proven by this chain.
        let chain = [input, buffer, input];
        let mut check = |roots| {
            heap.acyclic_finalizer([NONE; 2], roots, Some(&chain))
                .unwrap()
        };
        assert_eq!(check(vec![input].into_iter()), Some(dead));
        assert_eq!(check(vec![input, dead].into_iter()), None);
        assert_eq!(check(vec![input].into_iter()), Some(dead));
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
    fn recycled_slot_starts_in_the_young_generation() {
        let mut heap = Heap::new();
        let mark = heap.temporary_mark();
        let original = heap.alloc(Value::List(Vec::new()));
        heap.begin_collection(std::iter::empty(), 2).unwrap();
        heap.finish_full_collection(std::iter::empty());
        heap.end_full_collection();
        assert_eq!(heap.slots[original as usize].generation, 2);

        heap.release_temporaries_since(mark);
        heap.collect(std::iter::empty());
        let reused = heap.alloc(Value::List(Vec::new()));
        assert_eq!(original, reused);
        assert_eq!(heap.slots[reused as usize].generation, 0);
    }

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
        // The survivor has two incoming owners before resurrection.
        items.extend([child, survivor]);
        let Value::List(items) = heap.get_mut(doomed) else {
            unreachable!()
        };
        items.push(doomed);
        heap.release_temporaries_since(mark);

        assert!(
            heap.begin_collection(std::iter::empty(), 2)
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
        heap.begin_collection(std::iter::empty(), 2).unwrap();
        assert_eq!(heap.finish_full_collection(std::iter::empty()).0, 3);
    }

    #[test]
    fn prompt_cycles_preserve_descendants_and_release_acyclic_peers_at_both_sizes() {
        for garbage in [4, 256, 257] {
            let mut heap = Heap::new();
            let mark = heap.temporary_mark();
            let dead: Vec<_> = (0..garbage)
                .map(|_| heap.alloc(Value::List(Vec::new())))
                .collect();
            let cycle = dead[0];
            let child = dead[1];
            let root = dead[2];
            let peer = dead[3];
            let Value::List(edges) = heap.get_mut(cycle) else {
                unreachable!()
            };
            edges.extend([cycle, child, child]);
            let Value::List(edges) = heap.get_mut(peer) else {
                unreachable!()
            };
            edges.extend([root, root]);
            heap.release_temporaries_since(mark);
            heap.collect(std::iter::empty());
            assert!(heap.slots[cycle as usize].value.is_some());
            assert!(heap.slots[child as usize].value.is_some());
            assert!(
                dead[2..]
                    .iter()
                    .all(|&slot| heap.slots[slot as usize].value.is_none())
            );

            // A former cycle may lose its last back edge between prompt collections.
            let Value::List(edges) = heap.get_mut(cycle) else {
                unreachable!()
            };
            edges.clear();
            heap.collect(std::iter::empty());
            assert!(heap.slots[cycle as usize].value.is_none());
            assert!(heap.slots[child as usize].value.is_none());
        }
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
