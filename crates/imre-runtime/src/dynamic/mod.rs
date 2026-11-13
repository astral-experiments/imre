//! Object operations for native Python functions with dynamically loaded modules.
//! The compiler lowers statements, branches, loops and function bodies to LLVM.

use crate::dynamic::value::SlotStorage;
pub(crate) use stdlib::hashlib::digest::State as DigestState;
pub(crate) use stdlib::provider_available;
pub(crate) use stdlib::traceback::write_raise_origins;

mod abstract_class;
mod allocation_traces;
mod array;
mod async_generator;
#[cfg(target_os = "linux")]
mod at_fork;
mod atexit;
mod attribute_name;
mod builtins;
mod calls;
mod class_annotations;
mod class_bases;
mod code_locals;
mod collection_pattern;
mod complex;
mod dataclass_init;
mod defaultdict;
mod deque;
mod dictionary;
mod displayhook;
mod enums;
mod excepthook;
mod exception_group;
mod exception_members;
mod ffi;
mod finalizers;
mod frame_locals;
mod function_fields;
mod gc_schedule;
mod generator_protocol;
mod import_meta;
mod importlib;
mod importlib_registered;
mod iterators;
mod lowlevel_threads;
mod mapping_abc;
mod memoryview;
mod method_decorator;
mod module_type;
mod namedtuple;
mod namespace;
mod namespace_init;
mod namespace_repr;
mod native_suspension;
mod object_size;
mod object_state;
mod operators;
mod os_error;
mod paramspec;
mod property;
mod range;
mod raw_read_window;
#[cfg(unix)]
mod signal_handlers;
mod slice;
mod slot_member;
mod source_code;
mod stdlib;
mod super_object;
mod templatelib;
mod thread_callback;
mod thread_handle;
mod thread_local;
mod thread_lock;
mod thread_rlock;
mod thread_worker;
mod typing;
mod union;
mod unraisable;
mod value;
mod weakref;
mod wrapper_descriptor;
mod zip;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::objects::exceptions::string_repr;
use value::{
    Code, ComprehensionKind, DictIterator, DictViewKind, Error, FALSE, FrameOwner, Heap, Id, NONE,
    Result, Scope, TRUE, Value,
};

/// A live Python object can carry the linked body supplied by its producer.
enum LinkedSource {
    Loader(SourceModule),
    Doctest(SourceModule),
    Expression(Code),
}

/// How a registered module supplies its code object to importlib.
#[derive(Clone, Copy)]
enum ModuleInitializer {
    Native(Code),
    File {
        entry: Code,
        digest: Option<&'static [u8; 32]>,
        flags: Option<u32>,
        declared_open_path: Option<&'static Path>,
    },
    Zip(Code, &'static [u8; 32], u32),
    Builtin(Code),
    Invalid {
        message: &'static str,
        position: Option<&'static [u32; 4]>,
        in_args: bool,
        parser_line: Option<&'static str>,
        archive_digest: Option<&'static [u8; 32]>,
    },
}

/// The caller roots the script module and the entered frame roots its scope and code.
struct MainModuleFrame {
    module: Id,
    entry: Code,
}

impl MainModuleFrame {
    /// # Safety
    /// The caller owns execution of the context with this frame on top, and the linked
    /// initializer remains loaded until it returns.
    unsafe fn run(self, context: *mut Context) -> Result<Id> {
        // SAFETY: the caller keeps the initializer loaded and the prepared frame active.
        let result = unsafe { Context::run_code_frame(context, self.entry, true) };
        // SAFETY: the module initializer has returned to the owner of the live Context.
        unsafe { &mut *context }.finish_initialization(self.module);
        result.map(|_| self.module)
    }
}

#[derive(Clone)]
struct SourceModule {
    path: PathBuf,
    code: ModuleInitializer,
    bundled_text: Option<&'static [u8]>,
    single_inputs: Option<Rc<Vec<SingleInput>>>,
}

#[derive(Clone, Copy)]
struct SingleInput {
    filename: &'static str,
    source: &'static [u8],
    flags: u32,
    code: Code,
}

impl SourceModule {
    /// Only directory-backed registrations can supply a SourceFileLoader.
    fn in_file_finder(&self) -> bool {
        !matches!(
            self.code,
            ModuleInitializer::Builtin(_)
                | ModuleInitializer::Zip(..)
                | ModuleInitializer::Invalid {
                    archive_digest: Some(_),
                    ..
                }
        )
    }
}

enum IteratorSlice<'a> {
    Values(&'a [Id]),
    Array(&'a array::Array, usize),
    Reversed(&'a [Id]),
    Bytes(&'a [u8]),
    ReversedBytes(&'a [u8]),
    Text(&'a str),
    ReversedText(&'a str),
    Codepoints(&'a [u32]),
    ReversedCodepoints(&'a [u32]),
}

impl IteratorSlice<'_> {
    /// Counts positions without allocating boxed integer elements.
    fn len(&self) -> usize {
        match self {
            Self::Values(values) | Self::Reversed(values) => values.len(),
            Self::Bytes(bytes) | Self::ReversedBytes(bytes) => bytes.len(),
            Self::Text(text) | Self::ReversedText(text) => text.chars().count(),
            Self::Codepoints(points) | Self::ReversedCodepoints(points) => points.len(),
            Self::Array(array, offset) => array.len().saturating_sub(*offset),
        }
    }

    /// Test the byte boundary directly so each character step stays constant time.
    fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) | Self::ReversedText(text) => text.is_empty(),
            _ => self.len() == 0,
        }
    }
}

struct ExposedClassBody {
    mapping: Id,
    annotation_cell: Option<Id>,
}

/// Native state shared by the main context and its registered workers.
#[derive(Clone, Default)]
struct InterpreterState {
    worker_count: Arc<AtomicUsize>,
    socket_default_timeout: Arc<Mutex<Option<Duration>>>,
    finalizing: Arc<AtomicBool>,
}

pub(crate) struct Context {
    interpreter: InterpreterState,
    // Active calls to an overridable socket.send from sendfile. Receivers stay rooted by the call.
    sendfile_writes: Vec<(Id, crate::objects::socket::SendfileIdentity, Id)>,
    threads: lowlevel_threads::Workers,
    workers: thread_worker::Registry,
    heap: Heap,
    // Startup finishes applying environment overrides before any context can read this config.
    config: Arc<serde_json::Value>,
    executable: PathBuf,
    interpreter_layout: Result<imre_ast::layout::InterpreterLayout>,
    helper_executable: String,
    argv: Id,
    /// Filesystem bytes of the installed startup entry. An empty entry follows the current cwd.
    startup_path0: Option<Vec<u32>>,
    /// Registered files backing the initial cwd of a relocated, file-backed named entry.
    saved_entry_root: Option<(PathBuf, Vec<u32>)>,
    default_encoding: Id,
    displaced_sni_callbacks: std::collections::HashSet<Id>,
    audit_hooks: Vec<Id>,
    fork_hooks: [Vec<Id>; 3],
    // The fatal handler borrows its descriptor while Python retains the destination.
    fault_file: Option<Id>,
    fault_user_files: HashMap<i32, Id>,
    exit_callbacks: Vec<Id>,
    // The active stack roots suspended contexts while Context.run executes native code.
    contexts: Vec<Id>,
    // A buffer export pins its bytearray's size while Python callbacks run.
    buffer_exports: Vec<Id>,
    // Container ancestors visible to a reentrant Python __repr__.
    repr_ancestors: Vec<Id>,
    repr_spare_stack: bool,
    repr_sequence_words: Option<usize>,
    // Python alias frames represented by a renderer suspended in a tuple argument callback.
    suspended_typing_frames: usize,
    token_missing: Id,
    // Emitted filename constants live for the program. Their display spelling belongs to this interpreter.
    native_filename_labels: HashMap<usize, std::sync::Arc<str>>,
    // Builtin functions stay bound to the first builtins module across reimports.
    original_builtins: Option<Id>,
    source_builtins: Option<Id>,
    zip_directories: HashMap<String, crate::objects::ZipArchive>,
    zip_path_importers: Id,
    codec_entries: stdlib::codecs::Registry,
    warning_state: Option<stdlib::warnings_accel::State>,
    frozen_override: i32,
    pending_builtin_names_scope: Option<Id>,
    // The interpreter keeps its original dictionary even when Python reassigns sys.modules.
    modules: Id,
    import_missing: Id,
    // The running frozen bootstrap keeps its globals even if its registry entry is replaced.
    bootstrap_globals: Id,
    bootstrap_module: Id,
    import_registry_installed: bool,
    bootstrap_file_spec: Id,
    // The native path finder stays installed across fresh public bootstrap imports.
    finder_external: Id,
    classes: HashMap<String, Id>,
    // The Python enum bases use native layouts but have writable class dictionaries.
    enum_scopes: HashMap<Id, Id>,
    signal_handlers: Vec<Id>,
    reporting_wakeup_errors: bool,
    typing: typing::Registry,
    builtins: HashMap<String, Id>,
    /// Last observed binding position for each name bucket, validated against the current scope.
    scope_name_slots: [std::cell::Cell<usize>; 16],
    native_type_dicts: HashMap<Id, Id>,
    sources: HashMap<String, Vec<SourceModule>>,
    call_depth: usize,
    deque_comparison_repeats: usize,
    pending: Option<Error>,
    // Fallback for targets whose native stack bounds cannot be queried.
    slot_depth: usize,
}

/// Copy a runtime name without aborting on allocation failure.
fn copy_label(value: &str) -> Result<String> {
    let mut copied = String::new();
    copied
        .try_reserve_exact(value.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    copied.push_str(value);
    Ok(copied)
}

/// Copy IDs or dictionary entry pairs while preserving a catchable allocation failure.
fn copy_ids<T: Copy>(values: &[T]) -> Result<Vec<T>> {
    let mut copied = Vec::new();
    copied
        .try_reserve_exact(values.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    copied.extend_from_slice(values);
    Ok(copied)
}

/// Reserve the entire destination before copying either sequence's object references.
fn concat_ids(left: &[Id], right: &[Id]) -> Result<Vec<Id>> {
    let mut items = Vec::new();
    items
        .try_reserve_exact(left.len() + right.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    items.extend_from_slice(left);
    items.extend_from_slice(right);
    Ok(items)
}

/// Captured locals certified by the compiler for an ordinary frame.
#[derive(Debug)]
enum CellLayout {
    Unspecified,
    Incomplete,
    Complete(Vec<String>),
}

impl CellLayout {
    fn retains_displaced(&self, name: &str, cleared: bool) -> bool {
        match self {
            Self::Unspecified => false,
            // After clear the reconstructed slots no longer contain cells.
            Self::Incomplete => cleared,
            Self::Complete(cells) => cleared || !cells.iter().any(|cell| cell == name),
        }
    }
}

/// The fallback for a declared annotation whose live Python binding cannot be resolved.
#[derive(Clone, Copy, Debug)]
struct ScalarHint(Option<&'static str>);

impl ScalarHint {
    fn new(name: &str) -> Self {
        Self(
            ["str", "bytes", "int", "bool", "float", "None"]
                .into_iter()
                .find(|&ty| ty == name),
        )
    }
}

/// Immutable parameter lists used to bind calls to the same code object.
#[derive(Debug)]
struct CallParameters {
    positional: Vec<String>,
    positional_only: usize,
    keyword_only: Vec<String>,
    locals: value::DeclaredLocals,
    cell_layout: CellLayout,
    nonlocals: Vec<String>,
    locals_complete: bool,
    class_cell: bool,
    varargs: Option<String>,
    kwargs: Option<String>,
    return_type: Option<ScalarHint>,
    types: serde_json::Value,
    integer_sign_parameters: serde_json::Value,
    annotation_references: Option<Box<serde_json::Value>>,
}

impl CallParameters {
    fn shape(&self) -> crate::source_arguments::Shape<'_, String> {
        crate::source_arguments::Shape {
            positional: &self.positional,
            positional_only: self.positional_only,
            keyword_only: &self.keyword_only,
            varargs: self.varargs.is_some(),
            kwargs: self.kwargs.is_some(),
        }
    }

    /// Decode both compiled and runtime-created function metadata once per definition.
    fn from_metadata(metadata: &mut serde_json::Value) -> Result<Self> {
        let names = |key: &str| -> Result<Vec<String>> {
            let mut result = Vec::new();
            for name in metadata[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
            {
                result
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                result.push(copy_label(name)?);
            }
            Ok(result)
        };
        let mut positional = names("posonly")?;
        let positional_only = positional.len();
        let rest = names("positional")?;
        positional
            .try_reserve(rest.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        positional.extend(rest);
        let local_names = names("scope_locals")?;
        let mut locals = std::collections::HashSet::new();
        locals
            .try_reserve(local_names.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        locals.extend(local_names);
        // Runtime-created functions can declare their parameters without a separate local list.
        for arg in code_locals::argument_slots(metadata) {
            if !locals.contains(arg) {
                locals
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                locals.insert(copy_label(arg)?);
            }
        }
        Ok(Self {
            positional,
            positional_only,
            keyword_only: names("kwonly")?,
            locals: locals.into(),
            cell_layout: match metadata.get("scope_cellvars") {
                Some(serde_json::Value::Array(_)) => CellLayout::Complete(names("scope_cellvars")?),
                Some(serde_json::Value::Null) => CellLayout::Incomplete,
                _ => CellLayout::Unspecified,
            },
            nonlocals: names("scope_nonlocals")?,
            locals_complete: metadata["scope_locals_complete"].as_bool().unwrap_or(false),
            class_cell: metadata["class_cell"] == true,
            varargs: metadata["varargs"].as_str().map(copy_label).transpose()?,
            kwargs: metadata["kwargs"].as_str().map(copy_label).transpose()?,
            return_type: metadata["return_type"].as_str().map(ScalarHint::new),
            types: metadata
                .get_mut("types")
                .map(serde_json::Value::take)
                .unwrap_or_default(),
            annotation_references: metadata
                .get_mut("annotation_references")
                .map(|references| Box::new(references.take())),
            integer_sign_parameters: metadata
                .get_mut("integer_sign_parameters")
                .map(serde_json::Value::take)
                .unwrap_or_default(),
        })
    }
}

impl value::CodeObject {
    /// Keep one decoded binding layout for all calls in this heap.
    fn call_parameters(
        &self,
        decode: impl FnOnce() -> Result<CallParameters>,
    ) -> Result<std::rc::Rc<CallParameters>> {
        if let Some(parameters) = self.parsed_parameters.get() {
            return Ok(std::rc::Rc::clone(parameters));
        }
        let parameters = decode()?;
        Ok(std::rc::Rc::clone(
            self.parsed_parameters
                .get_or_init(|| std::rc::Rc::new(parameters)),
        ))
    }
}

impl Context {
    fn code_object(&self, id: Id) -> &value::CodeObject {
        let Value::Code(code) = self.heap.get(id) else {
            unreachable!("function must retain its code object")
        };
        code
    }

    fn parsed_parameters(&self, code: Id) -> Result<std::rc::Rc<CallParameters>> {
        let code = self.code_object(code);
        code.call_parameters(|| {
            let mut metadata = serde_json::from_str(self.str_value(code.parameters)?)
                .map_err(|error| Error::new("SystemError", error.to_string()))?;
            CallParameters::from_metadata(&mut metadata)
        })
    }

    fn alloc(&mut self, value: Value) -> Id {
        let class = value.finalizer_class();
        let id = self.heap.alloc(value);
        self.register_instance_finalizer(id, class);
        id
    }
    fn text(&mut self, value: impl Into<crate::Str>) -> Id {
        self.heap.owned_text(value.into())
    }
    fn text_codepoints(&mut self, points: Vec<u32>) -> Id {
        self.alloc(Value::from_codepoints(points))
    }

    /// Read Python code points without converting a surrogate into U+FFFD.
    fn string_codepoints(&self, id: Id) -> Result<Cow<'_, [u32]>> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Str(text) => Ok(Cow::Owned(text.chars().map(u32::from).collect())),
            Value::SurrogateStr(points) => Ok(Cow::Borrowed(points)),
            _ => Err(Error::new("TypeError", "expected str")),
        }
    }
    fn int(&mut self, value: i64) -> Id {
        self.alloc(Value::Int(crate::Int::from(value)))
    }
    fn dict_storage(&self, value: Id) -> Option<Id> {
        self.heap.dict_storage(value)
    }

    /// PyMapping_Check tests the subscript slot even for sequences with no keys method.
    fn has_mapping_subscription(&self, value: Id) -> bool {
        self.slot(value, "__getitem__").is_some()
            || self.dict_storage(value).is_some()
            || self.builtin_slot_accepts(value, "tuple")
            || matches!(
                self.heap.get(self.scalar_value(value)),
                Value::List(_)
                    | Value::Str(_)
                    | Value::SurrogateStr(_)
                    | Value::Bytes(_)
                    | Value::ByteArray(_)
                    | Value::MemoryView { .. }
                    | Value::Range(_)
                    | Value::Array(_)
                    | Value::MappingProxy(_)
                    | Value::Context(_)
                    | Value::RegexMatch(_)
            )
    }

    /// The native elements inherited by a Python-defined list or tuple subclass.
    fn sequence_payload(&self, object: Id) -> Option<(Id, &[Id])> {
        let Value::Instance {
            scalar: Some(storage),
            ..
        } = self.heap.get(object)
        else {
            return None;
        };
        match self.heap.get(*storage) {
            Value::List(items) | Value::Tuple(items) => Some((*storage, items)),
            _ => None,
        }
    }

    /// Return the native mutable sequence for an inherited subscript assignment or deletion.
    fn mutable_sequence_storage(&self, object: Id) -> Option<Id> {
        let storage = self.scalar_value(object);
        (storage != object
            && matches!(self.heap.get(storage), Value::List(_) | Value::ByteArray(_)))
        .then_some(storage)
    }

    /// Slot descriptors require the native layout even when a proxy passes isinstance.
    fn builtin_slot_accepts(&self, object: Id, owner: &str) -> bool {
        if matches!(self.heap.get(object), Value::NamedTuple { class: Some(class), .. } if *class == owner)
        {
            return true;
        }
        match (owner, self.heap.get(self.scalar_value(object))) {
            #[cfg(target_os = "linux")]
            ("mmap.mmap", Value::Mmap { .. }) => true,
            ("type", _) => self.is_type_object(object),
            (_, Value::Raised(error)) if builtins::is_exception(owner) => {
                self.is_builtin_error(error, owner)
            }
            (weakref::PROXY_TYPE | weakref::CALLABLE_PROXY_TYPE, Value::Weak(wr)) => {
                wr.kind.class() == owner
            }
            (
                "builtin_function_or_method" | "method" | "method-wrapper" | "wrapper_descriptor",
                _,
            ) => self.builtin_type_name(object) == Some(owner),
            ("list", Value::List(_)) | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => {
                true
            }
            (_, Value::DictView { .. }) => {
                Self::dict_view_owner(self.type_name(object)) == Some(owner)
            }
            ("getset_descriptor", Value::GetSetDescriptor { .. } | Value::LayoutDescriptor(..)) => {
                true
            }
            ("types.GenericAlias", Value::GenericAlias(alias)) => alias.is_builtin_alias(),
            ("code", Value::Code(_))
            | ("generic_alias_iterator", Value::GenericAliasIterator(_))
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("float", Value::Float(_))
            | ("filter", Value::Filter { .. })
            | ("set_iterator", Value::SetIterator(_))
            | ("map", Value::Map { .. })
            | ("zip", Value::Zip { .. })
            | ("mappingproxy", Value::MappingProxy(_))
            | ("super", Value::Super { .. })
            | ("complex", Value::Complex(_))
            | ("str", Value::Str(_) | Value::SurrogateStr(_))
            | ("bytes", Value::Bytes(_))
            | ("bytearray", Value::ByteArray(_))
            | ("array.array", Value::Array(_))
            | ("operator.attrgetter", Value::AttrGetter(_))
            | ("operator.itemgetter", Value::ItemGetter(_))
            | ("operator.methodcaller", Value::MethodCaller { .. })
            | (stdlib::functools::PARTIAL_CLASS, Value::Partial { .. })
            | (stdlib::functools::LRU_CACHE_CLASS, Value::Cached { .. })
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | (value::REGEX_PATTERN_TYPE, Value::RegexPattern(_))
            | (value::ZSTD_COMPRESS_TYPE, Value::ZstdCompressor(_))
            | (value::ZSTD_DECOMPRESS_TYPE, Value::ZstdDecompressor(_))
            | (value::BZ2_COMPRESS_TYPE, Value::Bz2Compressor(_))
            | (value::BZ2_DECOMPRESS_TYPE, Value::Bz2Decompressor(_))
            | (value::REGEX_MATCH_TYPE, Value::RegexMatch(_))
            | (thread_handle::CLASS, Value::ThreadHandle(_))
            | (thread_local::CLASS, Value::ThreadLocal(_)) => true,
            _ => false,
        }
    }

    fn dict_entries(&self, value: Id) -> Option<&[(Id, Id)]> {
        self.heap.dict_entries(value)
    }

    fn dict(&mut self, entries: Vec<(Id, Id)>) -> Id {
        self.alloc(Value::Dict(entries.into()))
    }

    fn keyword_args(&self, dict: Id) -> Result<Vec<(String, Id)>> {
        let Value::Dict(entries) = self.heap.get(dict) else {
            return Err(Error::new("TypeError", "expected keyword dict"));
        };
        entries
            .iter()
            .map(|&(key, value)| {
                let name = self
                    .str_value(key)
                    .map_err(|_| Error::new("TypeError", "keywords must be strings"))?;
                Ok((name.to_owned(), value))
            })
            .collect()
    }

    /// The immutable text backing an exact string or a Python subclass of str.
    fn string_storage(&self, id: Id) -> Option<Id> {
        match self.heap.get(id) {
            Value::Str(_) | Value::SurrogateStr(_) => Some(id),
            Value::Instance {
                scalar: Some(value),
                ..
            } if matches!(
                self.heap.get(*value),
                Value::Str(_) | Value::SurrogateStr(_)
            ) =>
            {
                Some(*value)
            }
            _ => None,
        }
    }

    /// An unchanged Unicode result retains an exact receiver and copies a subclass.
    fn unchanged_string(&mut self, receiver: Id, storage: Id) -> Result<Id> {
        if receiver == storage {
            return Ok(receiver);
        }
        let copy = match self.heap.get(storage) {
            Value::Str(text) if text.is_empty() => return Ok(self.text("")),
            Value::Str(text) => Value::Str(
                crate::Str::try_new(text.as_str()).map_err(|_| Error::new("MemoryError", ""))?,
            ),
            Value::SurrogateStr(points) => Value::SurrogateStr(copy_ids(points)?),
            _ => unreachable!("validated str receiver"),
        };
        self.heap.fresh_string(copy)
    }

    fn memoryview_owner(owner: Option<Id>) -> Result<Id> {
        owner.ok_or_else(|| {
            Error::new(
                "ValueError",
                "operation forbidden on released memoryview object",
            )
        })
    }

    /// A native buffer descriptor in the MRO takes precedence over a later Python definition.
    fn buffer_slot(&self, object: Id, name: &str) -> Option<Option<Id>> {
        let Value::Instance { class, .. } = self.heap.get(object) else {
            return None;
        };
        self.find_in_mro(*class, |base| match self.heap.get(base) {
            Value::Class { .. } => self.own_class_attr(base, name).map(Some),
            Value::Native(base)
                if matches!(base.as_str(), "array.array" | "bytearray")
                    || (base == "bytes" && name == "__buffer__") =>
            {
                Some(None)
            }
            _ => None,
        })
    }

    fn python_buffer_method(&self, object: Id, scoped: bool) -> Result<Option<Id>> {
        self.check_view_export(object)?;
        let Some(method) = self.buffer_slot(object, "__buffer__") else {
            return Ok(None);
        };
        // A long-lived view needs a Python release hook even if its last reference dies in GC.
        if (!scoped || method.is_none())
            && self
                .buffer_slot(object, "__release_buffer__")
                .flatten()
                .is_some()
        {
            return Err(Self::unsupported_buffer_hooks());
        }
        Ok(method)
    }

    fn call_buffer_hook(&mut self, object: Id, method: Id, arg: Id) -> Result<Id> {
        let class = self.exact_type(object)?;
        let method = self.bind_attribute(method, object, class)?;
        self.call(method, vec![arg], vec![])
    }

    fn acquire_python_buffer(&mut self, object: Id, method: Id, flags: i64) -> Result<Id> {
        let flags = self.int(flags);
        let view = self.call_buffer_hook(object, method, flags)?;
        if !matches!(self.heap.get(view), Value::MemoryView { .. }) {
            return Err(Error::new(
                "TypeError",
                "__buffer__ returned non-memoryview object",
            ));
        }
        Ok(view)
    }

    // Acquire the data before converting any later argument via Python's __index__.
    // While the export is held, callbacks can modify its bytes but cannot resize it.
    fn with_read_buffer<T>(
        &mut self,
        data: Id,
        consume: impl FnOnce(&mut Self, memoryview::ReadBuffer) -> Result<T>,
    ) -> Result<T> {
        if matches!(self.heap.get(data), Value::Instance { .. }) {
            if matches!(
                self.heap.get(self.scalar_value(data)),
                Value::Bytes(_) | Value::ByteArray(_)
            ) && self.buffer_slot(data, "__buffer__") == Some(None)
            {
                let buffer = self.read_buffer_range(data)?;
                return self.with_native_bytes_export(data, |ctx, _| consume(ctx, buffer));
            }
            if let Some(method) = self.python_buffer_method(data, true)? {
                return self.with_python_read_buffer(data, method, 0, |ctx, _, buffer| {
                    consume(ctx, buffer)
                });
            }
        }
        let buffer = self.read_buffer_range(data)?;
        self.with_buffer_export(data, |ctx| consume(ctx, buffer))
    }

    /// Consume a contiguous view returned by a Python buffer provider.
    fn with_contiguous_python_buffer<T>(
        &mut self,
        object: Id,
        method: Id,
        flags: i64,
        consume: impl FnOnce(&mut Self, Id) -> Result<T>,
    ) -> Result<T> {
        self.with_python_read_buffer(object, method, flags, |ctx, view, _| consume(ctx, view))
    }

    fn with_python_read_buffer<T>(
        &mut self,
        object: Id,
        method: Id,
        flags: i64,
        consume: impl FnOnce(&mut Self, Id, memoryview::ReadBuffer) -> Result<T>,
    ) -> Result<T> {
        let view = self.acquire_python_buffer(object, method, flags)?;
        // A failed export of the returned view does not call the provider's release hook.
        let buffer = self.read_buffer_range(view)?;
        self.with_python_buffer_export(object, view, |ctx| consume(ctx, view, buffer))
    }

    /// Run the consumer of a successfully acquired Python export, then release it.
    fn with_python_buffer_export<T>(
        &mut self,
        object: Id,
        view: Id,
        consume: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let result = self.with_buffer_export(view, consume);
        if let Err(error) = &result {
            for id in error.roots() {
                self.heap.keep(id);
            }
        }
        self.release_python_buffer(object, view);
        result
    }

    /// A bytes or bytearray subclass may inherit its native get-buffer slot and
    /// override only release. The callback receives an ownerless, temporary view
    /// of the original storage while the native export still prevents resizing.
    fn with_native_bytes_export<T>(
        &mut self,
        object: Id,
        consume: impl FnOnce(&mut Self, Id) -> Result<T>,
    ) -> Result<T> {
        self.with_buffer_export(object, |ctx| {
            let result = consume(ctx, object);
            if ctx
                .buffer_slot(object, "__release_buffer__")
                .flatten()
                .is_some()
            {
                if let Err(error) = &result {
                    for id in error.roots() {
                        ctx.heap.keep(id);
                    }
                }
                ctx.notify_native_buffer_release(object);
            }
            result
        })
    }

    fn notify_native_buffer_release(&mut self, object: Id) {
        let storage = self.scalar_value(object);
        let view = self.alloc(Value::MemoryView {
            owner: Some(storage),
            lease: None,
            anonymous: true,
            restricted: true,
            layout: None,
            readonly: matches!(self.heap.get(storage), Value::Bytes(_)),
            hash: None,
        });
        self.heap.keep(view);
        self.release_python_buffer(object, view);
        // The callback can retain or explicitly release the view. Its
        // storage must be inaccessible once PyBuffer_Release returns.
        let Value::MemoryView { owner, .. } = self.heap.get_mut(view) else {
            unreachable!()
        };
        if let Some(storage) = owner.take() {
            self.heap.weak_edge_removed(storage);
        }
    }

    /// Release a synchronous Python export after its consumer has dropped the view's export.
    fn release_python_buffer(&mut self, object: Id, view: Id) {
        if matches!(self.heap.get(view), Value::MemoryView { owner: Some(owner), lease: None, .. } if *owner == object)
        {
            return;
        }
        if let Some(method) = self.buffer_slot(object, "__release_buffer__").flatten()
            && let Err(error) = self.call_buffer_hook(object, method, view)
        {
            let message = format!(
                "Exception ignored in __release_buffer__ of {}",
                self.type_name(object)
            );
            unraisable::report(self, error, false, None, &message);
        }
    }

    fn unsupported_buffer_hooks() -> Error {
        Error::new(
            "NotImplementedError",
            "Python-defined buffer hooks are unsupported",
        )
    }

    fn check_buffer_exporter(&self, object: Id) -> Result<()> {
        if self.python_buffer_method(object, false)?.is_some() {
            return Err(Self::unsupported_buffer_hooks());
        }
        Ok(())
    }

    fn view_storage_id(&self, owner: Id) -> Id {
        match self.heap.get(owner) {
            Value::BytesIoBuffer { storage, .. } => *storage,
            _ => self.scalar_value(owner),
        }
    }

    fn owner_storage(&self, owner: Id) -> Result<memoryview::Storage<'_>> {
        use memoryview::Storage;
        let bytes = match self.heap.get(self.view_storage_id(owner)) {
            Value::Bytes(bytes) | Value::ByteArray(bytes) => bytes.as_slice(),
            Value::Array(a) => a.as_bytes(),
            Value::RawReadWindow(w) => {
                let storage = self.owner_storage(w.backing)?;
                let range = w.checked_range(storage.len())?;
                return storage.slice(range);
            }
            #[cfg(target_os = "linux")]
            value @ Value::Mmap { .. } => {
                let region = stdlib::mmap::region(value)?;
                return Ok(Storage::Mapped(region, 0..region.len()));
            }
            Value::BytesIoStorage(state) => state.file()?.storage(),
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        self.type_name(owner)
                    ),
                ));
            }
        };
        Ok(Storage::Native(bytes))
    }

    fn owner_bytes(&self, owner: Id) -> Result<&[u8]> {
        self.owner_storage(owner)?.borrowed()
    }

    fn owner_byte_len(&self, owner: Id) -> Result<usize> {
        Ok(self.owner_storage(owner)?.len())
    }

    fn view_element_count(&self, owner: Id) -> Result<usize> {
        match self.heap.get(self.view_storage_id(owner)) {
            Value::Array(a) => Ok(a.len()),
            Value::BytesIoStorage(state) => Ok(state.file()?.bytes().len()),
            _ => self.owner_byte_len(owner),
        }
    }

    fn writable_owner_bytes(&mut self, owner: Id) -> Result<&mut [u8]> {
        let storage = self.view_storage_id(owner);
        if let Value::RawReadWindow(w) = *self.heap.get(storage) {
            let range = w.checked_range(self.owner_bytes(w.backing)?.len())?;
            return Ok(&mut self.writable_storage_bytes(w.backing)?[range]);
        }
        self.writable_storage_bytes(storage)
    }

    fn writable_storage_bytes(&mut self, storage: Id) -> Result<&mut [u8]> {
        Self::mutable_storage_bytes(self.heap.get_mut(storage))
    }

    fn mutable_storage_bytes(value: &mut Value) -> Result<&mut [u8]> {
        match value {
            Value::ByteArray(bytes) => Ok(bytes.as_mut_slice()),
            #[cfg(target_os = "linux")]
            Value::Mmap { .. } => Err(stdlib::mmap::unsupported_borrow()),
            Value::Array(a) => Ok(a.as_bytes_mut()?),
            Value::BytesIoStorage(state) => Ok(state.file_mut()?.storage_mut()?.as_mut_slice()),
            _ => Err(Error::new("TypeError", "cannot modify read-only memory")),
        }
    }

    fn buffer_storage(&self, id: Id) -> Result<&Value> {
        let owner = match self.heap.get(id) {
            Value::MemoryView { owner, .. } => Self::memoryview_owner(*owner)?,
            _ => id,
        };
        Ok(match self.heap.get(self.view_storage_id(owner)) {
            Value::RawReadWindow(window) => self.heap.get(window.backing),
            storage => storage,
        })
    }

    fn bytes_like(&self, id: Id) -> Result<&[u8]> {
        match self.heap.get(self.scalar_value(id)) {
            Value::MemoryView { .. } => self.view_bytes(id),
            #[cfg(target_os = "linux")]
            Value::Mmap { .. } => Err(stdlib::mmap::unsupported_borrow()),
            Value::BytesIoBuffer { storage, .. } => match self.heap.get(*storage) {
                Value::BytesIoStorage(state) => Ok(state.file()?.bytes()),
                _ => Err(Error::new("ValueError", "I/O operation on closed file.")),
            },
            Value::Bytes(bytes) | Value::ByteArray(bytes) => Ok(bytes),
            _ => {
                let name = self.type_name(id);
                let name = &name[..name.floor_char_boundary(100)];
                Err(Error::new(
                    "TypeError",
                    format!("a bytes-like object is required, not '{name}'"),
                ))
            }
        }
    }

    /// Obtain the current contents of a contiguous export at the point of consumption.
    fn read_buffer_bytes(&self, id: Id) -> Result<std::borrow::Cow<'_, [u8]>> {
        let buffer = self.read_buffer_range(id)?.storage(self)?;
        buffer.read(0..buffer.len())
    }

    fn native_buffer_bytes(&self, id: Id) -> Result<&[u8]> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Array(array) => Ok(array.as_bytes()),
            _ => self.bytes_like(id),
        }
    }

    fn invalidate_stream_snapshot(&self, owner: Id) {
        if let Value::BytesIoBuffer { storage, .. } = *self.heap.get(owner)
            && let Value::BytesIoStorage(state) = self.heap.get(storage)
        {
            state.snapshot.set(None);
        }
    }

    fn view_is_readonly(&self, view: Id) -> Result<bool> {
        let Value::MemoryView {
            owner, readonly, ..
        } = *self.heap.get(view)
        else {
            unreachable!()
        };
        let owner = Self::memoryview_owner(owner)?;
        let storage = self.heap.get(self.view_storage_id(owner));
        #[cfg(target_os = "linux")]
        if matches!(storage, Value::Mmap { .. }) {
            return Ok(readonly || stdlib::mmap::region(storage)?.readonly());
        }
        Ok(readonly || matches!(storage, Value::Bytes(_)))
    }

    fn new_memoryview(&mut self, object: Id) -> Result<Id> {
        if let Some(method) = self.python_buffer_method(object, false)? {
            // PyMemoryView_FromObject requests PyBUF_FULL_RO, including shape and strides.
            let view = self.acquire_python_buffer(object, method, 0x11c)?;
            let outer = self.new_memoryview(view)?;
            let wrapper = self.alloc(Value::BufferWrapper);
            let export = self.alloc(Value::PythonBufferLease {
                exporter: object,
                view,
                wrapper,
            });
            self.heap.note_buffer_view(view);
            let Value::MemoryView { lease, .. } = self.heap.get_mut(outer) else {
                unreachable!()
            };
            *lease = Some(export);
            return Ok(outer);
        }
        let (owner, anonymous, layout, lease) = match *self.heap.get(self.scalar_value(object)) {
            Value::Bytes(_) | Value::ByteArray(_) => (object, false, None, None),
            #[cfg(target_os = "linux")]
            Value::Mmap { .. } => {
                stdlib::mmap::region(self.heap.get(object))?;
                (object, false, None, None)
            }
            Value::MemoryView {
                owner,
                anonymous,
                ref layout,
                lease,
                ..
            } => (
                Self::memoryview_owner(owner)?,
                anonymous,
                layout.clone(),
                lease,
            ),
            Value::BytesIoBuffer { .. } => {
                let len = self.view_element_count(object)?;
                self.invalidate_stream_snapshot(object);
                (object, false, Some(memoryview::ViewLayout::full(len)), None)
            }
            Value::Array(ref a) if a.view_layout().0 != "w" => (object, false, None, None),
            Value::Array(_) => {
                return Err(Error::new(
                    "NotImplementedError",
                    "memoryview of a mutable buffer is unsupported",
                ));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "memoryview: a bytes-like object is required, not '{}'",
                        self.type_name(object)
                    ),
                ));
            }
        };
        let storage = self.view_storage_id(owner);
        #[cfg(target_os = "linux")]
        if matches!(self.heap.get(storage), Value::Mmap { .. }) {
            self.heap.note_buffer_view(storage);
        }
        if matches!(
            self.heap.get(storage),
            Value::Array(_) | Value::ByteArray(_) | Value::BytesIoStorage(_)
        ) {
            self.heap.note_buffer_view(storage);
        }
        let readonly = matches!(
            self.heap.get(object),
            Value::MemoryView { readonly: true, .. }
        );
        Ok(self.alloc(Value::MemoryView {
            owner: Some(owner),
            lease,
            anonymous,
            restricted: false,
            layout,
            readonly,
            hash: None,
        }))
    }

    /// Keep the buffer exported while an argument conversion runs Python.
    fn with_buffer_export<T>(
        &mut self,
        input: Id,
        convert: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let input = self.scalar_value(input);
        let tracks_exports = match self.heap.get(input) {
            Value::ByteArray(_)
            | Value::Array(_)
            | Value::MemoryView { .. }
            | Value::BytesIoBuffer { .. }
            | Value::BytesIoStorage(_) => true,
            #[cfg(target_os = "linux")]
            Value::Mmap { .. } => true,
            _ => false,
        };
        if !tracks_exports {
            return convert(self);
        }
        let mark = self.buffer_exports.len();
        self.buffer_exports.push(input);
        // A raw callback view has no public obj. Keep its mapped backing open
        // for the duration of any further buffer acquisition through that view.
        #[cfg(target_os = "linux")]
        if let Some(window) = self.raw_read_window(input)
            && matches!(self.heap.get(window.backing), Value::Mmap { .. })
        {
            self.buffer_exports.push(window.backing);
        }
        let result = convert(self);
        self.buffer_exports.truncate(mark);
        result
    }

    /// Keep a mutable input exported while Python coerces a buffer API's flag.
    fn truth_with_buffer_export(&mut self, input: Id, flag: Id) -> Result<bool> {
        self.with_buffer_export(input, |ctx| ctx.truth(flag))
    }

    fn exported_bytearray_error() -> Error {
        Error::new(
            "BufferError",
            "Existing exports of data: object cannot be re-sized",
        )
    }

    fn buffer_is_exported(&mut self, storage: Id) -> Result<bool> {
        if self.buffer_exports.iter().any(|&id| id == storage || matches!(self.heap.get(id), Value::BytesIoBuffer {storage: backing,..} if *backing == storage)) {
            return Ok(true);
        }
        if !self.heap.may_have_buffer_views(storage) {
            return Ok(false);
        }
        let (heap, roots) = self.heap_and_roots();
        Ok(heap.live_buffer_export_count(storage, roots)? != 0)
    }

    fn check_bytearray_resize(&mut self, id: Id) -> Result<()> {
        if self.buffer_is_exported(id)? {
            Err(Self::exported_bytearray_error())
        } else {
            Ok(())
        }
    }

    /// Concatenate two string payloads, bypassing Python addition hooks.
    fn concat_strings(&mut self, left: Id, right: Id) -> Result<Id> {
        if [left, right].iter().any(|id| {
            matches!(
                self.heap.get(self.scalar_value(*id)),
                Value::SurrogateStr(_)
            )
        }) {
            let mut result = self.string_codepoints(left)?.into_owned();
            let suffix = self.string_codepoints(right)?;
            result
                .try_reserve_exact(suffix.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            result.extend_from_slice(&suffix);
            return Ok(self.text_codepoints(result));
        }
        let left = self.str_value(left)?;
        let right = self.str_value(right)?;
        let size = left
            .len()
            .checked_add(right.len())
            .ok_or_else(|| Error::new("OverflowError", "strings are too large to concatenate"))?;
        let mut result = String::new();
        result
            .try_reserve_exact(size)
            .map_err(|_| Error::new("MemoryError", ""))?;
        result.push_str(left);
        result.push_str(right);
        Ok(self.text(result))
    }

    fn str_value(&self, id: Id) -> Result<&str> {
        let id = self
            .string_storage(id)
            .ok_or_else(|| Error::new("TypeError", "expected str"))?;
        match self.heap.get(id) {
            Value::Str(value) => Ok(value),
            Value::SurrogateStr(_) => Err(Error::new(
                "UnicodeError",
                "operation requires Unicode scalar values",
            )),
            _ => unreachable!("string storage"),
        }
    }
    /// Copy a name across a mutable context operation, keeping short strings inline.
    fn copy_name(&self, id: Id) -> Result<crate::Str> {
        crate::Str::try_new(self.str_value(id)?).map_err(|_| Error::new("MemoryError", ""))
    }

    fn borrow_int(&self, id: Id) -> Result<Cow<'_, crate::Int>> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Int(n) => Ok(Cow::Borrowed(n)),
            Value::Bool(b) => Ok(Cow::Owned(crate::Int::from(i64::from(*b)))),
            _ => Err(Error::new("TypeError", "expected an integer")),
        }
    }

    fn as_int(&self, id: Id) -> Result<crate::Int> {
        self.borrow_int(id).map(Cow::into_owned)
    }

    /// Narrow only operations that explicitly use a native-sized index or offset.
    fn as_integer(&self, id: Id) -> Result<i64> {
        self.as_int(id)?.to_i64().ok_or_else(|| {
            Error::new(
                "OverflowError",
                "integer does not fit in an index-sized integer",
            )
        })
    }
    fn is_native_iterator(&self, id: Id) -> bool {
        let value = self.heap.get(id);
        matches!(
            value,
            Value::Iterator { .. }
                | Value::TemplateIter { .. }
                | Value::RangeIterator(_)
                | Value::GenericAliasIterator(_)
                | Value::MemoryIterator { .. }
                | Value::StructUnpackIterator { .. }
                | Value::SequenceIterator { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::Scandir(_)
                | Value::DictIterator(_)
                | Value::SetIterator(_)
                | Value::AstWalk { .. }
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::Zip { .. }
                | Value::CallableIterator(_)
                | Value::FormatIterator { .. }
                | Value::Generator { .. }
                | Value::CoroutineWrapper(_)
                | Value::ANextAwaitable { .. }
                | Value::AsyncGeneratorOperation { .. }
                | Value::BufferedRandom(..)
                | Value::File(_)
                | Value::Enumerate { .. }
                | Value::Count { .. }
                | Value::Repeat(_)
        ) || value.is_weak_proxy()
    }

    /// Select the inherited binary iterator for an exact value or a subclass with native storage.
    fn binary_iterator_kind(&self, id: Id) -> Option<&'static str> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Bytes(_) => Some("bytes_iterator"),
            Value::ByteArray(_) => Some("bytearray_iterator"),
            _ => None,
        }
    }

    /// Keeps sequence iterators on their source and respects an existing iterator's cursor.
    fn iterator(&mut self, id: Id) -> Result<Id> {
        let target = self.weak_proxy_target(id)?;
        if target != id {
            return self.with_weak_target(target, |ctx| ctx.iterator(target));
        }
        if self.is_native_iterator(id) {
            let closed = match self.heap.get(id) {
                Value::BufferedRandom(file, _) => file.borrow().closed(),
                Value::File(file) => file.closed(),
                _ => false,
            };
            if closed {
                return Err(Error::new("ValueError", "I/O operation on closed file."));
            }
            return Ok(id);
        }
        let (sequence, kind) = match *self.heap.get(id) {
            Value::Template {
                strings,
                interpolations,
            } => {
                return self.template_iterator(strings, interpolations);
            }
            Value::Context(_) => return Ok(self.context_iterator(id, "Context.keys")),
            Value::MappingProxy(_) => return self.iterator(self.mapping_proxy_target(id)),
            Value::Set { .. } => return Ok(self.set_iterator(id)),
            Value::Deque(ref ring) => {
                let cursor = deque::iterator(id, ring, false, 0);
                return Ok(self.alloc(cursor));
            }
            Value::GenericAlias(ref alias) if alias.is_builtin_alias() => {
                return Ok(self.alloc(Value::GenericAliasIterator(Some(id))));
            }
            Value::GenericAlias(ref alias) if alias.kind.iterates_unpack() => {
                let class = self.builtin(alias.kind.class());
                let method = self.lookup_attr(class, "__iter__")?;
                return self.call(method, vec![id], Vec::new());
            }
            Value::TypeVarTuple { .. } => {
                let unpacked = typing::unpack_parameter(self, id)?;
                (self.alloc(Value::Tuple(vec![unpacked])), "tuple_iterator")
            }
            Value::MemoryView { .. } => return self.memoryview_iterator(id),
            Value::Array(_) => (id, "array.arrayiterator"),
            Value::List(_) => (id, "list_iterator"),
            Value::Tuple(_) | Value::NamedTuple { .. } => (id, "tuple_iterator"),
            Value::Str(ref text) => {
                let kind = if self.heap.is_ascii_str(id) || text.is_ascii() {
                    "str_ascii_iterator"
                } else {
                    "str_iterator"
                };
                (id, kind)
            }
            Value::SurrogateStr(_) => (id, "str_iterator"),
            Value::Range(ref range) => {
                return Ok(self.alloc(Value::RangeIterator(range.iter(false))));
            }
            Value::Dict(_) | Value::DefaultDict { .. } => {
                return Ok(self.dict_iterator(id, DictViewKind::Keys, false, false));
            }
            Value::DictView {
                dict,
                kind,
                ordered,
            } => return Ok(self.dict_iterator(dict, kind, ordered, false)),
            ref value
                if let Some(class) = value.instance_class()
                    && let Some(slot) = self.class_attr(class, "__iter__") =>
            {
                if slot == NONE {
                    return Err(Error::new(
                        "TypeError",
                        format!("'{}' object is not iterable", self.type_name(id)),
                    ));
                }
                let method = self.bind_attribute(slot, id, class)?;
                let result = self.call(method, Vec::new(), Vec::new())?;
                if self.is_native_iterator(result) || self.slot(result, "__next__").is_some() {
                    return Ok(result);
                }
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "iter() returned non-iterator of type '{}'",
                        self.type_name(result)
                    ),
                ));
            }
            _ if let Some(kind) = self.binary_iterator_kind(id) => (id, kind),
            Value::Instance { .. } if let Some((storage, _)) = self.sequence_payload(id) => {
                return self.iterator(storage);
            }
            _ if self.slot(id, "__getitem__").is_some() => {
                return Ok(self.alloc(Value::SequenceIterator {
                    source: Some(id),
                    index: 0,
                }));
            }
            _ => {
                let values = self.items(id)?;
                (self.alloc(Value::Tuple(values)), "iterator")
            }
        };
        Ok(self.forward_iterator(sequence, kind))
    }

    fn forward_iterator(&mut self, sequence: Id, kind: &'static str) -> Id {
        self.alloc(Value::Iterator {
            sequence: Some(sequence),
            offset: 0,
            reverse: false,
            kind,
            context: None,
        })
    }

    fn set_iterator(&mut self, owner: Id) -> Id {
        let len = self.stored_entries(self.scalar_value(owner)).len();
        self.alloc(Value::SetIterator(value::SetIterator::new(owner, len)))
    }

    /// Start a cursor at the first entry without copying keys or values.
    fn dict_iterator(&mut self, dict: Id, kind: DictViewKind, ordered: bool, reverse: bool) -> Id {
        let storage = self.dict_storage(dict).expect("dictionary iterator source");
        let entries = self.stored_entries(storage);
        let iter = DictIterator::new(
            dict,
            kind,
            entries.len(),
            ordered.then(|| (entries.order_state(), entries.ordered_nodes().len())),
            reverse.then(|| {
                if ordered {
                    entries.ordered_nodes().len()
                } else {
                    entries.insertion_end()
                }
            }),
        );
        self.alloc(Value::DictIterator(iter))
    }

    /// Compiled loops test for an item before consuming it. User code must advance only once.
    fn loop_iterator(&mut self, iterable: Id) -> Result<Id> {
        let source = self.iterator(iterable)?;
        Ok(self.loop_cursor(source))
    }

    /// Keep lookahead separate from acquiring the Python iterator.
    fn loop_cursor(&mut self, source: Id) -> Id {
        let value = self.heap.get(source);
        if matches!(
            value,
            Value::TemplateIter { .. }
                | Value::GenericAliasIterator(_)
                | Value::MemoryIterator { .. }
                | Value::StructUnpackIterator { .. }
                | Value::SequenceIterator { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::Scandir(_)
                | Value::DictIterator(_)
                | Value::SetIterator(_)
                | Value::CallableIterator(_)
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::Zip { .. }
                | Value::Generator { .. }
                | Value::CoroutineWrapper(_)
                | Value::ANextAwaitable { .. }
                | Value::AsyncGeneratorOperation { .. }
                | Value::BufferedRandom(..)
                | Value::File(_)
                | Value::Enumerate { .. }
        ) || value.instance_class().is_some()
            || value.is_weak_proxy()
        {
            return self.alloc(Value::LoopIterator {
                source,
                peeked: None,
            });
        }
        source
    }

    /// Borrows the unconsumed part of the iterator's current sequence.
    fn iterator_remaining(&self, id: Id) -> Result<IteratorSlice<'_>> {
        let Value::Iterator {
            sequence,
            offset,
            reverse,
            ..
        } = self.heap.get(id)
        else {
            return Err(Error::new("TypeError", "expected iterator"));
        };
        let Some(sequence) = sequence else {
            return Ok(IteratorSlice::Values(&[]));
        };
        Ok(match self.heap.get(self.scalar_value(*sequence)) {
            Value::List(values) | Value::Tuple(values) | Value::NamedTuple { values, .. } => {
                if *reverse {
                    IteratorSlice::Reversed(values.get(..*offset).unwrap_or_default())
                } else {
                    IteratorSlice::Values(values.get(*offset..).unwrap_or_default())
                }
            }
            Value::SurrogateStr(points) if *reverse => {
                IteratorSlice::ReversedCodepoints(&points[..*offset])
            }
            Value::SurrogateStr(points) => IteratorSlice::Codepoints(&points[*offset..]),
            Value::Array(array) => IteratorSlice::Array(array, *offset),
            Value::Str(text) if *reverse => IteratorSlice::ReversedText(&text[..*offset]),
            Value::Str(text) => IteratorSlice::Text(&text[*offset..]),
            Value::Bytes(bytes) if *reverse => IteratorSlice::ReversedBytes(&bytes[..*offset]),
            Value::Bytes(bytes) | Value::ByteArray(bytes) => {
                IteratorSlice::Bytes(bytes.get(*offset..).unwrap_or_default())
            }
            _ => unreachable!("iterator sequence"),
        })
    }

    /// Reads the current position and permanently detaches the source on exhaustion.
    fn iterator_has_item(&mut self, id: Id) -> Result<bool> {
        if let Value::FormatIterator { input, offset, .. } = *self.heap.get(id) {
            return Ok(offset < self.str_value(input)?.len());
        }
        if matches!(self.heap.get(id), Value::Count { .. }) {
            return Ok(true);
        }
        if let Value::RangeIterator(iter) = self.heap.get(id) {
            return Ok(iter.remaining.as_small() != Some(0));
        }
        if let Value::Repeat(repeat) = self.heap.get(id) {
            return Ok(repeat.remaining != Some(0));
        }
        if let Value::LoopIterator { source, peeked } = *self.heap.get(id) {
            if peeked.is_some() {
                return Ok(true);
            }
            let item = self.next_item(source)?;
            *self.heap.get_mut(id) = Value::LoopIterator {
                source,
                peeked: item,
            };
            return Ok(item.is_some());
        }
        if matches!(self.heap.get(id), Value::AstWalk { .. }) {
            return stdlib::ast::has_item(self, id);
        }
        let more = !self.iterator_remaining(id)?.is_empty();
        if !more && let Value::Iterator { sequence, .. } = self.heap.get_mut(id) {
            *sequence = None;
        }
        Ok(more)
    }

    /// Discard StopIteration's payload only for consumers that treat it as exhaustion.
    fn next_item(&mut self, id: Id) -> Result<Option<Id>> {
        match self.next_result(id) {
            Err(error) if self.is_stop_iteration(&error) => Ok(None),
            result => result,
        }
    }

    fn is_stop_iteration(&mut self, error: &Error) -> bool {
        let stop = self.builtin("StopIteration");
        self.raised_error_matches(error, stop)
    }

    fn sequence_len(&mut self, source: Id) -> Result<usize> {
        let size = builtins::call(self, "len", &[source], &[])?;
        self.as_int(size)?
            .to_i64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                )
            })
    }

    /// PySequence_Length only calls the sequence length slot. Python dict subclasses
    /// acquire it through their type, while the native mappings only have mp_length.
    fn sequence_protocol_len(&mut self, source: Id) -> Result<usize> {
        let kind = match self.heap.get(source) {
            Value::Dict(_) => "dict",
            Value::DefaultDict { .. } => defaultdict::CLASS,
            Value::MappingProxy(_) => "mappingproxy",
            Value::Context(_) => "_contextvars.Context",
            Value::FrameLocalsProxy { .. } => frame_locals::CLASS,
            // The accelerated OrderedDict is an immutable class backed by a dict.
            Value::Instance {
                class,
                dictionary: Some(_),
                ..
            } if let Value::Class { scope, .. } = self.heap.get(*class)
                && self
                    .scope_data(*scope)
                    .class_namespace
                    .as_ref()
                    .is_some_and(|ns| ns.immutable_type) =>
            {
                "collections.OrderedDict"
            }
            _ => return self.sequence_len(source),
        };
        Err(Error::new("TypeError", format!("{kind} is not a sequence")))
    }

    /// Sequence-protocol iteration consults the current __getitem__ at each index.
    fn next_sequence_item(&mut self, id: Id, source: Option<Id>, index: i64) -> Result<Option<Id>> {
        let Some(source) = source else {
            return Ok(None);
        };
        if index == i64::MAX {
            return Err(Error::new("OverflowError", "iter index too large"));
        }
        let key = self.int(index);
        match self.get_item(source, key) {
            Ok(item) => {
                let Value::SequenceIterator { index, .. } = self.heap.get_mut(id) else {
                    unreachable!()
                };
                *index += 1;
                Ok(Some(item))
            }
            Err(error) => {
                let index_error = self.builtin("IndexError");
                if !self.raised_error_matches(&error, index_error)
                    && !self.is_stop_iteration(&error)
                {
                    return Err(error);
                }
                *self.heap.get_mut(id) = Value::SequenceIterator {
                    source: None,
                    index: 0,
                };
                self.heap.weak_edge_removed(source);
                Ok(None)
            }
        }
    }

    fn next_reverse_sequence(
        &mut self,
        id: Id,
        source: Option<Id>,
        remaining: usize,
    ) -> Result<Option<Id>> {
        let result = match (source, remaining.checked_sub(1)) {
            (Some(source), Some(index)) => {
                let index = self.int(index as i64);
                self.get_item(source, index).map(Some)
            }
            _ => Ok(None),
        };
        if let Ok(Some(_)) = result {
            let Value::ReverseSequence {
                remaining: saved, ..
            } = self.heap.get_mut(id)
            else {
                unreachable!()
            };
            *saved = remaining - 1;
            return result;
        }
        *self.heap.get_mut(id) = Value::ReverseSequence {
            source: None,
            remaining: 0,
        };
        if let Err(error) = &result {
            let index_error = self.builtin("IndexError");
            if self.raised_error_matches(error, index_error) || self.is_stop_iteration(error) {
                return Ok(None);
            }
        }
        result
    }

    /// Recognize a partial application of the builtin next to another sentinel iterator.
    fn delegated_callable_next(&self, callable: Id) -> Option<(Id, [Id; 2], Option<Id>)> {
        let Value::Partial {
            function,
            args,
            keywords,
            ..
        } = self.heap.get(callable)
        else {
            return None;
        };
        if !matches!(self.heap.get(*function), Value::Native(name) if name == "next")
            || !matches!(self.heap.get(*keywords), Value::Dict(entries) if entries.is_empty())
        {
            return None;
        }
        let Value::Tuple(args) = self.heap.get(*args) else {
            return None;
        };
        let (iterator, default) = match args.as_slice() {
            [iterator] => (*iterator, None),
            [iterator, default]
                if !matches!(self.heap.get(*default), Value::PartialPlaceholder) =>
            {
                (*iterator, Some(*default))
            }
            _ => return None,
        };
        match self.heap.get(iterator) {
            Value::CallableIterator(Some(state)) => Some((iterator, *state, default)),
            _ => None,
        }
    }

    fn next_callable_item(
        &mut self,
        mut id: Id,
        mut callable: Id,
        mut sentinel: Id,
    ) -> Result<Option<Id>> {
        // Advancing partial(next, child) needs only the parent's sentinel check on return.
        // A reentrant callback can exhaust the parent and release its last reference to id.
        self.heap.keep(id);
        let mut parents = Vec::new();
        while let Some((child, [next, stop], default)) = self.delegated_callable_next(callable) {
            parents
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", "cannot advance iterator chain"))?;
            // Reentrant callbacks may exhaust the parent before we compare its sentinel or return the default.
            self.heap
                .reserve_temporaries(2 + usize::from(default.is_some()))?;
            self.heap.keep(child);
            self.heap.keep(sentinel);
            if let Some(value) = default {
                self.heap.keep(value);
            }
            parents.push((id, sentinel, default));
            id = child;
            callable = next;
            sentinel = stop;
        }
        let mut item = self.call(callable, Vec::new(), Vec::new()).map(Some);
        loop {
            item = self.finish_callable_item(id, sentinel, item);
            let Some((parent, stop, default)) = parents.pop() else {
                return item;
            };
            if let Some(value) = default {
                match &item {
                    Ok(None) => item = Ok(Some(value)),
                    Err(error) if self.is_stop_iteration(error) => item = Ok(Some(value)),
                    _ => {}
                }
            }
            id = parent;
            sentinel = stop;
        }
    }

    /// Apply the sentinel comparison after a callable or delegated iterator returns.
    fn finish_callable_item(
        &mut self,
        id: Id,
        sentinel: Id,
        result: Result<Option<Id>>,
    ) -> Result<Option<Id>> {
        match result {
            Ok(Some(item)) => {
                // A recursive call can exhaust the same iterator inside the callable.
                if matches!(self.heap.get(id), Value::CallableIterator(None)) {
                    return Ok(None);
                }
                if item != sentinel {
                    let equal = self.binary("==", sentinel, item)?;
                    if !self.truth(equal)? {
                        return Ok(Some(item));
                    }
                }
            }
            Ok(None) => {}
            Err(error) if self.is_stop_iteration(&error) => {}
            Err(error) => return Err(error),
        }
        *self.heap.get_mut(id) = Value::CallableIterator(None);
        Ok(None)
    }

    /// Advances a boxed iterator once, leaving its remaining values available to other callers.
    fn next_result(&mut self, id: Id) -> Result<Option<Id>> {
        let target = self.weak_proxy_target(id)?;
        if target != id {
            if !self.is_native_iterator(target) && self.slot(target, "__next__").is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "Weakref proxy referenced a non-iterator '{:.200}' object",
                        self.type_name(target)
                    ),
                ));
            }
            return self.with_weak_target(target, |ctx| ctx.next_item(target));
        }
        if matches!(self.heap.get(id), Value::AsyncGeneratorOperation { .. }) {
            return self.call_async_generator_op(id, "__next__", &[]).map(Some);
        }
        if matches!(self.heap.get(id), Value::ANextAwaitable { .. }) {
            return self.call_anext_method("__next__", &[id]).map(Some);
        }
        if matches!(self.heap.get(id), Value::CoroutineWrapper(_)) {
            return self
                .call_coroutine_method("coroutine_wrapper", "__next__", &[id])
                .map(Some);
        }
        if let Value::Scandir(scan) = self.heap.get(id) {
            return stdlib::os::scandir::advance(self, id, scan.clone());
        }
        if let Value::GenericAliasIterator(source) = self.heap.get_mut(id) {
            let source = source.take();
            return Ok(source.map(|source| {
                let Value::GenericAlias(alias) = self.heap.get(source) else {
                    unreachable!()
                };
                self.alloc(Value::GenericAlias(alias.unpacked_copy()))
            }));
        }
        if matches!(self.heap.get(id), Value::StructUnpackIterator { .. }) {
            return stdlib::structure::advance_iterator(self, id);
        }
        if let Value::MemoryIterator {
            view,
            index,
            length,
        } = *self.heap.get(id)
        {
            return self.next_memoryview_item(id, view, index, length);
        }
        if let Value::SequenceIterator { source, index } = *self.heap.get(id) {
            return self.next_sequence_item(id, source, index);
        }
        if let Value::ReverseSequence { source, remaining } = *self.heap.get(id) {
            return self.next_reverse_sequence(id, source, remaining);
        }
        match self.heap.get_mut(id) {
            Value::DequeIterator {
                deque,
                state,
                remaining,
                reverse,
            } => {
                let (deque, state, mut remaining, reverse) = (*deque, *state, *remaining, *reverse);
                let Value::Deque(ring) = self.heap.get(self.scalar_value(deque)) else {
                    unreachable!()
                };
                let item = ring
                    .cursor_item(state, &mut remaining, reverse)
                    .map(|item| item.copied());
                let Value::DequeIterator {
                    remaining: saved, ..
                } = self.heap.get_mut(id)
                else {
                    unreachable!()
                };
                *saved = remaining;
                return Ok(item?);
            }
            Value::Iterator { .. } => {}
            Value::AstWalk { .. } => return stdlib::ast::advance(self, id),
            Value::TemplateIter { .. } => return self.next_template_item(id),
            Value::DictIterator(iter) => return (*iter).next(self, id),
            Value::SetIterator(cursor) => {
                let mut cursor = *cursor;
                let item = cursor.next(self);
                *self.heap.get_mut(id) = Value::SetIterator(cursor);
                return item;
            }
            Value::FormatIterator { .. } => return self.next_format_row(id),
            Value::Enumerate { input, .. } => {
                let input = *input;
                return self.next_enumerated(id, input);
            }
            Value::Count { .. } => return stdlib::itertools::advance(self, id).map(Some),
            Value::RangeIterator(iter) => {
                let item = iter.next()?;
                return Ok(item.map(|n| self.alloc(Value::Int(n))));
            }
            Value::Repeat(repeat) => return Ok(repeat.next()),
            Value::Map { .. } => return self.next_zip_or_map(id),
            Value::Filter { predicate, input } => {
                let (predicate, input) = (*predicate, *input);
                return stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    self.next_filtered(predicate, input)
                });
            }
            Value::Zip { .. } => {
                // A zip input can itself be a zip, so advancing a row can descend many native frames.
                return stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    self.next_zip_or_map(id)
                });
            }
            Value::Generator { .. } => {
                // Delegated generators resume their children on the native stack.
                return stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    self.resume_native(id, id, generator_protocol::GeneratorInput::Send(NONE))
                });
            }
            Value::LoopIterator { peeked, .. } => return Ok(peeked.take()),
            Value::BufferedRandom(..) | Value::File(_) => {
                let readline = self.lookup_attr(id, "readline")?;
                let line = self.call(readline, Vec::new(), Vec::new())?;
                return Ok(self.truth(line)?.then_some(line));
            }
            Value::CallableIterator(None) => return Ok(None),
            Value::CallableIterator(Some(state)) => {
                let [callable, sentinel] = *state;
                return self.next_callable_item(id, callable, sentinel);
            }
            Value::Instance {
                class,
                itertools: Some(cursor),
                ..
            } if matches!(cursor.as_ref(), stdlib::itertools::Cursor::Tee { base, .. } if base == class) =>
            {
                return stdlib::itertools::next_tee(self, id);
            }
            Value::Instance {
                class,
                itertools: Some(cursor),
                ..
            } if cursor.is_exact_accumulate(*class) => {
                return stdlib::itertools::next_accumulated(self, id);
            }
            Value::Instance {
                class,
                itertools: Some(cursor),
                ..
            } if cursor.is_exact_pairwise(*class) => {
                let base = *class;
                return stdlib::itertools::next_paired(self, base, id).map(Some);
            }
            _ => {
                if let Some(method) = self.bound_slot(id, "__next__")? {
                    return self.call(method, Vec::new(), Vec::new()).map(Some);
                }
                return Err(Error::new(
                    "TypeError",
                    format!("'{}' object is not an iterator", self.type_name(id)),
                ));
            }
        }
        self.next_builtin_sequence_item(id)
    }

    /// Read the current slice once, releasing the source on the first exhausted pull.
    fn next_builtin_sequence_item(&mut self, id: Id) -> Result<Option<Id>> {
        let remaining = self.iterator_remaining(id)?;
        if remaining.is_empty() {
            if let Value::Iterator { sequence, .. } = self.heap.get_mut(id) {
                *sequence = None;
            }
            return Ok(None);
        }
        let (item, width) = match remaining {
            IteratorSlice::Values(values) => (values[0], 1),
            IteratorSlice::Reversed(values) => (values[values.len() - 1], 1),
            IteratorSlice::Array(array, offset) => {
                let value = array.at(offset)?;
                (self.alloc(value), 1)
            }
            IteratorSlice::Bytes(bytes) => (self.int(i64::from(bytes[0])), 1),
            IteratorSlice::ReversedBytes(bytes) => (self.int(i64::from(bytes[bytes.len() - 1])), 1),
            IteratorSlice::Codepoints(points) => (self.text_codepoints(vec![points[0]]), 1),
            IteratorSlice::ReversedCodepoints(points) => {
                (self.text_codepoints(vec![points[points.len() - 1]]), 1)
            }
            IteratorSlice::Text(text) => {
                let width = text.chars().next().expect("nonempty iterator").len_utf8();
                (self.text(crate::Str::new(&text[..width])), width)
            }
            IteratorSlice::ReversedText(text) => {
                let ch = text.chars().next_back().expect("nonempty iterator");
                (self.text(ch.to_string()), ch.len_utf8())
            }
        };
        if let Value::Iterator {
            offset, reverse, ..
        } = self.heap.get_mut(id)
        {
            if *reverse {
                *offset -= width;
            } else {
                *offset += width;
            }
        }
        Ok(Some(item))
    }

    /// Retain an immutable tuple or snapshot the values needed by an unpacking assignment.
    fn unpack_sequence(
        &mut self,
        value: Id,
        count: usize,
        star: Option<usize>,
        read_only: bool,
    ) -> Result<Id> {
        if read_only
            && star.is_none()
            && matches!(self.heap.get(value), Value::Tuple(items) if items.len() == count)
        {
            return Ok(value);
        }
        let items = self.unpack_items(value, count, star)?;
        Ok(self.alloc(Value::List(items)))
    }

    /// Consume the values required by an assignment, probing one extra item for fixed targets.
    fn unpack_items(&mut self, value: Id, count: usize, star: Option<usize>) -> Result<Vec<Id>> {
        if star.is_none()
            && let Value::List(items) | Value::Tuple(items) = self.heap.get(value)
            && items.len() == count
        {
            let items = copy_ids(items)?;
            self.heap.reserve_temporaries(count)?;
            for &item in &items {
                self.heap.keep(item);
            }
            return Ok(items);
        }
        let required = count - usize::from(star.is_some());
        let prefix = star.unwrap_or(count);
        let iter = self.iterator(value).map_err(|error| {
            // __iter__ may have changed the type's slots before raising.
            if self.is_builtin_error(&error, "TypeError")
                && !self.has_iter_slot(value)
                && !self.is_sequence(value)
            {
                let name = stdlib::diagnostic_type_name(self, value);
                let name = &name[..name.floor_char_boundary(200)];
                return Error::new(
                    "TypeError",
                    format!("cannot unpack non-iterable {name} object"),
                );
            }
            error
        })?;
        let mut unpacked = Vec::new();
        unpacked
            .try_reserve_exact(count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        let too_short = |got| {
            let qualifier = if star.is_some() { "at least " } else { "" };
            Error::new(
                "ValueError",
                format!("not enough values to unpack (expected {qualifier}{required}, got {got})"),
            )
        };
        for i in 0..prefix {
            let item = self.next_item(iter)?.ok_or_else(|| too_short(i))?;
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(item);
            unpacked.push(item);
        }
        if let Some(star) = star {
            let tail = self.iterator(iter)?;
            let default = self.int(8);
            let hint = self.length_hint(iter, Some(default))?;
            let mut rest = Vec::new();
            rest.try_reserve_exact(self.as_integer(hint)? as usize)
                .map_err(|_| Error::new("MemoryError", ""))?;
            while let Some(item) = self.next_item(tail)? {
                rest.try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                self.heap.reserve_temporaries(1)?;
                self.heap.keep(item);
                rest.push(item);
            }
            let tail_start = rest
                .len()
                .checked_sub(required - star)
                .ok_or_else(|| too_short(star + rest.len()))?;
            unpacked.push(NONE);
            unpacked.extend(rest.drain(tail_start..));
            self.heap.reserve_temporaries(1)?;
            unpacked[star] = self.alloc(Value::List(rest));
        } else if self.next_item(iter)?.is_some() {
            let size = match self.heap.get(value) {
                Value::List(items) | Value::Tuple(items) => Some(items.len()),
                Value::Dict(items) => Some(items.len()),
                _ => None,
            };
            let detail = size.map_or_else(String::new, |n| format!(", got {n}"));
            return Err(Error::new(
                "ValueError",
                format!("too many values to unpack (expected {count}{detail})"),
            ));
        }
        Ok(unpacked)
    }

    /// Read positional tuple storage without calling a subclass's sequence slots.
    fn tuple_elements(&self, id: Id) -> Option<&[Id]> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Tuple(items) | Value::NamedTuple { values: items, .. } => Some(items),
            _ => None,
        }
    }

    fn tuple_items(&self, id: Id) -> Result<Option<Vec<Id>>> {
        self.tuple_elements(id).map(copy_ids).transpose()
    }

    /// Release the previous iteration's operands before collecting and advancing the iterator.
    fn comprehension_step(&mut self, mark: usize) {
        self.heap.release_temporaries_since(mark);
        self.heap.reserve_slot_or_request_collection();
        self.collect_garbage();
    }

    /// Keep the completed snapshot or exception, releasing iterator temporaries
    /// before the caller can invoke another Python callback.
    fn collect_iterable(&mut self, source: Id, tuple: bool) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let result = self
            .iterator(source)
            .and_then(|iterator| self.collect_iterator_items(iterator, tuple, Vec::new()));
        match &result {
            Ok(snapshot) => self.heap.keep_call_result(mark, *snapshot),
            Err(error) => {
                self.heap.release_temporaries_since(mark);
                for root in error.roots() {
                    self.heap.keep(root);
                }
            }
        }
        result
    }

    fn collect_iterator_items(&mut self, iterator: Id, tuple: bool, items: Vec<Id>) -> Result<Id> {
        self.heap.reserve_temporaries(2)?;
        self.heap.keep(iterator);
        let buffer = self.alloc(Value::List(items));
        let step = self.heap.temporary_mark();
        while let Some(item) = self.next_item(iterator)? {
            let Value::List(items) = self.heap.get_mut(buffer) else {
                unreachable!()
            };
            items
                .try_reserve(1)
                .map_err(|_| crate::Error::memory_error())?;
            items.push(item);
            self.comprehension_step(step);
        }
        if tuple {
            let Value::List(items) = self.heap.get_mut(buffer) else {
                unreachable!()
            };
            let items = std::mem::take(items);
            return Ok(self.alloc(Value::Tuple(items)));
        }
        Ok(buffer)
    }

    /// Test tp_iter without binding a descriptor, including a declaration set to None.
    fn has_iter_slot(&self, value: Id) -> bool {
        let native = |kind: &str| {
            abstract_class::native_collection_method(kind, "__iter__")
                || matches!(
                    kind,
                    "array.array"
                        | "_contextvars.Context"
                        | "typing._CallableType"
                        | "enum.EnumType"
                )
        };
        if let Some(class) = self.heap.get(value).instance_class() {
            return self
                .find_in_mro(class, |base| {
                    (self.own_class_attr(base, "__iter__").is_some()
                        || matches!(self.heap.get(base), Value::Native(kind) if native(kind)))
                    .then_some(())
                })
                .is_some();
        }
        self.is_native_iterator(value)
            || self.heap.get(value).is_weak_proxy()
            || matches!(
                self.heap.get(value),
                Value::GenericAlias(_)
                    | Value::TypeAlias { .. }
                    | Value::TypeVarTuple { .. }
                    | Value::Template { .. }
                    | Value::NamedTuple { .. }
            )
            || self.builtin_type_name(value).is_some_and(native)
    }

    /// Check the Python sequence protocol without consuming iterators.
    pub(super) fn is_sequence(&self, id: Id) -> bool {
        // PySequence_Check excludes dict subclasses even if they override __getitem__.
        self.dict_storage(id).is_none() && self.has_sequence_item_slot(id)
    }

    /// Require the sequence item slot used by native sequence consumers.
    fn require_sequence_item_slot(&self, id: Id) -> Result<()> {
        if self.has_sequence_item_slot(id) {
            return Ok(());
        }
        let kind = stdlib::diagnostic_type_name(self, id);
        let message = if self.has_mapping_subscription(id) || self.heap.get(id).is_weak_proxy() {
            format!("{kind} is not a sequence")
        } else {
            format!("'{kind}' object does not support indexing")
        };
        Err(Error::new("TypeError", message))
    }

    fn has_sequence_item_slot(&self, id: Id) -> bool {
        matches!(
            self.heap.get(self.scalar_value(id)),
            Value::List(_)
                | Value::Tuple(_)
                | Value::NamedTuple { .. }
                | Value::Range(_)
                | Value::Deque(_)
                | Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
                | Value::Array(_)
                | Value::MemoryView { .. }
        ) || self.slot(id, "__getitem__").is_some()
    }

    /// Materialize a PySequence_Fast operand. Exact lists and tuples already have
    /// sequence storage. Other inputs use list construction on their iterator.
    fn fast_sequence_values(&mut self, sequence: Id, not_iterable: &str) -> Result<Vec<Id>> {
        if matches!(self.heap.get(sequence), Value::List(_) | Value::Tuple(_)) {
            return self.items(sequence);
        }
        let iter = self.iterator(sequence).map_err(|err| {
            let type_error = self.builtin("TypeError");
            if self.raised_error_matches(&err, type_error) {
                Error::new("TypeError", not_iterable)
            } else {
                err
            }
        })?;
        let list = self.alloc(Value::List(Vec::new()));
        builtins::list_extend(self, list, iter)?;
        self.items(list)
    }

    fn items(&mut self, id: Id) -> Result<Vec<Id>> {
        if matches!(self.heap.get(id), Value::TypeVarTuple { .. }) {
            return Ok(vec![typing::unpack_parameter(self, id)?]);
        }
        if matches!(self.heap.get(id), Value::Generator { .. }) {
            let buffer = self.collect_iterable(id, false)?;
            let Value::List(items) = self.heap.get_mut(buffer) else {
                unreachable!()
            };
            let items = std::mem::take(items);
            self.heap.reserve_temporaries(items.len())?;
            for &item in &items {
                self.heap.keep(item);
            }
            return Ok(items);
        }
        if matches!(
            self.heap.get(id),
            Value::AstWalk { .. }
                | Value::Template { .. }
                | Value::ANextAwaitable { .. }
                | Value::AsyncGeneratorOperation { .. }
                | Value::TemplateIter { .. }
                | Value::GenericAliasIterator(_)
                | Value::MemoryView { .. }
                | Value::MemoryIterator { .. }
                | Value::StructUnpackIterator { .. }
                | Value::SequenceIterator { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::Scandir(_)
                | Value::DictIterator(_)
                | Value::SetIterator(_)
                | Value::DictView { .. }
                | Value::Zip { .. }
                | Value::CallableIterator(_)
                | Value::FormatIterator { .. }
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::BufferedRandom(..)
                | Value::File(_)
                | Value::Count { .. }
                | Value::Repeat(_)
                | Value::Enumerate { .. }
        ) || matches!(self.heap.get(id), Value::GenericAlias(alias) if alias.is_builtin_alias() || alias.kind.iterates_unpack())
            || self.heap.get(id).is_weak_proxy()
            || self.slot(id, "__iter__").is_some()
            || self.slot(id, "__getitem__").is_some()
        {
            let iter = self.iterator(id)?;
            let mut items = Vec::new();
            if let Value::Repeat(repeat) = self.heap.get(iter)
                && let Some(remaining) = repeat.remaining
            {
                items
                    .try_reserve_exact(remaining)
                    .map_err(|_| Error::new("MemoryError", ""))?;
            }
            while let Some(item) = self.next_item(iter)? {
                items
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                // The Rust vector is invisible to the collector during the next Python call.
                self.heap.reserve_temporaries(1)?;
                self.heap.keep(item);
                items.push(item);
            }
            return Ok(items);
        }
        if let Value::Deque(ring) = self.heap.get(id) {
            let mut values = Vec::new();
            values
                .try_reserve(ring.items.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            values.extend(&ring.items);
            return Ok(values);
        }
        if matches!(self.heap.get(id), Value::Array(_)) {
            let iterator = self.iterator(id)?;
            return self.items(iterator);
        }
        if let Some(values) = self.enum_values(id) {
            if let Some(ordered) = self.class_attr(id, "_member_names_") {
                let names = self.lookup_attr(id, "_member_map_")?;
                return self
                    .items(ordered)?
                    .into_iter()
                    .map(|key| self.get_item(names, key))
                    .collect();
            }
            let Value::Dict(entries) = self.heap.get(values) else {
                unreachable!()
            };
            return Ok(entries.iter().map(|&(_, member)| member).collect());
        }
        if matches!(self.heap.get(id), Value::MappingProxy(_)) {
            return self.items(self.mapping_proxy_target(id));
        }
        if let Some(storage) = self.string_storage(id)
            && storage != id
        {
            return self.items(storage);
        }
        if let Value::List(items) = self.heap.get(self.scalar_value(id)) {
            return copy_ids(items);
        }
        if let Some(items) = self.tuple_items(id)? {
            return Ok(items);
        }
        match self.heap.get(id).clone() {
            Value::Set { entries, .. }
            | Value::Dict(entries)
            | Value::DefaultDict { entries, .. } => {
                Ok(entries.into_iter().map(|(key, _)| key).collect())
            }
            Value::Context(bindings) => Ok(bindings.keys().copied().collect()),
            Value::Range(_) | Value::RangeIterator(_) => {
                let iter = self.iterator(id)?;
                let Value::RangeIterator(cursor) = self.heap.get(iter) else {
                    unreachable!()
                };
                let size = cursor.remaining.to_i64().ok_or_else(|| {
                    Error::new(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    )
                })?;
                let mut items = Vec::new();
                items
                    .try_reserve_exact(size as usize)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                while let Some(item) = self.next_item(iter)? {
                    items.push(item);
                }
                Ok(items)
            }
            Value::SurrogateStr(points) => Ok(points
                .into_iter()
                .map(|p| self.text_codepoints(vec![p]))
                .collect()),
            Value::Str(s) => Ok(s.chars().map(|c| self.text(c.to_string())).collect()),
            Value::Bytes(s) | Value::ByteArray(s) => {
                Ok(s.into_iter().map(|b| self.int(i64::from(b))).collect())
            }
            Value::Iterator { .. } => {
                let remaining = self.iterator_remaining(id)?;
                let mut values = Vec::new();
                values.try_reserve_exact(remaining.len()).map_err(|_| {
                    Error::new("MemoryError", "iterator is too large to materialize")
                })?;
                match remaining {
                    IteratorSlice::Values(items) => values.extend_from_slice(items),
                    IteratorSlice::Reversed(items) => values.extend(items.iter().rev().copied()),
                    IteratorSlice::Array(..)
                    | IteratorSlice::Bytes(_)
                    | IteratorSlice::ReversedBytes(_)
                    | IteratorSlice::Codepoints(_)
                    | IteratorSlice::ReversedCodepoints(_)
                    | IteratorSlice::Text(_)
                    | IteratorSlice::ReversedText(_) => {
                        while let Some(item) = self.next_item(id)? {
                            values.push(item);
                        }
                        return Ok(values);
                    }
                }
                if let Value::Iterator { sequence, .. } = self.heap.get_mut(id) {
                    *sequence = None;
                }
                Ok(values)
            }
            Value::Instance { .. } if self.binary_iterator_kind(id).is_some() => {
                let iter = self.iterator(id)?;
                self.items(iter)
            }
            _ => Err(Error::new(
                "TypeError",
                format!("'{}' object is not iterable", self.type_name(id)),
            )),
        }
    }
    fn scope(&mut self, module: &str, parent: Option<Id>) -> Id {
        self.alloc(Value::Scope(allocator_api2::boxed::Box::new(Scope::new(
            module.into(),
            parent,
        ))))
    }
    /// Comprehensions own their loop variables, but assignment expressions bind outside them.
    fn comprehension_scope(&mut self, parent: Id, targets: Id) -> Result<Id> {
        let targets = self
            .str_value(targets)?
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<std::collections::HashSet<_>>();
        Ok(self.comprehension_scope_with_locals(parent, targets.into()))
    }

    fn comprehension_scope_with_locals(&mut self, parent: Id, locals: value::DeclaredLocals) -> Id {
        let enclosing = self.scope_data(parent);
        let module = enclosing.module.clone();
        let (parent, kind) =
            if enclosing.class_body.is_some() || enclosing.class_namespace.is_some() {
                (
                    enclosing.parent.expect("class has an enclosing scope"),
                    ComprehensionKind::ClassBody,
                )
            } else {
                (parent, ComprehensionKind::Ordinary)
            };
        let scope = self.scope(&module, Some(parent));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.comprehension = Some(kind);
        frame.closure.declared = locals;
        frame.closure.declared_complete = true;
        scope
    }

    /// A walrus binds in the first enclosing code unit outside any eager comprehension.
    fn named_expression_scope(&self, mut scope: Id, name: &str) -> Id {
        while self.scope_data(scope).comprehension.is_some() {
            scope = self
                .scope_data(scope)
                .parent
                .expect("enclosing comprehension scope");
        }
        let frame = self.scope_data(scope);
        if frame.closure.environment && !frame.closure.cells.contains_key(name) {
            return frame
                .parent
                .expect("closure environment has module globals");
        }
        scope
    }

    fn scope_data(&self, scope: Id) -> &Scope {
        let Value::Scope(s) = self.heap.get(scope) else {
            unreachable!("compiled scope handle")
        };
        s
    }
    fn root_scope(&self, mut scope: Id) -> Id {
        while let Some(parent) = self.scope_data(scope).parent {
            scope = parent;
        }
        scope
    }
    fn binding_scope(&self, scope: Id, name: &str) -> Id {
        if self.scope_data(scope).closure.nonlocals.contains(name) {
            let mut cursor = self.scope_data(scope).parent;
            while let Some(parent) = cursor {
                let frame = self.scope_data(parent);
                if frame.closure.cells.contains_key(name) || frame.closure.declared.contains(name) {
                    return parent;
                }
                cursor = frame.parent;
            }
        }
        if self.scope_data(scope).globals.contains(name) {
            return self.root_scope(scope);
        }
        scope
    }

    fn scope_name(&self, scope: Id, name: &str) -> Option<Id> {
        let scope = self.scope_data(scope);
        if let Some(&cell) = scope.closure.cells.get(name)
            && let Value::Cell(contents) = self.heap.get(cell)
        {
            return *contents;
        }
        self.namespace_binding(scope, name)
    }

    /// Read the scope's own dictionary, excluding cells captured from another scope.
    fn namespace_binding(&self, scope: &Scope, name: &str) -> Option<Id> {
        if let Some(namespace) = scope.binding_namespace(name) {
            return self.dict_named_item(namespace, name);
        }
        self.frame_binding(scope, name)
    }

    fn frame_binding(&self, scope: &Scope, name: &str) -> Option<Id> {
        let middle = name.as_bytes().get(name.len() / 2).copied().unwrap_or(0);
        let bucket = name.len() ^ usize::from(middle);
        let slot = &self.scope_name_slots[bucket % self.scope_name_slots.len()];
        if let Some((key, value)) = scope.names.get_index(slot.get())
            && key == name
        {
            return Some(*value);
        }
        let (index, _, &value) = scope.names.get_full(name)?;
        slot.set(index);
        Some(value)
    }

    /// Enumerate a scope's bindings even after Python code materializes its namespace.
    fn scope_bindings(&self, scope: Id) -> impl Iterator<Item = (&str, Id)> {
        let frame = self.scope_data(scope);
        let names = frame
            .names
            .iter()
            .map(|(name, &value)| (name.as_str(), value));
        let dictionary = frame.namespace.and_then(|dict| self.dict_entries(dict));
        names.chain(
            dictionary
                .into_iter()
                .flatten()
                .filter_map(|&(key, value)| self.str_value(key).ok().map(|name| (name, value))),
        )
    }

    /// Link a function's free-variable cells to its globals without retaining its caller's frame.
    fn closure_environment(&mut self, globals: Id, names: Vec<String>, cells: &[Id]) -> Id {
        if cells.is_empty() {
            return globals;
        }
        let module = self.scope_data(globals).module.clone();
        let environment = self.scope(&module, Some(globals));
        let Value::Scope(frame) = self.heap.get_mut(environment) else {
            unreachable!()
        };
        frame.closure.environment = true;
        frame.closure.declared_complete = true;
        frame
            .closure
            .cells
            .extend(names.into_iter().zip(cells.iter().copied()));
        environment
    }

    /// Materialize the class namespace used by its mapping proxy and annotation functions.
    fn class_dictionary(&mut self, scope: Id) -> Id {
        let dictionary = self.class_namespace_dict(scope, false);
        self.alloc(Value::MappingProxy(dictionary))
    }

    /// The own members of object use the same interned descriptors as attribute lookup.
    fn raw_object_member(&mut self, name: &str) -> Option<Id> {
        if crate::objects::OBJECT_METHODS.contains(&name) || name == "__class__" {
            return Some(self.builtin(&format!("object.{name}")));
        }
        if name == "__doc__" {
            let doc = self
                .builtins
                .get("object.__doc__")
                .copied()
                .unwrap_or_else(|| {
                    let doc = self.text(Self::OBJECT_DOC);
                    self.builtins.insert("object.__doc__".to_owned(), doc);
                    doc
                });
            return Some(doc);
        }
        None
    }

    fn object_dictionary(&mut self) -> Id {
        let dict = if let Some(&dict) = self.builtins.get("object.__dict__") {
            dict
        } else {
            let mut entries = Vec::new();
            for name in crate::objects::OBJECT_METHODS
                .iter()
                .copied()
                .chain(["__class__", "__doc__"])
            {
                let value = self.raw_object_member(name).unwrap();
                entries.push((self.heap.code_name(name), value));
            }
            let dict = self.dict(entries);
            self.builtins.insert("object.__dict__".into(), dict);
            dict
        };
        self.alloc(Value::MappingProxy(dict))
    }

    const OBJECT_DOC: &'static str = "The base class of the class hierarchy.\n\nWhen called, it accepts no arguments and returns a new featureless\ninstance that has no instance attributes and cannot be given any.\n";

    /// Expose the implemented members of an immutable native type through ordinary class lookup.
    fn native_class_dictionary(
        &mut self,
        class: Id,
        storage: &'static str,
        members: impl Iterator<Item = &'static str>,
    ) -> Result<Id> {
        let dictionary = if let Some(&dictionary) = self.builtins.get(storage) {
            dictionary
        } else {
            let mut entries = Vec::with_capacity(members.size_hint().0);
            for name in members {
                let key = self.heap.code_name(name);
                let value = match self.own_class_attr(class, name) {
                    Some(member) => member,
                    None => self.lookup_attr(class, name)?,
                };
                let value = if matches!(self.heap.get(value), Value::Native(symbol) if symbol == "str.maketrans")
                {
                    method_decorator::static_method(self, value)?
                } else {
                    value
                };
                entries.push((key, value));
            }
            let dictionary = self.dict(entries);
            self.builtins.insert(storage.into(), dictionary);
            dictionary
        };
        Ok(self.alloc(Value::MappingProxy(dictionary)))
    }

    /// Materialize a mutable class namespace for an annotation closure.
    fn class_namespace_dict(&mut self, scope: Id, building: bool) -> Id {
        let frame = self.scope_data(scope);
        match frame
            .class_namespace
            .as_ref()
            .and_then(|class| class.dictionary)
        {
            Some(existing) => existing,
            None => {
                let names = frame.names.clone();
                let entries = names
                    .into_iter()
                    .filter(|(name, _)| building || name != "__qualname__")
                    .map(|(name, value)| (self.text(name), value))
                    .collect();
                let dictionary = self.dict(entries);
                let Value::Scope(frame) = self.heap.get_mut(scope) else {
                    unreachable!()
                };
                frame
                    .class_namespace
                    .get_or_insert_with(Default::default)
                    .dictionary = Some(dictionary);
                dictionary
            }
        }
    }

    /// Expose the current class-body mapping and use it for subsequent name bindings.
    fn active_class_namespace(&mut self, scope: Id) -> Id {
        if let Some(dict) = self.scope_data(scope).namespace {
            return dict;
        }
        let dict = self.class_namespace_dict(scope, true);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.namespace = Some(dict);
        frame.names.clear();
        dict
    }

    /// Share the class namespace cell across source-loaded and precompiled method providers.
    pub(super) fn class_annotation_dict_cell(&mut self, scope: Id) -> Id {
        let owner = self.scope_data(scope);
        if owner.class_body.is_none()
            && let Some(&cell) = owner.closure.cells.get("__classdict__")
        {
            return cell;
        }
        if let Some(cell) = self
            .scope_data(scope)
            .class_namespace
            .as_ref()
            .and_then(|ns| ns.annotation_dict_cell)
        {
            return cell;
        }
        let dict = self.active_class_namespace(scope);
        let cell = self.alloc(Value::Cell(Some(dict)));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_namespace.as_mut().unwrap().annotation_dict_cell = Some(cell);
        cell
    }

    /// Copy the bindings from an exposed class-body namespace before creating the type.
    fn finish_class_namespace(&mut self, scope: Id) -> Result<Option<ExposedClassBody>> {
        let frame = self.scope_data(scope);
        let Some(body) = frame.namespace else {
            return Ok(None);
        };
        let annotation_cell = frame
            .class_namespace
            .as_ref()
            .and_then(|ns| ns.annotation_dict_cell);
        let names: indexmap::IndexMap<crate::Str, Id> = self
            .dict_entries(body)
            .expect("class body dictionary")
            .iter()
            .filter_map(|&(key, value)| {
                self.str_value(key)
                    .ok()
                    .filter(|&name| annotation_cell.is_none() || name != "__classdictcell__")
                    .map(|name| (crate::Str::new(name), value))
            })
            .collect();
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_body = Some(names.keys().map(|name| name.as_str().to_owned()).collect());
        frame.names = names;
        frame.namespace = None;
        frame.class_namespace.as_mut().unwrap().dictionary = None;
        self.heap.keep(body);
        if let Some(cell) = annotation_cell {
            let key = self.text("__classdictcell__");
            self.put_item(body, key, cell)?;
        }
        Ok(Some(ExposedClassBody {
            mapping: body,
            annotation_cell,
        }))
    }

    fn module_globals(&mut self, scope: Id) -> Result<Id> {
        let scope = self.root_scope(scope);
        self.publish_builtin_module_names(scope)?;
        self.module_globals_storage(scope)
    }

    /// Materialize the live binding dictionary without exposing it to Python.
    fn module_globals_storage(&mut self, scope: Id) -> Result<Id> {
        self.check_thread_reporting_namespace(scope)?;
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!("compiled scope handle")
        };
        if let Some(globals) = frame.exec_globals {
            return Ok(globals.dict);
        }
        if let Some(dict) = frame.namespace {
            return Ok(dict);
        }
        let names = std::mem::take(&mut frame.names);
        let entries = names
            .into_iter()
            .map(|(name, v)| (self.heap.code_name(&name), v))
            .collect();
        let dict = self.dict(entries);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.namespace = Some(dict);
        Ok(dict)
    }

    fn remove_binding(&mut self, scope: Id, name: &str, key: Id) -> Result<bool> {
        if let Some(&cell) = self.scope_data(scope).closure.cells.get(name) {
            if let Some(previous) = self.heap.replace_cell(cell, None) {
                self.release_list_owner(previous);
                return Ok(true);
            }
            return Ok(false);
        }
        let frame = self.scope_data(scope);
        if let Some(locals) = frame.exec_locals.or(frame.class_body_mapping()) {
            // DELETE_NAME reports a missing name on deletion failure, even for a colliding key.
            return Ok(self.delete_item(locals, key).is_ok());
        }
        if let Some(namespace) = self.scope_data(scope).binding_namespace(name) {
            if self.scope_name(scope, name).is_none() {
                return Ok(false);
            }
            let storage = self.dict_storage(namespace).expect("namespace dictionary");
            self.delete_item(storage, key)?;
            return Ok(true);
        }
        let Value::Scope(data) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        if let Some(order) = &mut data.class_body {
            order.retain(|member| member != name);
        }
        let Some(previous) = data.names.shift_remove(name) else {
            return Ok(false);
        };
        let dictionary = data
            .class_namespace
            .as_ref()
            .and_then(|class| class.dictionary);
        // Mirror mutations can run Python. Their own removal releases the stored
        // value, so never consult the old name slot after touching the dictionary.
        self.heap.weak_edge_removed(previous);
        if name != "__qualname__"
            && let Some(dictionary) = dictionary
        {
            self.delete_item(dictionary, key)?;
        } else {
            self.release_list_owner(previous);
        }
        Ok(true)
    }

    /// Bind a parameter in a fresh function scope using checked name and table storage.
    fn bind_argument(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        let name = crate::Str::try_new(name).map_err(|_| Error::new("MemoryError", ""))?;
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!("function call scope")
        };
        debug_assert!(frame.class_body.is_none());
        debug_assert!(frame.class_namespace.is_none());
        debug_assert!(frame.namespace.is_none());
        if !frame.names.contains_key(name.as_str()) {
            frame
                .names
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        if let Some(previous) = frame.names.insert(name, value)
            && previous != value
        {
            self.heap.weak_edge_removed(previous);
        }
        Ok(())
    }

    fn bind(&mut self, scope: Id, name: &str, value: Id) {
        let scope = self.binding_scope(scope, name);
        self.bind_local(scope, name, value);
    }

    fn bind_local(&mut self, scope: Id, name: &str, value: Id) {
        if let Some(&cell) = self.scope_data(scope).closure.cells.get(name) {
            if let Some(previous) = self.heap.replace_cell(cell, Some(value))
                && previous != value
            {
                self.release_list_owner(previous);
            }
            return;
        }
        if let Some(namespace) = self.scope_data(scope).binding_namespace(name) {
            let key = self.heap.code_name(name);
            let storage = self.dict_storage(namespace).expect("namespace dictionary");
            self.store_binding_item(storage, key, value)
                .expect("string key in namespace dictionary");
            return;
        }
        let Value::Scope(s) = self.heap.get_mut(scope) else {
            unreachable!("compiled scope handle")
        };
        let previous = if let Some(binding) = s.names.get_mut(name) {
            Some(std::mem::replace(binding, value))
        } else {
            if let Some(order) = &mut s.class_body {
                order.push(name.to_owned());
            }
            s.names.insert(crate::Str::new(name), value);
            None
        };
        let dictionary = s
            .class_namespace
            .as_ref()
            .and_then(|class| class.dictionary);
        let mirror = dictionary.filter(|_| s.class_body.is_some() || name != "__qualname__");
        let displaced = previous.filter(|&old| old != value);
        if let Some(old) = displaced {
            self.heap.weak_edge_removed(old);
        }
        if let Some(dictionary) = mirror {
            let key = self.text(name);
            self.store_binding_item(dictionary, key, value)
                .expect("class dictionary has string keys");
        } else if let Some(old) = displaced {
            self.release_list_owner(old);
        }
    }
    /// Describe a missing binding in the scope that owns it.
    fn missing_binding(&mut self, origin: Id, scope: Id, name: &str) -> Error {
        let frame = self.scope_data(scope);
        let declared = frame.class_body.is_none() && frame.closure.declared.contains(name);
        let mut error = if declared && origin == scope {
            Error::new(
                "UnboundLocalError",
                format!(
                    "cannot access local variable '{name}' where it is not associated with a value"
                ),
            )
        } else if declared || frame.closure.cells.contains_key(name) {
            Error::new(
                "NameError",
                format!(
                    "cannot access free variable '{name}' where it is not associated with a value in enclosing scope"
                ),
            )
        } else {
            Error::new("NameError", format!("name '{name}' is not defined"))
        };
        if error.kind == "NameError" {
            error.tracing().name_error_name = Some(self.heap.code_name(name));
        }
        error
    }

    /// Read a live namespace key, including equality with keys installed by Python.
    fn lookup_dict_name(&mut self, dict: Id, name: &str) -> Result<Option<Id>> {
        if let Some(value) = self.scalar_dict_name(dict, name) {
            return Ok(value);
        }
        let key = self.heap.code_name(name);
        let (_, index) = self.find_dictionary_key(dict, key, None)?;
        Ok(index.map(|index| self.stored_entries(dict)[index].1))
    }

    fn lookup_scope_name(&mut self, scope: Id, name: &str) -> Result<Option<Id>> {
        if let Some(dict) = self.scope_data(scope).binding_namespace(name) {
            return self.lookup_dict_name(dict, name);
        }
        Ok(self.scope_name(scope, name))
    }

    fn lookup_class_local(&mut self, scope: Id, name: &str) -> Result<Id> {
        if let Some(value) = self.lookup_scope_name(scope, name)? {
            return Ok(value);
        }
        self.lookup(self.root_scope(scope), name)
    }

    fn lookup(&mut self, mut scope: Id, name: &str) -> Result<Id> {
        let origin = scope;
        loop {
            let target = self.binding_scope(scope, name);
            let frame = self.scope_data(scope);
            if target != scope
                && frame.class_body_mapping().is_some()
                && frame.closure.nonlocals.contains(name)
                && let Some(value) = self.lookup_scope_name(scope, name)?
            {
                // LOAD_FROM_DICT_OR_DEREF checks class locals before its enclosing cell.
                return Ok(value);
            }
            scope = target;
            if let Some(locals) = self.scope_data(scope).exec_locals {
                match self.load_exec_name(locals, name) {
                    Err(error) if self.is_builtin_error(&error, "KeyError") => {}
                    value => return value,
                }
            }
            let s = self.scope_data(scope);
            if let Some(&cell) = s.closure.cells.get(name) {
                let value = match self.heap.get(cell) {
                    Value::Cell(value) => *value,
                    _ => self.namespace_binding(s, name),
                };
                return value.ok_or_else(|| self.missing_binding(origin, scope, name));
            }
            if let Some(globals) = s.exec_globals {
                return self.source_global(globals, name);
            }
            let binding = match s.binding_namespace(name) {
                Some(dict) => self.lookup_dict_name(dict, name)?,
                None => self.frame_binding(s, name),
            };
            if let Some(id) = binding {
                return Ok(id);
            }
            let s = self.scope_data(scope);
            if s.class_body.is_none() && s.closure.declared.contains(name) {
                return Err(self.missing_binding(origin, scope, name));
            }
            // An eager comprehension in a class cannot capture an outer function's __class__.
            if name == "__class__" && s.comprehension == Some(ComprehensionKind::ClassBody) {
                scope = self.root_scope(scope);
                continue;
            }
            if name == "__class__"
                && s.class_body.is_none()
                && let Some(cell) = s.class_cell
                && let Value::Cell(value) = self.heap.get(cell)
            {
                return value.ok_or_else(|| Error::new("NameError", "cannot access free variable '__class__' where it is not associated with a value in enclosing scope"));
            }
            if let Some(parent) = s.parent {
                scope = parent;
            } else {
                break;
            }
        }
        let builtins = self.default_source_builtins()?;
        self.lookup_dict_name(builtins, name)?
            .ok_or_else(|| self.missing_binding(origin, scope, name))
    }
    /// Resolve the factory in the mapping payload of an exact or derived defaultdict.
    fn defaultdict_factory(&self, object: Id) -> Option<Id> {
        let Value::DefaultDict { factory, .. } = self.heap.get(self.dict_storage(object)?) else {
            return None;
        };
        Some(*factory)
    }

    /// Replace the factory without exposing it through the subclass instance dictionary.
    fn replace_defaultdict_factory(&mut self, object: Id, value: Id) -> bool {
        let Some(storage) = self.dict_storage(object) else {
            return false;
        };
        let Value::DefaultDict { factory, .. } = self.heap.get_mut(storage) else {
            return false;
        };
        let old = std::mem::replace(factory, value);
        self.heap.weak_edge_removed(old);
        true
    }

    /// Give a descriptor type its own read-only documentation descriptor.
    fn descriptor_doc_member(&mut self, owner: &'static str) -> Id {
        let get = self.builtin(&format!("_{owner}_doc"));
        self.alloc(Value::GetSetDescriptor {
            owner,
            name: "__doc__",
            get,
            set: None,
        })
    }

    /// Register a builtin number's read-only fields and expose the declaring descriptors.
    fn install_numeric_dictionary<const N: usize>(
        &mut self,
        class: Id,
        owner: &'static str,
        methods: &[&str],
        fields: [&'static str; N],
    ) {
        for field in fields {
            let get = self.builtin(&format!("_{owner}_getset.{field}"));
            let descriptor = self.alloc(Value::GetSetDescriptor {
                owner,
                name: field,
                get,
                set: None,
            });
            self.builtins.insert(format!("{owner}.{field}"), descriptor);
        }
        let members = methods
            .iter()
            .chain(builtins::real_methods(owner))
            .copied()
            .chain(["__new__"])
            .chain((owner == "int").then_some("__sizeof__"))
            .chain(fields);
        let mut entries: Vec<_> = members
            .map(|member| {
                let key = self.heap.code_name(member);
                (key, self.builtin(&format!("{owner}.{member}")))
            })
            .collect();
        if owner == "int" {
            let doc = self.text(concat!(
                "int([x]) -> integer\nint(x, base=10) -> integer\n\n",
                "Convert a number or string to an integer, or return 0 if no arguments\n",
                "are given.  If x is a number, return x.__int__().  For floating-point\n",
                "numbers, this truncates towards zero.\n\n",
                "If x is not a number or if base is given, then x must be a string,\n",
                "bytes, or bytearray instance representing an integer literal in the\n",
                "given base.  The literal can be preceded by '+' or '-' and be surrounded\n",
                "by whitespace.  The base defaults to 10.  Valid bases are 0 and 2-36.\n",
                "Base 0 means to interpret the base from the string as an integer literal.\n",
                ">>> int('0b100', base=0)\n4",
            ));
            entries.push((self.heap.code_name("__doc__"), doc));
        }
        let dictionary = self.dict(entries);
        self.native_type_dicts.insert(class, dictionary);
    }

    fn builtin(&mut self, name: &str) -> Id {
        if let Some(&value) = self.builtins.get(name) {
            return value;
        }
        if name == "UnsupportedOperation" {
            let class = stdlib::io::unsupported_operation(self);
            self.builtins.insert(name.to_owned(), class);
            return class;
        }
        let value = self.alloc(Value::Native(name.to_owned()));
        self.builtins.insert(name.to_owned(), value);
        if self.is_type_object(value) {
            self.heap.register_native_type(value);
        }
        if name == "object" {
            let descriptor = self.alloc(Value::LayoutDescriptor(value, "__class__"));
            self.builtins.insert("object.__class__".into(), descriptor);
        } else if name == "type" {
            let descriptor = self.alloc(Value::LayoutDescriptor(value, "__text_signature__"));
            self.builtins
                .insert("type.__text_signature__".into(), descriptor);
        }
        if name == "code" {
            self.install_code_members(value);
        }
        if name == thread_handle::CLASS {
            thread_handle::install(self, value);
        }
        let codec_members = match name {
            value::ZSTD_COMPRESS_TYPE => Some((
                value::ZSTD_COMPRESS_TYPE,
                value::ZSTD_COMPRESS_METHODS,
                &["last_mode"][..],
            )),
            value::ZSTD_DECOMPRESS_TYPE => Some((
                value::ZSTD_DECOMPRESS_TYPE,
                value::CODEC_DECOMPRESS_METHODS,
                value::BZ2_DECOMPRESS_FIELDS,
            )),
            value::BZ2_COMPRESS_TYPE => Some((
                value::BZ2_COMPRESS_TYPE,
                value::CODEC_COMPRESS_METHODS,
                &[][..],
            )),
            value::BZ2_DECOMPRESS_TYPE => Some((
                value::BZ2_DECOMPRESS_TYPE,
                value::CODEC_DECOMPRESS_METHODS,
                value::BZ2_DECOMPRESS_FIELDS,
            )),
            value::LZMA_COMPRESS_TYPE => Some((
                value::LZMA_COMPRESS_TYPE,
                value::CODEC_COMPRESS_METHODS,
                &[][..],
            )),
            value::LZMA_DECOMPRESS_TYPE => Some((
                value::LZMA_DECOMPRESS_TYPE,
                value::CODEC_DECOMPRESS_METHODS,
                value::LZMA_DECOMPRESS_FIELDS,
            )),
            _ => None,
        };
        if let Some((owner, methods, fields)) = codec_members {
            let (module, _) = owner.rsplit_once('.').expect("qualified codec type");
            let mut entries = vec![(
                self.heap.code_name("__module__"),
                self.heap.code_name(module),
            )];
            if owner == value::ZSTD_COMPRESS_TYPE {
                for (mode, n) in [("CONTINUE", 0), ("FLUSH_BLOCK", 1), ("FLUSH_FRAME", 2)] {
                    let n = self.int(n);
                    entries.push((self.heap.code_name(mode), n));
                }
            }
            if matches!(module, "_bz2" | "compression.zstd") {
                let new = self.builtin(&format!("{owner}.__new__"));
                entries.push((self.heap.code_name("__new__"), new));
            }
            for &field in fields {
                let member = self.readonly_member(owner, field);
                entries.push((self.heap.code_name(field), member));
            }
            for &method in methods {
                let member = self.builtin(&format!("{name}.{method}"));
                entries.push((self.heap.code_name(method), member));
            }
            let dictionary = self.dict(entries);
            self.native_type_dicts.insert(value, dictionary);
        }
        if matches!(name, "filter" | "map" | "zip") {
            let mut entries = Vec::new();
            let methods = if name == "filter" {
                builtins::FILTER_METHODS
            } else {
                builtins::ITERATOR_SLOTS
            };
            for slot in std::iter::once("__new__").chain(methods.iter().copied()) {
                let member = self.builtin(&format!("{name}.{slot}"));
                entries.push((self.heap.code_name(slot), member));
            }
            if name == "filter" {
                let doc = self.text(concat!(
                    "Return an iterator yielding those items of iterable for which function(item)\n",
                    "is true. If function is None, return the items that are true."
                ));
                entries.push((self.heap.code_name("__doc__"), doc));
            }
            let dictionary = self.dict(entries);
            self.native_type_dicts.insert(value, dictionary);
        }
        if name == "super" {
            let mut entries = Vec::new();
            for name in [
                "__new__",
                "__repr__",
                "__getattribute__",
                "__init__",
                "__get__",
            ] {
                let method = self.builtin(&format!("super.{name}"));
                entries.push((self.heap.code_name(name), method));
            }
            for name in ["__thisclass__", "__self__", "__self_class__"] {
                let field = self.readonly_member("super", name);
                entries.push((self.heap.code_name(name), field));
            }
            let dictionary = self.dict(entries);
            self.native_type_dicts.insert(value, dictionary);
        }
        if matches!(name, "_contextvars.ContextVar" | "_contextvars.Token") {
            let key = self.heap.code_name("__class_getitem__");
            let method = self.builtin(&format!("{name}.__class_getitem__"));
            let dictionary = self.dict(vec![(key, method)]);
            self.native_type_dicts.insert(value, dictionary);
        }
        if name == stdlib::functools::PARTIAL_CLASS {
            let mut entries = Vec::new();
            for method in [
                "__new__",
                "__call__",
                "__get__",
                "__class_getitem__",
                "__reduce__",
                "__setstate__",
            ] {
                let slot = self.builtin(&format!("{name}.{method}"));
                entries.push((self.heap.code_name(method), slot));
            }
            for field in ["func", "args", "keywords"] {
                let member = self.readonly_member(stdlib::functools::PARTIAL_CLASS, field);
                entries.push((self.heap.code_name(field), member));
            }
            let namespace = self.alloc(Value::LayoutDescriptor(value, "__dict__"));
            entries.push((self.heap.code_name("__dict__"), namespace));
            let repr = self.builtin("functools.partial.__repr__");
            entries.push((self.heap.code_name("__repr__"), repr));
            let module = self.heap.code_name("functools");
            entries.push((self.heap.code_name("__module__"), module));
            let doc = self.text("Create a new function with partial application of the given arguments\nand keywords.");
            entries.push((self.heap.code_name("__doc__"), doc));
            let dictionary = self.dict(entries);
            self.native_type_dicts.insert(value, dictionary);
        }
        if name == stdlib::functools::LRU_CACHE_CLASS {
            let mut entries = Vec::new();
            for &member in stdlib::functools::LRU_METHODS {
                let method = self.builtin(&format!("{name}.{member}"));
                entries.push((self.heap.code_name(member), method));
            }
            entries.push((
                self.heap.code_name("__module__"),
                self.heap.code_name("functools"),
            ));
            let dictionary = self.dict(entries);
            self.native_type_dicts.insert(value, dictionary);
        }
        if name == "coroutine" {
            let origin = self.readonly_member("coroutine", "cr_origin");
            let key = self.heap.code_name("cr_origin");
            let dictionary = self.dict(vec![(key, origin)]);
            self.native_type_dicts.insert(value, dictionary);
        }
        if name == stdlib::time::STRUCT_TIME {
            stdlib::structseq::install_type(
                self,
                value,
                &stdlib::time::LAYOUT,
                stdlib::time::STRUCT_TIME_DOC,
            );
        }
        if name == "struct.Struct" {
            let mut entries = Vec::new();
            for member in builtins::STRUCT_METHODS.iter().copied().chain(["__new__"]) {
                let key = self.heap.code_name(member);
                entries.push((key, self.builtin(&format!("struct.Struct.{member}"))));
            }
            for field in ["size", "format"] {
                let get = self.builtin(&format!("struct.Struct.{field}"));
                let member = self.alloc(Value::GetSetDescriptor {
                    owner: "struct.Struct",
                    name: field,
                    get,
                    set: None,
                });
                entries.push((self.heap.code_name(field), member));
            }
            let dictionary = self.dict(entries);
            self.native_type_dicts.insert(value, dictionary);
        }
        if name == deque::CLASS {
            deque::install(self);
        }
        if let Some(owner) = Self::dict_view_owner(name)
            && owner == name
        {
            let get = self.builtin(&format!("_dictview_mapping.{owner}"));
            let mapping = self.alloc(Value::GetSetDescriptor {
                owner,
                name: "mapping",
                get,
                set: None,
            });
            let key = self.heap.code_name("mapping");
            let namespace = self.dict(vec![(key, mapping)]);
            self.native_type_dicts.insert(value, namespace);
        }
        if let Some(owner) = stdlib::itertools::cursor_type(name) {
            stdlib::itertools::install_namespace(self, value, owner);
        }
        if builtins::is_enum_base(name) {
            self.install_enum_base(value, name);
        }
        if let Some(members) = stdlib::datetime::members(name) {
            members.install(self, value);
        }
        if let Some(class) = typing::ParameterClass::lookup(name) {
            class.install(self, value);
        } else {
            typing::install_alias_class(self, name, value);
        }
        if let Some(kind) = builtins::exception_kind(name) {
            for base in builtins::exception_ancestors(kind) {
                self.builtin(base);
            }
            let mut entries = Vec::new();
            for (index, field) in exception_members::names(kind).iter().enumerate() {
                let field = self.heap.code_name(field);
                let member = self.alloc(Value::SlotMember {
                    owner: value,
                    name: field,
                    storage: SlotStorage::Exception(kind, index),
                });
                entries.push((field, member));
            }
            if kind == imre_ast::exceptions::Kind::OSError {
                let member = self.alloc(Value::LayoutDescriptor(value, "characters_written"));
                entries.push((self.heap.code_name("characters_written"), member));
            }
            for slot in ["__new__", "__init__"] {
                if builtins::declares_exception_constructor(kind, slot) {
                    let method = self.builtin(&format!("{name}.{slot}"));
                    entries.push((self.heap.code_name(slot), method));
                }
            }
            for &slot in builtins::exception_methods(kind) {
                let method = self.builtin(&format!("{name}.{slot}"));
                entries.push((self.heap.code_name(slot), method));
            }
            if name == "BaseException" {
                for field in
                    std::iter::once("__dict__").chain(builtins::EXCEPTION_GETSETS.iter().copied())
                {
                    let descriptor = self.alloc(Value::LayoutDescriptor(value, field));
                    entries.push((self.heap.code_name(field), descriptor));
                }
            }
            if name == "BaseExceptionGroup" {
                for slot in std::iter::once("__class_getitem__")
                    .chain(exception_group::METHODS.iter().copied())
                {
                    let method = self.builtin(&format!("{name}.{slot}"));
                    entries.push((self.heap.code_name(slot), method));
                }
                for field in ["message", "exceptions"] {
                    let descriptor = self.readonly_member("BaseExceptionGroup", field);
                    entries.push((self.heap.code_name(field), descriptor));
                }
            }
            if !entries.is_empty() {
                let namespace = self.dict(entries);
                self.native_type_dicts.insert(value, namespace);
            }
        }
        if let Some(symbol) = object_size::descriptor(name) {
            self.builtin(symbol);
        }
        let mut additional: &[&str] = &[];
        let methods = match name {
            "type" => &[
                "__call__",
                "mro",
                "__subclasses__",
                "__prepare__",
                "__setattr__",
                "__text_signature__",
            ][..],
            "object" => &[
                "__hash__",
                "__ne__",
                "__format__",
                "__init_subclass__",
                "__getstate__",
                "__reduce__",
                "__reduce_ex__",
            ],
            "dict" => builtins::DICT_METHODS,
            "module" => {
                self.install_module_namespace(value);
                &[]
            }
            "function" => &["__call__", "__get__"],
            "method" => &["__get__", "__reduce__"],
            "builtin_function_or_method" | "method-wrapper" => &["__reduce__"],
            defaultdict::CLASS => {
                self.builtin("dict");
                defaultdict::METHODS
            }
            "list" => builtins::LIST_METHODS,
            "method_descriptor" | "wrapper_descriptor" | "classmethod_descriptor" => &["__get__"],
            "tuple" => &[
                "__repr__",
                "__getitem__",
                "__getnewargs__",
                "__iter__",
                "__len__",
                "count",
                "index",
            ],
            "set" | "frozenset" => builtins::set_methods(name),
            "set_iterator" => builtins::SET_ITERATOR_METHODS,
            "bytearray" => {
                additional = &["fromhex"];
                builtins::BYTEARRAY_METHODS
            }
            "property" => property::METHODS,
            "memoryview" => builtins::MEMORYVIEW_METHODS,
            "bool" => {
                self.builtin("int");
                &["__repr__"]
            }
            "int" => &[
                "__add__",
                "__index__",
                "bit_count",
                "bit_length",
                "from_bytes",
                "to_bytes",
                "__format__",
                "__repr__",
            ],
            "float" => &[
                "__format__",
                "__getformat__",
                "fromhex",
                "as_integer_ratio",
                "__repr__",
            ],
            "complex" => complex::METHODS,
            "bytes" => {
                additional = &["__new__", "fromhex"];
                builtins::BYTES_METHODS
            }
            "str" => {
                self.builtin("str.maketrans");
                builtins::STR_METHODS
            }
            deque::ITERATOR | deque::REVERSE_ITERATOR => {
                deque::install_iterator_namespace(self, value, name);
                &[]
            }
            thread_local::CLASS => &["__new__", "__getattribute__", "__setattr__", "__delattr__"],
            stdlib::fixed_structseq::SCHED_PARAM => &["__new__", "__reduce__", "__replace__"],
            weakref::PROXY_TYPE => weakref::PROXY_METHODS,
            weakref::CALLABLE_PROXY_TYPE => weakref::CALLABLE_PROXY_METHODS,
            weakref::REFERENCE_TYPE => weakref::REFERENCE_METHODS,
            _ => &[],
        };
        let mut entries = Vec::new();
        for &method in methods
            .iter()
            .chain(additional)
            .chain(builtins::set_operators(name))
            .chain(builtins::real_methods(name))
        {
            let member = self.builtin(&format!("{name}.{method}"));
            entries.push((self.heap.code_name(method), member));
        }
        match name {
            // These types publish their dictionaries lazily, including static methods
            // and other descriptors that need an initialized runtime.
            "object" | "str" | defaultdict::CLASS => {}
            "getset_descriptor" => {
                let doc = self.descriptor_doc_member("getset_descriptor");
                let key = self.heap.code_name("__doc__");
                let dictionary = self.dict(vec![(key, doc)]);
                self.native_type_dicts.insert(value, dictionary);
            }
            "wrapper_descriptor" => {
                let doc = self.descriptor_doc_member("wrapper_descriptor");
                let bind = self.builtin("wrapper_descriptor.__get__");
                let entries = vec![
                    (self.heap.code_name("__get__"), bind),
                    (self.heap.code_name("__doc__"), doc),
                ];
                let dictionary = self.dict(entries);
                self.native_type_dicts.insert(value, dictionary);
            }
            "int" => self.install_numeric_dictionary(
                value,
                "int",
                methods,
                ["real", "imag", "numerator", "denominator"],
            ),
            "float" => self.install_numeric_dictionary(value, "float", methods, ["real", "imag"]),
            _ if !entries.is_empty() && !self.native_type_dicts.contains_key(&value) => {
                if name == "function" {
                    let descriptor = self.alloc(Value::LayoutDescriptor(value, "__code__"));
                    entries.push((self.heap.code_name("__code__"), descriptor));
                } else if name == "method" {
                    for field in ["__func__", "__self__"] {
                        let descriptor = self.readonly_member("method", field);
                        entries.push((self.heap.code_name(field), descriptor));
                    }
                }
                if let Some(symbol) = object_size::descriptor(name) {
                    entries.push((self.heap.code_name("__sizeof__"), self.builtin(symbol)));
                }
                if self.own_class_attr(value, "__hash__") == Some(NONE) {
                    entries.push((self.heap.code_name("__hash__"), NONE));
                }
                let dictionary = self.dict(entries);
                self.native_type_dicts.insert(value, dictionary);
            }
            _ => {}
        }
        value
    }
    fn execute(&mut self, code: Id, scope: Id, owner: FrameOwner) -> Result<Id> {
        let call = self.prepare_execution(code, scope, owner, false)?;
        // SAFETY: this synchronous caller owns the context with the prepared frame.
        unsafe { call.run(self) }
    }

    fn prepare_execution(
        &mut self,
        code: Id,
        scope: Id,
        owner: FrameOwner,
        discard: bool,
    ) -> Result<calls::RegisteredCall> {
        match self.code_object(code).body {
            value::Body::Native(entry, _) => {
                self.heap.enter(scope, code, owner);
                Ok(calls::RegisteredCall::Code {
                    entry,
                    module: matches!(owner, FrameOwner::Module(_)),
                    discard,
                })
            }
            value::Body::Suspended(_) => {
                let result = self.new_suspended(code, scope, owner)?;
                Ok(calls::RegisteredCall::Returned(if discard {
                    NONE
                } else {
                    result
                }))
            }
            value::Body::Parsed { .. } => self.missing_compiled_body(code),
        }
    }

    /// Run a linked code body against its already-entered frame.
    ///
    /// # Safety
    /// The caller owns the context and keeps the initializer loaded until it returns.
    unsafe fn run_code_frame(context: *mut Self, entry: Code, module: bool) -> Result<Id> {
        // SAFETY: the entered frame roots the scope through native reentry.
        let scope = unsafe { &*context }.heap.current_scope();
        // SAFETY: the caller keeps the entry loaded, and no context borrow spans the native call.
        let result = unsafe { entry(context, scope) };
        // SAFETY: the native initializer has returned to the owner of the context.
        let ctx = unsafe { &mut *context };
        if module && ctx.pending.is_none() {
            ctx.register_decimal_class(ctx.heap.current_code(), entry, scope);
        }
        ctx.finish_native_frame(result)
    }

    fn finish_native_frame(&mut self, result: Id) -> Result<Id> {
        self.leave_frame(result, self.pending.is_none());
        self.pending.take().map_or(Ok(result), Err)
    }

    /// Resolve a named import from a loaded module, including package children.
    fn import_from(&mut self, object: Id, member: Id) -> Result<Id> {
        let name = self.str_value(member)?.to_owned();
        match self.lookup_named_attr(object, &name, Some(member)) {
            Err(error) if self.is_attribute_error(&error) => {}
            value => return value,
        }
        // A circular import can register a child before publishing it on its parent.
        let module = self
            .optional_attr(object, "__name__")?
            .filter(|&id| self.string_storage(id).is_some());
        if let Some(module) = module {
            self.heap.keep(module);
            let suffix = self.text(format!(".{name}"));
            let child = self.concat_strings(module, suffix)?;
            if let Some(value) = self.initialized_registry_entry(child)? {
                return Ok(value);
            }
        }

        let spec = self.optional_attr(object, "__spec__")?;
        let origin = self.spec_file_origin(spec)?;
        // Only the spec's file origin participates in the shadowing check.
        let shadow = origin
            .map(|id| self.possibly_shadowing(id))
            .transpose()?
            .unwrap_or(false);
        let path = origin.or_else(|| match *self.heap.get(object) {
            Value::Module { scope, .. } if spec.is_some() => self
                .scope_name(scope, "__file__")
                .filter(|&id| self.string_storage(id).is_some()),
            _ => None,
        });
        if let Some(path) = path {
            self.heap.keep(path);
        }
        let label = module.unwrap_or_else(|| self.text("<unknown module name>"));
        let shadowed_stdlib = shadow && self.stdlib_name(label)?;
        let mut fields = vec![member, label];
        let format = if shadowed_stdlib {
            fields.extend([origin.unwrap(), label]);
            "cannot import name %r from %r (consider renaming %r since it has the same name as the standard library module named %r and prevents importing that standard library module)"
        } else if let Some(spec) = spec
            && self.spec_initializing(spec)?
        {
            if shadow {
                fields.push(origin.unwrap());
                "cannot import name %r from %r (consider renaming %r if it has the same name as a library you intended to import)"
            } else if let Some(path) = path {
                fields.push(path);
                "cannot import name %r from partially initialized module %r (most likely due to a circular import) (%s)"
            } else {
                "cannot import name %r from partially initialized module %r (most likely due to a circular import)"
            }
        } else if let Some(path) = path {
            fields.push(path);
            "cannot import name %r from %r (%s)"
        } else {
            "cannot import name %r from %r (unknown location)"
        };
        let fields = self.alloc(Value::Tuple(fields));
        let message = builtins::percent(self, format, fields)?;
        let mut error = Error::new("ImportError", self.display(message));
        error.args = Some(self.alloc(Value::Tuple(vec![message])));
        let trace = error.tracing();
        trace.import_message = Some(message);
        trace.import_names = [module.unwrap_or(NONE), path.unwrap_or(NONE), member].map(Some);
        Err(error)
    }

    fn spec_file_origin(&mut self, spec: Option<Id>) -> Result<Option<Id>> {
        let Some(spec) = spec else { return Ok(None) };
        self.heap.keep(spec);
        let has_location = self.optional_attr(spec, "has_location")?;
        if !self.optional_truth(has_location, false)? {
            return Ok(None);
        }
        let origin = self
            .optional_attr(spec, "origin")?
            .filter(|&id| self.string_storage(id).is_some());
        if let Some(origin) = origin {
            self.heap.keep(origin);
        }
        Ok(origin)
    }

    fn stdlib_name(&mut self, name: Id) -> Result<bool> {
        if let Some(names) = self.sys_binding("stdlib_module_names")?
            && let storage = self.scalar_value(names)
            && matches!(self.heap.get(storage), Value::Set { .. })
        {
            builtins::set_member(self, storage, name, false)
        } else {
            Ok(false)
        }
    }

    /// Check whether the script's startup search path could shadow an installed module.
    fn possibly_shadowing(&self, origin: Id) -> Result<bool> {
        if self.config["safe_path"].as_bool() == Some(true) {
            return Ok(false);
        }
        // CPython compares the directory spelling in a bounded, NUL-terminated
        // wide-character buffer. A POSIX filename can contain surrogate escapes.
        let limit = (libc::PATH_MAX as usize).max(1024);
        let path: Cow<'_, [u32]> = match self.heap.get(self.scalar_value(origin)) {
            Value::Str(text) => Cow::Owned(text.chars().take(limit).map(u32::from).collect()),
            Value::SurrogateStr(points) => Cow::Borrowed(&points[..points.len().min(limit)]),
            _ => unreachable!("a file origin is a Python string"),
        };
        let path = path.split(|&point| point == 0).next().unwrap_or_default();
        let sep = u32::from(std::path::MAIN_SEPARATOR);
        let Some(end) = path.iter().rposition(|&point| point == sep) else {
            return Ok(false);
        };
        let dir = &path[..end];
        let root = if path[end + 1..]
            .iter()
            .copied()
            .eq("__init__.py".bytes().map(u32::from))
        {
            dir.iter()
                .rposition(|&point| point == sep)
                .map(|end| &dir[..end])
        } else {
            Some(dir)
        };
        let Some((root, first)) = root.zip(self.startup_path0.as_deref()) else {
            return Ok(false);
        };
        if first.is_empty() {
            let Some(cwd) = Self::startup_cwd() else {
                return Ok(false);
            };
            return Ok(Self::import_path_points(cwd.as_os_str().as_encoded_bytes())? == root);
        }
        Ok(first == root)
    }

    fn missing_module(name: &str, key: Id) -> Error {
        Error::new(
            "ModuleNotFoundError",
            format!("No module named {}", string_repr(name)),
        )
        .with_import_name(key)
    }

    /// Read the interpreter's original sys dictionary even if its import-cache entry changed.
    fn sys_binding(&mut self, name: &str) -> Result<Option<Id>> {
        let sys = match self.threads.system_module {
            Some(sys) => sys,
            None => self.module("sys")?,
        };
        let Value::Module { scope, .. } = *self.heap.get(sys) else {
            unreachable!()
        };
        Ok(self.scope_name(scope, name))
    }

    /// Read the original sys namespace for native searches before PathFinder is installed.
    fn startup_search_path(&mut self) -> Result<Id> {
        self.sys_binding("path")?
            .ok_or_else(|| Error::new("RuntimeError", "unable to get sys.path"))
    }

    fn registered_module(&mut self, name: Id) -> Result<Option<Id>> {
        let (_, index) = self.find_dictionary_key(self.modules, name, None)?;
        Ok(index.map(|i| self.stored_entries(self.modules)[i].1))
    }

    fn public_modules(&mut self) -> Result<Id> {
        match self.threads.system_module {
            Some(sys) => self.lookup_attr(sys, "modules"),
            None => Ok(self.modules),
        }
    }

    fn loading_sentinel(&mut self) -> Result<Id> {
        if !self.import_registry_installed {
            return Ok(self.import_missing);
        }
        let dict = self.bootstrap_globals;
        if let Some(sentinel) = self.dict_named_item(dict, "_NEEDS_LOADING") {
            return Ok(sentinel);
        }
        let builtins = self.namespace_builtins(dict, None)?;
        self.source_global(value::ExecGlobals { dict, builtins }, "_NEEDS_LOADING")
    }

    fn public_module_get(&mut self, name: Id, with_sentinel: bool) -> Result<Id> {
        let modules = self.public_modules()?;
        let get = if matches!(self.heap.get(modules), Value::Dict(_)) {
            None
        } else {
            let method = self.lookup_attr(modules, "get")?;
            self.heap.keep(method);
            Some(method)
        };
        // A get descriptor can replace _NEEDS_LOADING before the argument is evaluated.
        let default = with_sentinel.then(|| self.loading_sentinel()).transpose()?;
        if let Some(get) = get {
            let args = match default {
                Some(default) => vec![name, default],
                None => vec![name],
            };
            return self.call(get, args, Vec::new());
        }
        let (_, index) = self.find_dictionary_key(modules, name, None)?;
        Ok(index.map_or(default.unwrap_or(NONE), |i| {
            self.stored_entries(modules)[i].1
        }))
    }

    fn public_registered_module(&mut self, name: Id) -> Result<Option<Id>> {
        let value = self.public_module_get(name, true)?;
        Ok((value != self.loading_sentinel()?).then_some(value))
    }

    fn spec_initializing(&mut self, spec: Id) -> Result<bool> {
        let flag = self.optional_attr(spec, "_initializing")?;
        self.optional_truth(flag, false)
    }

    fn module_initializing(&mut self, module: Id) -> Result<bool> {
        self.optional_attr(module, "__spec__")?
            .map_or(Ok(false), |spec| self.spec_initializing(spec))
    }

    /// PyImport_GetModule waits for initialization and verifies the same cache entry survived.
    fn initialized_registry_entry(&mut self, key: Id) -> Result<Option<Id>> {
        let Some(module) = self.registered_module(key)? else {
            return Ok(None);
        };
        self.heap.keep(module);
        if module != NONE {
            if self.module_initializing(module)? {
                let wait = self.lookup_attr(self.bootstrap_module, "_lock_unlock_module")?;
                self.call(wait, vec![key], Vec::new())?;
            }
            if self.registered_module(key)? != Some(module) {
                return Ok(None);
            }
        }
        Ok(Some(module))
    }

    fn cached_import(&mut self, name: &str, key: Id) -> Result<Option<Id>> {
        let mut retries = 0;
        let module = loop {
            let Some(module) = self.public_registered_module(key)? else {
                break self.public_registered_module(key)?;
            };
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(module);
            if self.module_initializing(module)? {
                break self.public_registered_module(key)?;
            }
            if self.public_module_get(key, false)? == module {
                break Some(module);
            }
            // A callback can replace the entry while _find_and_load checks its spec.
            retries += 1;
            self.check_call_depth(retries)?;
        };
        match module {
            Some(NONE) => Err(Error::new(
                "ModuleNotFoundError",
                format!("import of {name} halted; None in sys.modules"),
            )
            .with_import_name(key)),
            module => Ok(module),
        }
    }

    fn publish_module(&mut self, name: &str, module: Id) -> Result<()> {
        let key = self.text(name);
        let modules = self.public_modules()?;
        self.put_item(modules, key, module)
    }

    fn module(&mut self, name: &str) -> Result<Id> {
        let key = self.text(name);
        let cached = self.registered_module(key)?;
        self.module_from_cache(name, key, cached)
    }

    /// The original __import__ waits for a cached module's initialization.
    fn import_by_key(&mut self, name: &str, key: Id) -> Result<Id> {
        let cached = if self.import_registry_installed {
            self.initialized_registry_entry(key)?
        } else {
            self.registered_module(key)?
        };
        self.module_from_cache(name, key, cached)
    }

    fn module_from_cache(&mut self, name: &str, key: Id, cached: Option<Id>) -> Result<Id> {
        if let Some(module) = cached
            && module != NONE
        {
            return Ok(module);
        }
        self.find_module_from(name, key, NONE, None)
            .map_err(|error| self.trim_import_traceback(error))?
            .ok_or_else(|| Self::missing_module(name, key))
    }

    /// Resolve registered and filesystem modules against the current import path.
    fn find_module(&mut self, name: &str) -> Result<Option<Id>> {
        let key = self.text(name);
        self.find_module_from(name, key, NONE, None)
            .map_err(|error| self.trim_import_traceback(error))
    }

    fn find_module_from(
        &mut self,
        mut name: &str,
        mut key: Id,
        source_globals: Id,
        import: Option<Id>,
    ) -> Result<Option<Id>> {
        let mut parents = Vec::new();
        let mut module = loop {
            if let Some(id) = self.cached_import(name, key)? {
                break Some(id);
            }
            let Some((parent, child)) = name.rsplit_once('.') else {
                break self.load_module(name, key, None, source_globals, import)?;
            };
            let parent_key = self.text(parent);
            if self.import_registry_installed {
                let modules = self.public_modules()?;
                if self.contains(parent_key, modules)? {
                    break self.load_module(
                        name,
                        key,
                        Some((parent_key, child)),
                        source_globals,
                        import,
                    )?;
                }
            }
            // Each unresolved parent consumes three importlib frames.
            self.check_call_depth(3 * (parents.len() + 1))?;
            parents
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            parents.push((name, key, parent, child));
            name = parent;
            key = parent_key;
        };
        for (name, child_key, parent, child) in parents.into_iter().rev() {
            module.ok_or_else(|| Self::missing_module(parent, key))?;
            module =
                self.load_module(name, child_key, Some((key, child)), source_globals, import)?;
            key = child_key;
        }
        Ok(module)
    }

    /// Run a module initializer after its parent has been resolved.
    fn load_module(
        &mut self,
        name: &str,
        key: Id,
        parent: Option<(Id, &str)>,
        source_globals: Id,
        import: Option<Id>,
    ) -> Result<Option<Id>> {
        let parent = if let Some((parent_key, child_name)) = parent {
            let child = self.public_module_get(key, false)?;
            if child != NONE {
                return Ok(Some(child));
            }
            let modules = self.public_modules()?;
            let parent = self.get_item(modules, parent_key)?;
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(parent);
            Some((parent_key, parent, child_name))
        } else {
            None
        };
        if self.import_registry_installed {
            let path = match parent {
                Some((parent_key, parent, _)) => {
                    let path = self.parent_import_path(name, key, parent)?;
                    self.heap.reserve_temporaries(2)?;
                    self.heap.keep(path);
                    let spec = self.lookup_attr(parent, "__spec__")?;
                    self.heap.keep(spec);
                    if self.spec_initializing(spec)? {
                        let import = import.unwrap_or_else(|| self.builtin("__import__"));
                        self.call(import, vec![parent_key], Vec::new())?;
                    }
                    // Either parent attribute can publish the child before the finder runs.
                    let child = self.public_module_get(key, false)?;
                    if child != NONE {
                        return Ok(Some(child));
                    }
                    path
                }
                None => NONE,
            };
            let spec = self.meta_call(source_globals, "_find_spec", vec![key, path])?;
            if spec == NONE {
                return Ok(None);
            }
            let module = self.meta_call(source_globals, "_load_unlocked", vec![spec])?;
            self.bind_import_child(name, source_globals, module)?;
            return Ok(Some(module));
        }
        // Bootstrap initializers and live sys.modules hooks can reenter Python.
        stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
            self.load_bootstrap_module(
                name,
                key,
                parent.map(|(_, id, child)| (id, child)),
                source_globals,
            )
        })
    }

    /// Import site before the entry, publishing __main__ first if a startup hook will run.
    fn initialize_startup_site(&mut self) -> Result<()> {
        if self.config["sitecustomize"] != true {
            return self.module("site").map(drop);
        }
        let (main, _, _, _) = self.allocate_import_module("__main__", &[], None)?;
        self.publish_module("__main__", main)?;
        let site = self.module("site")?;
        if let Some(path) = self.config["user_site_pth"].as_str().map(str::to_owned) {
            let add = self.lookup_attr(site, "addsitedir")?;
            let path = self.text(path);
            self.call(add, vec![path], Vec::new())?;
            let later: Vec<String> =
                serde_json::from_value(self.config["after_user_site"].clone()).unwrap_or_default();
            for path in later {
                let sys = self.module("sys")?;
                let paths = self.lookup_attr(sys, "path")?;
                let path = self.text(path);
                if !self.contains(path, paths)? {
                    let append = self.lookup_attr(paths, "append")?;
                    self.call(append, vec![path], Vec::new())?;
                }
            }
        }
        let hook = self.lookup_attr(site, "execsitecustomize")?;
        self.call(hook, Vec::new(), Vec::new())?;
        Ok(())
    }

    // CPython's _Py_wgetcwd uses a MAXPATHLEN buffer and treats an unavailable cwd as absent.
    fn startup_cwd() -> Option<PathBuf> {
        std::env::current_dir().ok().filter(|path| {
            path.as_os_str().as_encoded_bytes().len() < (libc::PATH_MAX as usize).max(1024)
        })
    }

    /// Convert Python filesystem text without invoking path-like hooks.
    fn import_path(&mut self, id: Id) -> Result<PathBuf> {
        if let Ok(text) = self.str_value(id)
            && text.is_ascii()
        {
            return Ok(PathBuf::from(text));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(self.encode_os_path(id)?).into())
        }
        #[cfg(not(unix))]
        Ok(self.str_value(id)?.into())
    }

    fn import_path_points(bytes: &[u8]) -> Result<Vec<u32>> {
        let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
        Ok(codec.decode_filesystem(bytes)?)
    }

    fn import_path_text(&mut self, path: &Path) -> Result<Id> {
        if let Some(text) = path.to_str()
            && text.is_ascii()
        {
            return Ok(self.text(text));
        }
        Ok(self.text_codepoints(Self::import_path_points(
            path.as_os_str().as_encoded_bytes(),
        )?))
    }

    /// Install the search entry after site, keeping its configured value independent of sys.path.
    fn insert_startup_path0(&mut self, launched: bool) -> Result<()> {
        let entry = if self.config["path_entry"] != true
            && self.config["command_source"].is_string()
        {
            self.text("")
        } else if self.config["path_entry"] != true && self.config["module_entry"].is_string() {
            let Some(cwd) = Self::startup_cwd() else {
                return Ok(());
            };
            if !launched && self.config["path0_bytes"].is_array() {
                let bytes: Vec<u8> = serde_json::from_value(self.config["path0_bytes"].clone())
                    .expect("compiler path0 bytes");
                if cwd.as_os_str().as_encoded_bytes() != bytes {
                    self.saved_entry_root = Some((cwd.clone(), Self::import_path_points(&bytes)?));
                }
            }
            self.import_path_text(&cwd)?
        } else if self.config["path0_bytes"].is_array() {
            let bytes: Vec<u8> = serde_json::from_value(self.config["path0_bytes"].clone())
                .expect("compiler path0 bytes");
            self.text_codepoints(Self::import_path_points(&bytes)?)
        } else {
            let first = PathBuf::from(self.config["paths"][0].as_str().expect("compiler path0"));
            self.import_path_text(&first)?
        };
        self.startup_path0 = Some(self.string_codepoints(entry)?.into_owned());
        let path = self.startup_search_path()?;
        let storage = self
            .list_storage(path)
            .ok_or_else(|| Error::new("SystemError", "bad argument to internal function"))?;
        let Value::List(items) = self.heap.get_mut(storage) else {
            unreachable!()
        };
        items
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        items.insert(0, entry);
        Ok(())
    }

    fn prepare_main_module(&mut self) -> Result<Option<MainModuleFrame>> {
        let packaged = self.config["packaged_file_module"].as_str();
        // The compiler registers the selected FILE first, even when another search root
        // supplies a package or module with the same qualified name.
        let Some(source) = self
            .sources
            .get(packaged.unwrap_or("__main__"))
            .and_then(|sources| sources.first())
        else {
            return Ok(None);
        };
        let initializer = source.code;
        let source = (
            source.path.clone(),
            importlib::SourceBody::Registered(initializer),
        );
        let packaged = packaged.map(str::to_owned);
        let (module, scope, name, origin) =
            self.allocate_import_module("__main__", &[], Some(&source))?;
        self.publish_module("__main__", module)?;
        let (module, scope) = if let Some(qualified) = packaged {
            self.prepare_packaged_file(name, &qualified, &source.0)?
        } else {
            (module, scope)
        };
        let code = match origin {
            Some(filename) => self.module_code_at(initializer, &source.0, filename),
            None => self.module_code(initializer, &source.0),
        }
        .inspect_err(|_| {
            self.finish_initialization(module);
        })?;
        let value::Body::Native(entry, _) = self.code_object(code).body else {
            unreachable!("registered module code has a native entry")
        };
        self.heap.enter(scope, code, FrameOwner::Module(name));
        Ok(Some(MainModuleFrame { module, entry }))
    }

    /// Before installing the registry, execute its dependencies and the script's explicit entry.
    fn load_bootstrap_module(
        &mut self,
        name: &str,
        key: Id,
        parent: Option<(Id, &str)>,
        source_globals: Id,
    ) -> Result<Option<Id>> {
        if name == "__main__" {
            // SAFETY: a recursive bootstrap import synchronously runs the frame it just pushed.
            return self
                .prepare_main_module()?
                .map(|frame| unsafe { frame.run(self) })
                .transpose();
        }
        let mut locations = Vec::new();
        let source = if stdlib::is_module(name) || self.is_frozen(name) {
            if matches!(name, "email" | "importlib" | "json" | "re" | "urllib") {
                locations.push(PathBuf::from(format!("<imre stdlib>/{name}")));
            } else if name == "collections" {
                locations.push(stdlib::package_path(self, name)?);
            }
            // A sealed namespace may have both native operations and bundled Python classes.
            // Only its bundled initializer may extend it, never a same-named file on sys.path.
            self.bundled_source(name)
        } else {
            let (search, child) = if let Some((parent, child)) = parent {
                (self.parent_import_path(name, key, parent)?, child)
            } else {
                (self.startup_search_path()?, name)
            };
            self.source_on_path(name, child, search, parent.is_none(), &mut locations)?
        };
        if source.is_none() && !stdlib::is_module(name) {
            return Ok(None);
        }
        let (module, scope, name_id, origin) =
            self.allocate_import_module(name, &locations, source.as_ref())?;
        let key = self.text(name);
        let modules = self.public_modules()?;
        self.put_item(modules, key, module)?;
        if let Err(error) = self.run_initializer(
            name,
            module,
            scope,
            name_id,
            source_globals,
            source.map(|(path, body)| (path, body, origin)),
        ) {
            let exception = self.raised_instance(error);
            self.with_import_exception(exception, |ctx| {
                let modules = ctx.public_modules()?;
                match ctx.delete_item(modules, key) {
                    Err(error) if ctx.is_builtin_error(&error, "KeyError") => Ok(()),
                    result => result,
                }
            })?;
            return self.op("reraise", exception, NONE, NONE).map(Some);
        }
        // An initializer can replace its own registry entry. The successful import
        // returns that value and moves its binding after the dependencies it imported.
        let modules = self.public_modules()?;
        let pop = self.lookup_attr(modules, "pop")?;
        let loaded = self.call(pop, vec![key], Vec::new())?;
        self.heap.keep(loaded);
        let modules = self.public_modules()?;
        self.put_item(modules, key, loaded)?;
        self.bind_import_child(name, source_globals, loaded)?;
        Ok(Some(loaded))
    }

    fn bind_import_child(&mut self, name: &str, source_globals: Id, loaded: Id) -> Result<()> {
        if let Some((parent_name, child)) = name.rsplit_once('.') {
            let modules = self.public_modules()?;
            let parent_key = self.text(parent_name);
            let parent = self.get_item(modules, parent_key)?;
            if let Err(error) = self.set_attr(parent, child, loaded) {
                if !self.is_attribute_error(&error) {
                    return Err(error);
                }
                let exception = self.raised_instance(error);
                self.with_import_exception(exception, |ctx| {
                    ctx.warn_child_binding(
                        source_globals,
                        &format!(
                            "Cannot set an attribute on {} for child module {}",
                            string_repr(parent_name),
                            string_repr(child)
                        ),
                    )
                })?;
            }
        }
        Ok(())
    }

    fn allocate_import_module(
        &mut self,
        name: &str,
        locations: &[PathBuf],
        source: Option<&(PathBuf, importlib::SourceBody)>,
    ) -> Result<(Id, Id, Id, Option<Id>)> {
        let main = name == "__main__";
        // A package parent runs before the entry acquires its file and qualified spec.
        let packaged_main = main && self.config["packaged_file_module"].is_string();
        // A startup hook can populate or replace __main__. Execute the application
        // in its current namespace, leaving any bindings the hook installed intact.
        if main
            && self.config["sitecustomize"] == true
            && let Some((path, _)) = source
        {
            let key = self.text(name);
            if let Some(module) = self.public_registered_module(key)?
                && let Value::Module { scope, .. } = *self.heap.get(module)
            {
                // The script can delete the registry binding while its initializer is running.
                self.heap.keep(module);
                let origin = if self.config["command"] != true && !packaged_main {
                    let file = self.import_path_text(path)?;
                    if self.scope_name(scope, "__file__").is_none() {
                        self.bind(scope, "__file__", file);
                    }
                    Some(file)
                } else {
                    None
                };
                return Ok((module, scope, key, origin));
            }
        }
        let source = source.filter(|_| !packaged_main);
        let scope = self.scope(name, None);
        // CPython's private module publishes its ABCs under the public name.
        let name_id = self.text(if name == "_collections_abc" {
            "collections.abc"
        } else {
            name
        });
        self.bind(scope, "__name__", name_id);
        if source.is_some() || main {
            self.bind(scope, "__doc__", NONE);
        }
        if !locations.is_empty() {
            let dirs = self.location_list(locations)?;
            self.bind(scope, "__path__", dirs);
        }
        let package = if main {
            NONE
        } else if !locations.is_empty() {
            name_id
        } else {
            self.text(name.rsplit_once('.').map_or("", |(pkg, _)| pkg))
        };
        self.bind(scope, "__package__", package);
        if name == "builtins" {
            self.install_builtins(scope)?;
        } else if source.is_some() || main {
            let builtins = if main {
                self.module("builtins")?
            } else {
                self.default_source_builtins()?
            };
            self.bind(scope, "__builtins__", builtins);
        } else if name == "math" {
            // Re-executing the native loader preserves this module's attribute edits.
            for name in [
                "asin",
                "asinh",
                "atan",
                "atanh",
                "comb",
                "cosh",
                "degrees",
                "erfc",
                "fmod",
                "fabs",
                "factorial",
                "perm",
                "log10",
                "log1p",
                "nextafter",
                "radians",
                "remainder",
                "sinh",
                "tanh",
            ] {
                let function = self.builtin(&format!("math.{name}"));
                self.bind(scope, name, function);
            }
        }
        if main && source.is_none() {
            self.bind(scope, "__spec__", NONE);
            let loader = self.lookup_attr(self.bootstrap_module, "_NativeImporter")?;
            self.bind(scope, "__loader__", loader);
        }
        let origin = match source {
            Some((path, _)) if locations.contains(path) => {
                self.bind(scope, "__file__", NONE);
                None
            }
            Some((path, _)) if !main || self.config["command"] != true => {
                let file = self.import_path_text(path)?;
                self.bind(scope, "__file__", file);
                Some(file)
            }
            _ => None,
        };
        let module = self.alloc(Value::Module {
            name: name.to_owned(),
            scope,
            origin,
            initializing: source.is_some(),
            module_state: None,
            module_slots: None,
            constructor: None,
        });
        match (name, source) {
            ("builtins", _) => {
                self.original_builtins.get_or_insert(module);
            }
            ("_opcode", None) => stdlib::opcode::install(self, module, scope),
            ("_bisect", None) => stdlib::bisect::install(self, module, scope),
            _ => {}
        }
        Ok((module, scope, name_id, origin))
    }

    /// Run an importlib exception handler with its caught exception visible to Python callbacks.
    fn with_import_exception<T>(
        &mut self,
        exception: Id,
        handler: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.heap.reserve_temporaries(1)?;
        let previous = self.heap.replace_active(exception);
        self.heap.keep(previous);
        let result = handler(self);
        self.heap.replace_active(previous);
        result.map_err(|mut error| {
            if error.context == NONE
                && error.trace.as_ref().and_then(|t| t.instance) != Some(exception)
            {
                error.context = exception;
            }
            error
        })
    }

    fn finish_initialization(&mut self, module: Id) {
        if let Value::Module { initializing, .. } = self.heap.get_mut(module) {
            *initializing = false;
        }
    }

    fn run_initializer(
        &mut self,
        name: &str,
        module: Id,
        scope: Id,
        name_id: Id,
        source_globals: Id,
        source: Option<(PathBuf, importlib::SourceBody, Option<Id>)>,
    ) -> Result<()> {
        if name == "posix" && source.is_none() {
            stdlib::os::attributes::initialize(self, scope);
        }
        #[cfg(target_os = "linux")]
        if name == "posix" && source.is_none() {
            stdlib::os::eventfd::initialize(self, scope);
            for name in [
                "major",
                "minor",
                "makedev",
                "uname",
                "getloadavg",
                "ctermid",
                "sync",
            ] {
                let function = self.builtin(&format!("posix.{name}"));
                self.bind(scope, name, function);
            }
            let nonblock = self.int(i64::from(libc::PIDFD_NONBLOCK));
            self.bind(scope, "PIDFD_NONBLOCK", nonblock);
            for (name, flag) in [
                ("TFD_CLOEXEC", libc::TFD_CLOEXEC),
                ("TFD_NONBLOCK", libc::TFD_NONBLOCK),
                ("TFD_TIMER_ABSTIME", libc::TFD_TIMER_ABSTIME),
                ("TFD_TIMER_CANCEL_ON_SET", libc::TFD_TIMER_CANCEL_ON_SET),
            ] {
                let value = self.int(i64::from(flag));
                self.bind(scope, name, value);
            }
            if stdlib::os::fork_available() {
                for attr in ["fork", "register_at_fork"] {
                    let function = self.builtin(&format!("posix.{attr}"));
                    self.bind(scope, attr, function);
                }
            }
        }
        if name == "_thread" && source.is_none() {
            let rlock = self.builtin(thread_rlock::CLASS);
            self.bind(scope, "RLock", rlock);
        }
        #[cfg(target_os = "linux")]
        if name == "posix" && source.is_none() {
            let memfd = self.builtin("posix.memfd_create");
            self.bind(scope, "memfd_create", memfd);
            let close_on_exec = self.int(i64::from(libc::MFD_CLOEXEC));
            self.bind(scope, "MFD_CLOEXEC", close_on_exec);
        }
        if matches!(name, "io" | "_io") {
            let error = self.builtin("BlockingIOError");
            self.bind(scope, "BlockingIOError", error);
        }
        #[cfg(unix)]
        if name == "posix" && source.is_none() {
            for attr in [
                "getuid",
                "geteuid",
                "getgid",
                "getgroups",
                "getpgrp",
                "getegid",
                #[cfg(target_os = "linux")]
                "getresgid",
                #[cfg(target_os = "linux")]
                "getresuid",
            ] {
                let value = self.builtin(&format!("posix.{attr}"));
                self.bind(scope, attr, value);
            }
        }
        match name {
            "_frozen_importlib_external" | "importlib._bootstrap_external" => {
                let finder = if self.finder_external == NONE {
                    module
                } else {
                    self.finder_external
                };
                self.bind(scope, "_finder_external", finder);
            }
            "_imre_importlib" => {
                let compiler = self.builtin("compile");
                self.bind(scope, "builtin_compile", compiler);
                let suffixes = imre_ast::module_suffixes()
                    .into_iter()
                    .map(|suffix| self.heap.owned_text(suffix.into()))
                    .collect();
                let suffixes = self.alloc(Value::Tuple(suffixes));
                self.bind(scope, "module_suffixes", suffixes);
                // Both bootstrap modules import the index used to build the installed archive.
                let members = self.config["stdlib_members"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .map(|member| self.heap.owned_text(member.into()))
                    .collect();
                let members = self.alloc(Value::Tuple(members));
                self.bind(scope, "_installed_source_members", members);
                let archive = self.interpreter_layout.as_ref().map_or_else(
                    |_| self.executable.with_file_name("imre-stdlib.zip"),
                    |layout| layout.stdlib_archive.clone(),
                );
                let archive = self.config["stdlib_fallback"]
                    .as_str()
                    .or_else(|| archive.to_str().filter(|_| archive.is_absolute()))
                    .map_or(NONE, |path| self.heap.owned_text(path.into()));
                self.bind(scope, "_installed_stdlib", archive);
                // All bootstrap instances invalidate the finders used by the native path search.
                if self.zip_path_importers == NONE {
                    self.zip_path_importers = self.dict(Vec::new());
                }
                self.bind(scope, "_zip_path_importers", self.zip_path_importers);
                // Source loaders can compile directly even when ordinary module imports are disabled.
                let index = self.builtin("operator.index");
                self.bind(scope, "index", index);
                let factory = self.builtin(thread_rlock::CLASS);
                let lock = self.call(factory, vec![], vec![])?;
                self.bind(scope, "_import_lock", lock);
            }
            "itertools" => {
                for attr in ["count", "repeat"] {
                    let value = self.builtin(&format!("{name}.{attr}"));
                    self.bind(scope, attr, value);
                }
            }
            "_imre_fractions" => {
                for name in ["_limit_candidates", "_hash_function"] {
                    let value = self.builtin(&format!("_imre_fractions.{name}"));
                    self.bind(scope, name, value);
                }
            }
            "_imre_itertools" => {
                for attr in [
                    "_accumulate_allocate",
                    "_accumulate_next",
                    "_tee_allocate",
                    "_tee_next",
                    "_cycle_allocate",
                    "_cycle_next",
                    "_pairwise_allocate",
                    "_pairwise_next",
                    "_starmap_allocate",
                    "_starmap_next",
                    "_zip_longest_allocate",
                    "_zip_longest_state",
                    "_predicate_allocate",
                    "_predicate_state",
                    "_uses_base_init",
                    "_chain_allocate",
                    "_chain_next",
                ] {
                    let value = self.builtin(&format!("{name}.{attr}"));
                    self.bind(scope, attr, value);
                }
            }
            "inspect" => {
                for &(name, flag) in imre_ast::code_flags::INSPECT_FLAGS {
                    let value = self.int(i64::from(flag));
                    self.bind(scope, name, value);
                }
            }
            "abc" => {
                let meta = self.abc_meta();
                // ABCMeta.__new__ supplies type.__new__'s caller globals even after
                // sys.modules is replaced. Keep the metaclass's defining module alive.
                let Value::Class {
                    scope: meta_scope, ..
                } = *self.heap.get(meta)
                else {
                    unreachable!()
                };
                let Value::Scope(frame) = self.heap.get_mut(meta_scope) else {
                    unreachable!()
                };
                frame.parent.get_or_insert(scope);
                self.bind(scope, "ABCMeta", meta);
                let token = self.builtin("abc.get_cache_token");
                self.bind(scope, "get_cache_token", token);
            }
            #[cfg(target_os = "linux")]
            "fcntl" if source.is_none() => {
                let value = self.int(i64::from(libc::FD_CLOEXEC));
                self.bind(scope, "FD_CLOEXEC", value);
            }
            #[cfg(target_os = "linux")]
            "posix" if source.is_none() => {
                stdlib::posix_spawn::initialize(self, scope);
                stdlib::os::waitid::initialize(self, scope);
                let cpu_count = self.builtin("posix.cpu_count");
                self.bind(scope, "cpu_count", cpu_count);
                stdlib::os::scheduler::initialize(self, scope);
                stdlib::os::sysconf::initialize(self, module, scope);
                for name in [
                    "getrandom",
                    "execve",
                    "get_terminal_size",
                    "lockf",
                    "posix_fadvise",
                    "posix_openpt",
                    "grantpt",
                    "unlockpt",
                    "ptsname",
                    "preadv",
                    "pwritev",
                    "readv",
                    "writev",
                    "splice",
                ] {
                    let function = self.builtin(&format!("posix.{name}"));
                    self.bind(scope, name, function);
                }
                let getgrouplist = self.builtin("posix.getgrouplist");
                self.bind(scope, "getgrouplist", getgrouplist);
                for (attr, flag) in [
                    ("GRND_NONBLOCK", libc::GRND_NONBLOCK),
                    ("GRND_RANDOM", libc::GRND_RANDOM),
                    ("SPLICE_F_MOVE", libc::SPLICE_F_MOVE),
                    ("SPLICE_F_NONBLOCK", libc::SPLICE_F_NONBLOCK),
                    ("SPLICE_F_MORE", libc::SPLICE_F_MORE),
                ] {
                    let value = self.int(i64::from(flag));
                    self.bind(scope, attr, value);
                }
                for (attr, flag) in [
                    ("RWF_APPEND", libc::RWF_APPEND),
                    ("RWF_DSYNC", libc::RWF_DSYNC),
                    ("RWF_SYNC", libc::RWF_SYNC),
                    ("RWF_HIPRI", libc::RWF_HIPRI),
                    ("RWF_NOWAIT", libc::RWF_NOWAIT),
                    ("O_PATH", libc::O_PATH),
                    ("O_NOCTTY", libc::O_NOCTTY),
                    ("O_TMPFILE", libc::O_TMPFILE),
                    ("F_LOCK", libc::F_LOCK),
                    ("F_TLOCK", libc::F_TLOCK),
                    ("F_ULOCK", libc::F_ULOCK),
                    ("F_TEST", libc::F_TEST),
                    ("POSIX_FADV_NORMAL", libc::POSIX_FADV_NORMAL),
                    ("POSIX_FADV_SEQUENTIAL", libc::POSIX_FADV_SEQUENTIAL),
                    ("POSIX_FADV_RANDOM", libc::POSIX_FADV_RANDOM),
                    ("POSIX_FADV_WILLNEED", libc::POSIX_FADV_WILLNEED),
                    ("POSIX_FADV_DONTNEED", libc::POSIX_FADV_DONTNEED),
                    ("POSIX_FADV_NOREUSE", libc::POSIX_FADV_NOREUSE),
                ] {
                    let value = self.int(i64::from(flag));
                    self.bind(scope, attr, value);
                }
                #[cfg(any(target_env = "gnu", target_env = "musl"))]
                for (attr, flag) in [
                    ("RTLD_LAZY", libc::RTLD_LAZY),
                    ("RTLD_NOW", libc::RTLD_NOW),
                    ("RTLD_GLOBAL", libc::RTLD_GLOBAL),
                    ("RTLD_LOCAL", libc::RTLD_LOCAL),
                    ("RTLD_NODELETE", libc::RTLD_NODELETE),
                    ("RTLD_NOLOAD", libc::RTLD_NOLOAD),
                    #[cfg(target_env = "gnu")]
                    ("RTLD_DEEPBIND", libc::RTLD_DEEPBIND),
                ] {
                    let value = self.int(i64::from(flag));
                    self.bind(scope, attr, value);
                }
                stdlib::os::xattr::initialize(self, scope);
            }
            #[cfg(unix)]
            "_signal" if source.is_none() => {
                stdlib::signal::initialize(self, scope);
                for (name, key, value) in [
                    ("SIG_DFL", "_signal.SIG_DFL", libc::SIG_DFL),
                    ("SIG_IGN", "_signal.SIG_IGN", libc::SIG_IGN),
                ] {
                    let value = self.signal_sentinel(key, value);
                    self.bind(scope, name, value);
                }
                for name in ["signal", "getsignal"] {
                    let function = self.builtin(&format!("_signal.{name}"));
                    self.bind(scope, name, function);
                }
                let handler = self.builtin("_signal.default_int_handler");
                self.bind(scope, "default_int_handler", handler);
            }
            #[cfg(unix)]
            "_imre_signal" if source.is_none() => {
                let factory = self.builtin("_imre_signal.make_signals");
                self.bind(scope, "make_signals", factory);
            }
            "_thread" => {
                let handle = self.builtin(thread_handle::CLASS);
                self.bind(scope, "_ThreadHandle", handle);
                for name in ["allocate_lock", "allocate"] {
                    let factory = self.builtin(&format!("_thread.{name}"));
                    self.bind(scope, name, factory);
                }
                let limit = self.alloc(Value::Float(crate::objects::locks::TIMEOUT_MAX));
                self.bind(scope, "TIMEOUT_MAX", limit);
            }
            "threading" if source.is_none() => {
                // The source module imports these aliases itself. The native fallback
                // also stores them in the ordinary module dictionary.
                let lock = self.builtin(thread_lock::CLASS);
                self.bind(scope, "Lock", lock);
                let local = self.builtin(thread_local::CLASS);
                self.bind(scope, "local", local);
                let thread = self.module("_thread")?;
                let error = self.lookup_attr(thread, "error")?;
                self.bind(scope, "ThreadError", error);
                let limit = self.lookup_attr(thread, "TIMEOUT_MAX")?;
                self.bind(scope, "TIMEOUT_MAX", limit);
            }
            "os" => {
                let posix = self.module("posix")?;
                // The bundled Python body performs its own imports. The native-only
                // facade copies the provider's current bindings here instead.
                if source.is_none() && self.builtin_initializer(name).is_none() {
                    self.import_star(scope, posix)?;
                    match self.lookup_attr(posix, "_exit") {
                        Ok(value) => self.bind(scope, "_exit", value),
                        Err(error) if self.is_attribute_error(&error) => (),
                        Err(error) => return Err(error),
                    }
                }
                for &(name, flag) in crate::objects::os::SEEK_ORIGINS {
                    let value = self.int(i64::from(flag));
                    self.bind(scope, name, value);
                }
                #[cfg(not(target_os = "linux"))]
                for name in ["confstr", "uname"] {
                    let value = self.builtin(&format!("os.{name}"));
                    self.bind(scope, name, value);
                }
                #[cfg(unix)]
                stdlib::os::install_capabilities(self, posix, scope)?;
                let path = self.module("posixpath")?;
                let posix_sep = self.lookup_attr(path, "sep")?;
                for (name, alternate) in [("sep", false), ("altsep", true)] {
                    let value = match crate::objects::os_separator(alternate) {
                        Some(sep) if self.str_value(posix_sep)? == sep => posix_sep,
                        Some(sep) => self.text(sep),
                        None => NONE,
                    };
                    self.bind(scope, name, value);
                }
                for name in ["pathsep", "devnull"] {
                    let value = self.lookup_attr(path, name)?;
                    self.bind(scope, name, value);
                }
                let os_name = self.heap.code_name(crate::sys::OS_NAME);
                self.bind(scope, "name", os_name);
                let linesep = self.text(crate::objects::os::LINE_SEPARATOR);
                self.bind(scope, "linesep", linesep);
                self.bind(scope, "path", path);
                self.publish_module("os.path", path)?;
            }
            "posixpath" => {
                let pathsep = self.text(":");
                self.bind(scope, "pathsep", pathsep);
                let devnull = self.text("/dev/null");
                self.bind(scope, "devnull", devnull);
            }
            "random" if source.is_none() => {
                let choices = self.builtin("random._choices");
                self.bind(scope, "_choices", choices);
                if self.scope_name(scope, "__builtins__").is_none() {
                    let builtins = self.default_source_builtins()?;
                    self.bind(scope, "__builtins__", builtins);
                }
            }
            "secrets" if source.is_none() => {
                let random = self.module("random")?;
                let class = self.lookup_attr(random, "SystemRandom")?;
                let instance = self.call(class, Vec::new(), Vec::new())?;
                let randbits = self.lookup_attr(instance, "getrandbits")?;
                self.bind(scope, "SystemRandom", class);
                self.bind(scope, "_sysrand", instance);
                self.bind(scope, "randbits", randbits);
            }
            "math" if source.is_none() => {
                for (name, number) in [
                    ("pi", std::f64::consts::PI),
                    ("e", std::f64::consts::E),
                    ("tau", std::f64::consts::TAU),
                    ("inf", f64::INFINITY),
                    ("nan", f64::NAN.abs()),
                ] {
                    let value = self.alloc(Value::Float(number));
                    self.bind(scope, name, value);
                }
            }
            #[cfg(unix)]
            "_ssl" if source.is_none() => {
                stdlib::ssl::provider::objects::install(self, module, scope)?
            }
            #[cfg(unix)]
            "_imre_ssl" if source.is_none() => stdlib::ssl::install(self, scope)?,
            "_hashlib" if source.is_none() => {
                stdlib::hashlib::install_openssl(self, module, scope)?
            }
            "hmac"
                if source.is_none()
                    || source.as_ref().is_some_and(|(path, _, _)| {
                        self.config["bundled_root"]
                            .as_str()
                            .is_some_and(|root| path == &std::path::Path::new(root).join("hmac.py"))
                    }) =>
            {
                let provider = match self.find_module("_hashlib") {
                    Err(err) if self.is_builtin_error(&err, "ImportError") => None,
                    result => result?,
                };
                let (module, attr) = if let Some(provider) = provider {
                    (provider, "compare_digest")
                } else {
                    let operator = self
                        .find_module("_operator")?
                        .ok_or_else(|| Self::missing_module("_operator", self.text("_operator")))?;
                    (operator, "_compare_digest")
                };
                let compare = self.lookup_attr(module, attr)?;
                self.bind(scope, "compare_digest", compare);
                let new = self.alloc(Value::Native("hmac.new".into()));
                self.bind(scope, "new", new);
                let hmac = self.builtin("hmac.HMAC");
                self.bind(scope, "HMAC", hmac);
                let digest = self.builtin("hmac._digest_native");
                self.bind(scope, "_digest_native", digest);
            }
            "hashlib" => {
                let native_new = self.builtin("hashlib.new");
                self.bind(scope, "_native_new", native_new);
                let readinto = self.builtin("hashlib._file_digest_readinto");
                self.bind(scope, "_file_digest_readinto", readinto);
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            "grp" => stdlib::grp::install(self, scope),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            "pwd" => stdlib::pwd::install(self, scope),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            "termios" if source.is_none() => stdlib::termios::install(self, module, scope)?,
            "_zstd" if source.is_none() => stdlib::zstd::install(self, scope),
            "_bz2" if source.is_none() => {
                for name in ["BZ2Compressor", "BZ2Decompressor"] {
                    let class = self.builtin(&format!("_bz2.{name}"));
                    self.bind(scope, name, class);
                }
            }
            "_lzma" if source.is_none() => {
                for name in [
                    "is_check_supported",
                    "_encode_filter_properties",
                    "_decode_filter_properties",
                ] {
                    let function = self.builtin(&format!("_lzma.{name}"));
                    self.bind(scope, name, function);
                }
                for &(name, value) in imre_ast::lzma::INTEGER_CONSTANTS {
                    let value = self.int(value);
                    self.bind(scope, name, value);
                }
                for name in ["LZMAError", "LZMACompressor", "LZMADecompressor"] {
                    let class = self.builtin(&format!("_lzma.{name}"));
                    self.bind(scope, name, class);
                }
            }
            "unicodedata" if source.is_none() => stdlib::unicodedata::install(self, module, scope)?,
            "_md5" => stdlib::hashlib::md5::install(self, module, scope)?,
            "_sha3" => {
                for name in [
                    "sha3_224",
                    "sha3_256",
                    "sha3_384",
                    "sha3_512",
                    "shake_128",
                    "shake_256",
                ] {
                    let class = crate::objects::HashState::public_constructor(name).unwrap();
                    let constructor = self.builtin(class);
                    self.bind(scope, name, constructor);
                }
                let backend = self.text("RustCrypto");
                self.bind(scope, "implementation", backend);
                let threshold = self.int(2048);
                self.bind(scope, "_GIL_MINSIZE", threshold);
            }
            "_blake2" => {
                for name in ["blake2b", "blake2s"] {
                    let class = crate::objects::HashState::public_constructor(name).unwrap();
                    let constructor = self.builtin(class);
                    self.bind(scope, name, constructor);
                    for attr in [
                        "SALT_SIZE",
                        "PERSON_SIZE",
                        "MAX_KEY_SIZE",
                        "MAX_DIGEST_SIZE",
                    ] {
                        let constant =
                            crate::objects::HashState::class_constant(class, attr).unwrap();
                        let value = self.int(constant as i64);
                        self.bind(
                            scope,
                            &format!("{}_{attr}", name.to_ascii_uppercase()),
                            value,
                        );
                    }
                }
                let threshold = self.int(2048);
                self.bind(scope, "_GIL_MINSIZE", threshold);
            }
            "_datetime" => {
                stdlib::datetime::isocalendar::install(self, module)?;
                for name in ["date", "datetime", "time", "timedelta", "timezone"] {
                    let class = self.builtin(&format!("datetime.{name}"));
                    self.bind(scope, name, class);
                }
                let base = stdlib::datetime::tzinfo(self);
                self.bind(scope, "tzinfo", base);
                let utc = stdlib::datetime::utc(self);
                self.bind(scope, "UTC", utc);
                for (name, year) in [("MINYEAR", 1), ("MAXYEAR", 9999)] {
                    let value = self.int(year);
                    self.bind(scope, name, value);
                }
            }
            "operator" | "_operator" if source.is_none() => {
                stdlib::operator::install(self, module, scope, name);
            }
            "_functools" => {
                for name in ["partial", "Placeholder", "_PlaceholderType"] {
                    let value = stdlib::attribute(self, "functools", name)?;
                    self.bind(scope, name, value);
                }
                let callable = self.builtin("_functools.reduce");
                let reduce = self.alloc(Value::Method {
                    callable,
                    receiver: module,
                    explicit: false,
                });
                self.bind(scope, "reduce", reduce);
            }
            "errno" => stdlib::install_errno(self, scope)?,
            "atexit" => atexit::install(self, scope),
            "faulthandler" => stdlib::faulthandler::install(self, scope),
            "_socket" => stdlib::install_lowlevel_socket(self, module, scope)?,
            "socket"
                if source.is_none()
                    || source.as_ref().is_some_and(|(path, _, _)| {
                        self.config["bundled_root"].as_str().is_some_and(|root| {
                            path == &std::path::Path::new(root).join("socket.py")
                        })
                    }) =>
            {
                stdlib::import_socket_constants(self, scope)?
            }
            "_opcode" => {
                // Imre emits native code and has no adaptive bytecode interpreter.
                let disabled = self.int(0);
                self.bind_source_name(scope, "ENABLE_SPECIALIZATION", disabled)?;
                self.bind_source_name(scope, "ENABLE_SPECIALIZATION_FT", disabled)?;
            }
            "_frozen_importlib" => {
                if self.bootstrap_globals == NONE {
                    self.bootstrap_globals = self.module_globals(scope)?;
                }
                let key = self.text("_frozen_importlib_external");
                let external = if let Some(module) = self.public_registered_module(key)? {
                    module
                } else {
                    self.find_module("_frozen_importlib_external")?
                        .ok_or_else(|| Self::missing_module("_frozen_importlib_external", key))?
                };
                self.bind(scope, "_bootstrap_external", external);
            }
            "io" => stdlib::io::install(self, scope),
            "_json" => {
                for name in ["encode_basestring", "encode_basestring_ascii", "scanstring"] {
                    let callable = self.builtin(&format!("_json.{name}"));
                    let function = self.alloc(Value::Method {
                        callable,
                        receiver: module,
                        explicit: false,
                    });
                    self.bind(scope, name, function);
                }
            }
            "site" => stdlib::initialize_site(self, scope)?,
            "_locale" => stdlib::locale::install(self, module, scope)?,
            "_warnings" => stdlib::warnings_accel::install(self, module, scope)?,
            "_tracemalloc" => stdlib::tracemalloc::install(self, module, scope),
            "time" => {
                let class = self.builtin(stdlib::time::STRUCT_TIME);
                self.bind(scope, "struct_time", class);
                stdlib::time::install_timezone(self, scope)?;
            }
            "struct" => stdlib::structure::install(self, scope)?,
            "marshal" => {
                for name in ["dumps", "loads"] {
                    let function = self.alloc(Value::Native(format!("marshal.{name}")));
                    self.bind(scope, name, function);
                }
                let version = self.int(i64::from(imre_ast::MARSHAL_VERSION));
                self.bind(scope, "version", version);
            }
            "_pickle" => stdlib::pickle::install(self, module, scope)?,
            "urllib.parse" => stdlib::urllib::initialize(self, scope),
            "_io" => stdlib::io::install_native(self, scope, module),
            "ast" => stdlib::ast::install(self, scope),
            #[cfg(target_os = "linux")]
            "mmap" => stdlib::mmap::install(self, scope),
            #[cfg(target_os = "linux")]
            "select" => {
                let epoll = self.builtin("select.epoll");
                self.bind(scope, "epoll", epoll);
                for &(name, mask) in imre_ast::epoll::FLAGS {
                    let value = self.int(i64::from(mask));
                    self.bind(scope, name, value);
                }
            }
            "selectors" => {
                stdlib::readiness::initialize(self, scope)?;
            }
            "sys" => {
                let options: Vec<String> =
                    serde_json::from_value(self.config["xoptions"].clone()).unwrap_or_default();
                let xoptions = self.dict(Vec::new());
                for option in options {
                    let (key, value) = match option.split_once('=') {
                        Some((key, value)) => (key, self.text(value)),
                        None => (option.as_str(), TRUE),
                    };
                    let key = self.text(key);
                    self.put_item(xoptions, key, value)?;
                }
                self.bind(scope, "_xoptions", xoptions);
                if self.threads.system_module.is_none() {
                    let paths: Vec<String> = serde_json::from_value(self.config["paths"].clone())
                        .expect("compiler search paths");
                    // The launcher inserts path[0] separately during startup.
                    let entries = paths
                        .into_iter()
                        .skip(usize::from(self.config["path0"] == true))
                        .map(|path| self.text(path))
                        .collect();
                    let path = self.alloc(Value::List(entries));
                    self.bind(scope, "path", path);
                    self.bind(scope, "modules", self.modules);
                    let version = stdlib::sys_version(self)?;
                    self.bind(scope, "version", version);
                    if let Some(hexversion) = self.config["python_hexversion"].as_i64() {
                        let value = self.int(hexversion);
                        self.bind(scope, "hexversion", value);
                    }
                    let names = self.alloc(Value::Set {
                        entries: Default::default(),
                        frozen: true,
                    });
                    for name in crate::sys::STDLIB_MODULE_NAMES.lines() {
                        let key = self.text(name);
                        builtins::set_add(self, names, key)?;
                    }
                    self.bind(scope, "stdlib_module_names", names);
                }
                let byteorder = self.text(if cfg!(target_endian = "little") {
                    "little"
                } else {
                    "big"
                });
                self.bind(scope, "byteorder", byteorder);
                let float_info = stdlib::fixed_structseq::float_info(self);
                self.bind(scope, "float_info", float_info);
                // floats::render uses shortest round-tripping digits with Python's decimal spelling.
                let float_repr_style = self.text("short");
                self.bind(scope, "float_repr_style", float_repr_style);
                let thread_info = stdlib::fixed_structseq::thread_info(self);
                self.bind(scope, "thread_info", thread_info);
                for &name in stdlib::SYS_EAGER_FUNCTIONS {
                    let callable = self.builtin(&format!("sys.{name}"));
                    self.bind(scope, name, callable);
                }
                for &name in stdlib::SYS_BOUND_FUNCTIONS {
                    let callable = self.builtin(&format!("sys.{name}"));
                    let function = self.alloc(Value::Method {
                        callable,
                        receiver: module,
                        explicit: false,
                    });
                    self.bind(scope, name, function);
                    if name == "displayhook" {
                        self.bind(scope, "__displayhook__", function);
                    } else if name == "excepthook" {
                        self.bind(scope, "__excepthook__", function);
                    } else if name == "unraisablehook" {
                        self.bind(scope, "__unraisablehook__", function);
                        if self.threads.system_module.is_none() {
                            self.threads.default_unraisable_hook = Some(function);
                        }
                    }
                }
                for (name, original, stderr) in [
                    ("stdout", "__stdout__", false),
                    ("stderr", "__stderr__", true),
                ] {
                    let stream = stdlib::io::standard_output(self, stderr)?;
                    self.bind(scope, name, stream);
                    self.bind(scope, original, stream);
                    if self.threads.system_module.is_none() {
                        self.heap.standard_outputs[usize::from(stderr)] = Some(stream);
                        if stderr {
                            self.threads.system_module = Some(module);
                            self.pending_builtin_names_scope = Some(scope);
                        }
                    }
                }
                let stdin = stdlib::io::standard_input(self)?;
                self.bind(scope, "stdin", stdin);
                self.bind(scope, "__stdin__", stdin);
                let jit = stdlib::jit_module(self)?;
                self.bind(scope, "_jit", jit);
                if self.threads.system_module == Some(module) {
                    let monitoring = stdlib::monitoring::module(self)?;
                    self.bind(scope, "monitoring", monitoring);
                }
            }
            "re._constants" => {
                let limit = self.int(i64::from(imre_regex::MAX_GROUPS));
                self.bind(scope, "MAXGROUPS", limit);
            }
            "re" => {
                let pattern = self.builtin(value::REGEX_PATTERN_TYPE);
                self.bind(scope, "Pattern", pattern);
                let match_type = self.builtin(value::REGEX_MATCH_TYPE);
                self.bind(scope, "Match", match_type);
            }
            "array" => {
                let array = self.builtin("array.array");
                self.bind(scope, "array", array);
                let abc = self.module("_collections_abc")?;
                let sequence = self.lookup_attr(abc, "MutableSequence")?;
                let register = self.lookup_attr(sequence, "register")?;
                self.call(register, vec![array], Vec::new())?;
            }
            "_collections" => {
                for (field, class) in [
                    ("deque", deque::CLASS),
                    ("_deque_iterator", deque::ITERATOR),
                    ("_deque_reverse_iterator", deque::REVERSE_ITERATOR),
                ] {
                    let value = self.builtin(class);
                    self.bind(scope, field, value);
                }
            }
            "collections" => {
                let namedtuple = self.builtin("collections.namedtuple");
                self.bind(scope, "_namedtuple_factory", namedtuple);
                self.bind(scope, "namedtuple", namedtuple);
                let abc = self.module("_collections_abc")?;
                self.bind(scope, "abc", abc);
                self.publish_module("collections.abc", abc)?;
            }
            _ => {}
        }
        if name == "_thread" && source.is_none() {
            let error = self.builtin("RuntimeError");
            self.bind(scope, "error", error);
            #[cfg(unix)]
            {
                let interrupt = self.builtin("_thread.interrupt_main");
                self.bind(scope, "interrupt_main", interrupt);
            }
            for function in [
                "start_new_thread",
                "_count",
                "_shutdown",
                "get_ident",
                "exit",
                "exit_thread",
            ] {
                let value = self.builtin(&format!("_thread.{function}"));
                self.bind(scope, function, value);
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let id = self.builtin("_thread.get_native_id");
                self.bind(scope, "get_native_id", id);
            }
        }
        let source = source.or_else(|| {
            self.builtin_initializer(name).map(|body| {
                (
                    body.path.clone(),
                    importlib::SourceBody::Registered(body.code),
                    None,
                )
            })
        });
        if let Some((path, body, origin)) = source {
            let native_random = name == "random"
                && matches!(
                    body,
                    importlib::SourceBody::Registered(ModuleInitializer::Builtin(_))
                );
            let result = match body {
                importlib::SourceBody::Namespace if self.sources.contains_key("importlib") => {
                    self.lookup_attr(module, "__path__").and_then(|locations| {
                        self.namespace_spec(name_id, locations, module, source_globals)
                    })
                }
                importlib::SourceBody::Namespace => Ok(NONE),
                importlib::SourceBody::Zip(_) => self.initialize_source(
                    module,
                    scope,
                    name_id,
                    origin.expect("ZIP source has a filename"),
                    body,
                ),
                importlib::SourceBody::Registered(_)
                    if let Some(origin) = origin
                        && ((matches!(
                            name,
                            "importlib" | "importlib._bootstrap" | "importlib._bootstrap_external"
                        ) && self.bootstrap_file_spec != NONE)
                            || (name != "__main__"
                                && !stdlib::is_module(name)
                                && self.sources.contains_key("importlib")
                                && path.is_absolute())) =>
                {
                    self.initialize_source(module, scope, name_id, origin, body)
                }
                importlib::SourceBody::Registered(entry) => self
                    .module_code(entry, &path)
                    .and_then(|code| self.execute(code, scope, FrameOwner::Module(name_id))),
                importlib::SourceBody::Filesystem => match origin {
                    Some(file) => self.initialize_source(module, scope, name_id, file, body),
                    // Namespace packages have no initializer.
                    None => Ok(NONE),
                },
            };
            self.finish_initialization(module);
            result?;
            if native_random {
                stdlib::random::instance::install(self, scope)?;
            }
            if self.finder_external == NONE
                && matches!(
                    name,
                    "_frozen_importlib_external" | "importlib._bootstrap_external"
                )
            {
                self.finder_external = module;
            }
            if name == "importlib"
                && self.bootstrap_file_spec == NONE
                && let Some(make_spec) = self.scope_name(scope, "_file_spec")
            {
                // Fresh bootstrap imports run while the replacement package is still initializing.
                self.bootstrap_file_spec = make_spec;
            }
            // The private implementation supplies the accelerator's immutable OrderedDict.
            // A separately loaded pure-Python OrderedDict retains its writable namespace.
            if name == "_imre_ordered_dict"
                && self.config["bundled_root"].as_str().is_some_and(|root| {
                    path == std::path::Path::new(root).join("_imre_ordered_dict.py")
                })
                && let Some(owner) = self.scope_name(scope, "OrderedDict")
                && let Value::Class {
                    scope: class_scope, ..
                } = *self.heap.get(owner)
                && self.root_scope(class_scope) == scope
                && let Value::Scope(frame) = self.heap.get_mut(class_scope)
                && let Some(namespace) = &mut frame.class_namespace
            {
                namespace.immutable_type = true;
            }
            if name == "itertools"
                && self.config["bundled_root"]
                    .as_str()
                    .is_some_and(|root| path == std::path::Path::new(root).join("itertools.py"))
            {
                self.freeze_itertools_types(scope);
                for (name, owner) in [
                    ("_tee", "itertools._tee"),
                    ("_TeeBlock", "itertools._TeeBlock"),
                    ("accumulate", "itertools.accumulate"),
                    ("cycle", "itertools.cycle"),
                    ("filterfalse", "itertools.filterfalse"),
                    ("pairwise", "itertools.pairwise"),
                    ("starmap", "itertools.starmap"),
                    ("takewhile", "itertools.takewhile"),
                    ("dropwhile", "itertools.dropwhile"),
                    ("compress", "itertools.compress"),
                    ("zip_longest", "itertools.zip_longest"),
                ] {
                    if let Some(class) = self.scope_name(scope, name)
                        && let Value::Class { scope, .. } = *self.heap.get(class)
                        && let Value::Scope(frame) = self.heap.get_mut(scope)
                        && let Some(layout) = &mut frame.class_namespace
                    {
                        layout.itertools_layout = Some(owner);
                    }
                }
            }
            if name == "itertools"
                && let Some(owner) = self.scope_name(scope, "batched")
            {
                // Keep the iterator state outside Python attribute lookup, even on subclasses.
                let name = self.text("_batched_state");
                let member = self.alloc(Value::SlotMember {
                    owner,
                    name,
                    storage: SlotStorage::Instance,
                });
                self.bind(scope, "_batched_state", member);
            }
            if name == "re"
                && let Some(class) = self.scope_name(scope, "PatternError")
            {
                self.classes.insert("re.PatternError".into(), class);
            }
            if name == "_collections_abc"
                && self.typing.abc_callable_reducer.is_none()
                && self.config["bundled_root"].as_str().is_some_and(|root| {
                    path == std::path::Path::new(root).join("_collections_abc.py")
                })
            {
                self.typing.abc_callable_reducer = self.scope_name(scope, "_reduce_callable_alias");
            }
            if name == "typing"
                && self.typing.callable_factory.is_none()
                && self.config["bundled_root"]
                    .as_str()
                    .is_some_and(|root| path == std::path::Path::new(root).join("typing.py"))
            {
                // A retained Callable still uses its defining module after sys.modules changes.
                self.typing.callable_factory = self.scope_name(scope, "_make_callable_alias");
            }
            if name == "threading"
                && self.config["bundled_root"]
                    .as_str()
                    .is_some_and(|root| path == std::path::Path::new(root).join("threading.py"))
                && let (Some(class), Some(main)) = (
                    self.scope_name(scope, "Thread"),
                    self.scope_name(scope, "_main_thread"),
                )
            {
                self.register_thread_run(class, main);
            }
            if name == "typing"
                && self.config["bundled_root"]
                    .as_str()
                    .is_some_and(|root| path == std::path::Path::new(root).join("typing.py"))
                && let Some(class) = self.scope_name(scope, "_SpecialGenericAlias")
            {
                // Callable retains its defining base even when sys.modules["typing"] changes.
                self.classes
                    .entry("typing._SpecialGenericAlias".into())
                    .or_insert(class);
            }
        }
        Ok(())
    }
}
