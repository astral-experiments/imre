//! Object operations for native Python functions with dynamically loaded modules.
//! The compiler lowers statements, branches, loops and function bodies to LLVM.

mod abstract_class;
mod allocation_traces;
mod array;
mod async_generator;
mod atexit;
mod builtins;
mod calls;
mod class_annotations;
mod collection_pattern;
mod complex;
mod defaultdict;
mod deque;
mod dictionary;
mod displayhook;
mod enums;
mod excepthook;
mod ffi;
mod finalizers;
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
use std::path::PathBuf;

use crate::objects::exceptions::string_repr;
use value::{
    Code, ComprehensionKind, DictIterator, DictViewKind, Error, FALSE, FrameOwner, Heap, Id, NONE,
    Result, Scope, TRUE, Value,
};

struct SourceModule {
    path: PathBuf,
    code: Code,
    bundled_text: Option<&'static [u8]>,
}

enum IteratorSlice<'a> {
    Values(&'a [Id]),
    Array(&'a array::Array, usize),
    Reversed(&'a [Id]),
    Bytes(&'a [u8]),
    ReversedBytes(&'a [u8]),
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
            Self::ReversedText(text) => text.chars().count(),
            Self::Codepoints(points) | Self::ReversedCodepoints(points) => points.len(),
            Self::Array(array, offset) => array.len().saturating_sub(*offset),
        }
    }

    /// Test the byte boundary directly so each character step stays constant time.
    fn is_empty(&self) -> bool {
        match self {
            Self::ReversedText(text) => text.is_empty(),
            _ => self.len() == 0,
        }
    }
}

struct ExposedClassBody {
    mapping: Id,
    annotation_cell: Option<Id>,
}

type SocketDefaultTimeout = std::sync::Arc<std::sync::Mutex<Option<std::time::Duration>>>;

pub(crate) struct Context {
    socket_default_timeout: SocketDefaultTimeout,
    threads: lowlevel_threads::Workers,
    workers: thread_worker::Registry,
    heap: Heap,
    config: serde_json::Value,
    executable: PathBuf,
    helper_executable: String,
    argv: Id,
    sys_path: Id,
    default_encoding: Id,
    audit_hooks: Vec<Id>,
    // The fatal handler borrows its descriptor while Python retains the destination.
    fault_file: Option<Id>,
    exit_callbacks: Vec<Id>,
    finalizing: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // The active stack roots suspended contexts while Context.run executes native code.
    contexts: Vec<Id>,
    // A buffer export pins its bytearray's size while Python callbacks run.
    buffer_exports: Vec<Id>,
    // Active sorts temporarily detach their list storage while Python callbacks run.
    sorting_lists: Vec<Id>,
    // Container ancestors visible to a reentrant Python __repr__.
    repr_ancestors: Vec<Id>,
    repr_sequence_words: Option<usize>,
    // Python alias frames represented by a renderer suspended in a tuple argument callback.
    suspended_typing_frames: usize,
    token_missing: Id,
    native_code_objects: HashMap<usize, Id>,
    source_builtins: Option<Id>,
    zip_directories: HashMap<String, crate::objects::ZipArchive>,
    zip_path_importers: Id,
    codec_entries: stdlib::codecs::Registry,
    warning_state: Option<stdlib::warnings_accel::State>,
    frozen_override: i32,
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
    typing: typing::Registry,
    builtins: HashMap<String, Id>,
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

impl Context {
    fn code_object(&self, id: Id) -> &value::CodeObject {
        let Value::Code(code) = self.heap.get(id) else {
            unreachable!("function must retain its code object")
        };
        code
    }

    fn alloc(&mut self, value: Value) -> Id {
        let finalizer = matches!(&value, Value::Instance { class, .. }
            if self.class_attr(*class, "__del__").is_some());
        let id = self.heap.alloc(value);
        if finalizer {
            self.heap.register_finalizer(id);
        }
        id
    }
    fn text(&mut self, value: impl Into<String>) -> Id {
        self.alloc(Value::Str(value.into()))
    }
    /// Use the ordinary UTF-8 layout whenever all Python code points are scalars.
    fn text_codepoints(&mut self, points: Vec<u32>) -> Id {
        if points.iter().any(|c| (0xd800..=0xdfff).contains(c)) {
            self.alloc(Value::SurrogateStr(points))
        } else {
            self.text(
                points
                    .into_iter()
                    .map(|c| char::from_u32(c).expect("Python code point"))
                    .collect::<String>(),
            )
        }
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
        match (owner, self.heap.get(self.scalar_value(object))) {
            (weakref::PROXY_TYPE | weakref::CALLABLE_PROXY_TYPE, Value::Weak(wr)) => {
                wr.kind.class() == owner
            }
            ("builtin_function_or_method" | "method", _) => {
                self.builtin_type_name(object) == Some(owner)
            }
            ("list", Value::List(_)) | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => {
                true
            }
            ("tuple", Value::Instance { class, .. }) => self.namedtuple_fields(*class).is_some(),
            (_, Value::DictView { .. }) => {
                Self::dict_view_owner(self.type_name(object)) == Some(owner)
            }
            ("types.GenericAlias", Value::GenericAlias(alias)) => alias.is_builtin_alias(),
            ("generic_alias_iterator", Value::GenericAliasIterator(_))
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("float", Value::Float(_))
            | ("mappingproxy", Value::MappingProxy(_))
            | ("super", Value::Super { .. })
            | ("complex", Value::Complex(_))
            | ("str", Value::Str(_) | Value::SurrogateStr(_))
            | ("bytes", Value::Bytes(_))
            | ("bytearray", Value::ByteArray(_))
            | ("array.array", Value::Array(_))
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | (value::REGEX_PATTERN_TYPE, Value::RegexPattern(_))
            | (value::REGEX_MATCH_TYPE, Value::RegexMatch(_))
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

    fn memoryview_owner(owner: Option<Id>) -> Result<Id> {
        owner.ok_or_else(|| {
            Error::new(
                "ValueError",
                "operation forbidden on released memoryview object",
            )
        })
    }

    /// Acquiring a Python-defined buffer can replace the native storage or install a release callback.
    fn check_buffer_exporter(&self, object: Id) -> Result<()> {
        let Value::Instance { class, .. } = self.heap.get(object) else {
            return Ok(());
        };
        let python_slot = |slot| {
            self.find_in_mro(*class, |base| match self.heap.get(base) {
                Value::Class { .. } => self.own_class_attr(base, slot).map(|_| true),
                Value::Native(name)
                    if matches!(name.as_str(), "array.array" | "bytearray")
                        || (name == "bytes" && slot == "__buffer__") =>
                {
                    Some(false)
                }
                _ => None,
            })
        };
        if match python_slot("__buffer__") {
            Some(true) => true,
            Some(false) => python_slot("__release_buffer__") == Some(true),
            None => false,
        } {
            return Err(Error::new(
                "NotImplementedError",
                "Python-defined buffer hooks are unsupported",
            ));
        }
        Ok(())
    }

    fn view_storage_id(&self, owner: Id) -> Id {
        match self.heap.get(owner) {
            Value::BytesIoBuffer { storage, .. } => *storage,
            _ => self.scalar_value(owner),
        }
    }

    fn owner_bytes(&self, owner: Id) -> Result<&[u8]> {
        match self.heap.get(self.view_storage_id(owner)) {
            Value::Bytes(bytes) | Value::ByteArray(bytes) => Ok(bytes),
            Value::Array(a) => Ok(a.as_bytes()),
            Value::RawReadWindow(w) => self.window_bytes(*w),
            Value::BytesIoStorage(Some(file)) => Ok(file.storage()),
            Value::BytesIoStorage(None) => {
                Err(Error::new("ValueError", "I/O operation on closed file."))
            }
            _ => Err(Error::new(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    self.type_name(owner)
                ),
            )),
        }
    }

    fn view_element_count(&self, owner: Id) -> Result<usize> {
        match self.heap.get(self.view_storage_id(owner)) {
            Value::Array(a) => Ok(a.len()),
            Value::BytesIoStorage(Some(file)) => Ok(file.bytes().len()),
            _ => Ok(self.owner_bytes(owner)?.len()),
        }
    }

    fn writable_storage_bytes(&mut self, storage: Id) -> Result<&mut [u8]> {
        Self::mutable_storage_bytes(self.heap.get_mut(storage))
    }

    fn mutable_storage_bytes(value: &mut Value) -> Result<&mut [u8]> {
        match value {
            Value::ByteArray(bytes) => Ok(bytes.as_mut_slice()),
            Value::Array(a) => Ok(a.as_bytes_mut()?),
            Value::BytesIoStorage(Some(file)) => {
                Ok(std::rc::Rc::make_mut(file).storage_mut()?.as_mut_slice())
            }
            _ => Err(Error::new("TypeError", "cannot modify read-only memory")),
        }
    }

    fn buffer_storage(&self, id: Id) -> Result<&Value> {
        let owner = match self.heap.get(id) {
            Value::MemoryView { owner, .. } => Self::memoryview_owner(*owner)?,
            _ => id,
        };
        Ok(self.heap.get(self.view_storage_id(owner)))
    }

    fn bytes_like(&self, id: Id) -> Result<&[u8]> {
        match self.heap.get(self.scalar_value(id)) {
            Value::MemoryView { .. } => self.view_bytes(id),
            Value::BytesIoBuffer { storage, .. } => match self.heap.get(*storage) {
                Value::BytesIoStorage(Some(file)) => Ok(file.bytes()),
                _ => Err(Error::new("ValueError", "I/O operation on closed file.")),
            },
            Value::Bytes(bytes) | Value::ByteArray(bytes) => Ok(bytes),
            _ => Err(Error::new(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    self.type_name(id)
                ),
            )),
        }
    }

    fn native_buffer_bytes(&self, id: Id) -> Result<&[u8]> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Array(array) => Ok(array.as_bytes()),
            _ => self.bytes_like(id),
        }
    }

    fn invalidate_stream_snapshot(&self, owner: Id) {
        if let Value::BytesIoBuffer { stream, .. } = self.heap.get(owner)
            && let Value::Instance {
                memory_stream: Some(stdlib::io::MemoryStream::Bytes(state)),
                ..
            } = self.heap.get(*stream)
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
        Ok(readonly || matches!(self.heap.get(self.view_storage_id(owner)), Value::Bytes(_)))
    }

    fn new_memoryview(&mut self, object: Id) -> Result<Id> {
        self.check_buffer_exporter(object)?;
        let (owner, anonymous, layout) = match *self.heap.get(self.scalar_value(object)) {
            Value::Bytes(_) | Value::ByteArray(_) => (object, false, None),
            Value::MemoryView {
                owner,
                anonymous,
                layout,
                ..
            } => (Self::memoryview_owner(owner)?, anonymous, layout),
            Value::BytesIoBuffer { .. } => {
                let len = self.view_element_count(object)?;
                self.invalidate_stream_snapshot(object);
                (object, false, Some(memoryview::ViewLayout::full(len)))
            }
            Value::Array(ref a) if a.view_layout().0 != "w" => (object, false, None),
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
            anonymous,
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
        if !matches!(
            self.heap.get(input),
            Value::ByteArray(_)
                | Value::Array(_)
                | Value::MemoryView { .. }
                | Value::BytesIoBuffer { .. }
                | Value::BytesIoStorage(_)
        ) {
            return convert(self);
        }
        self.buffer_exports.push(input);
        let result = convert(self);
        self.buffer_exports.pop();
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
        heap.has_live_buffer_view(storage, roots)
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
    fn as_int(&self, id: Id) -> Result<crate::Int> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Int(n) => Ok(n.clone()),
            Value::Bool(b) => Ok(crate::Int::from(i64::from(*b))),
            _ => Err(Error::new("TypeError", "expected an integer")),
        }
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
                | Value::SequenceIterator { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::Scandir(_)
                | Value::DictIterator(_)
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
                | Value::TextFile(_)
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
                Value::TextFile(file) => file.closed(),
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
            Value::Array(_) => (id, "array.arrayiterator"),
            Value::List(_) => (id, "list_iterator"),
            Value::Tuple(_) => (id, "tuple_iterator"),
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
        let value = self.heap.get(source);
        if matches!(
            value,
            Value::TemplateIter { .. }
                | Value::GenericAliasIterator(_)
                | Value::SequenceIterator { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::Scandir(_)
                | Value::DictIterator(_)
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
                | Value::TextFile(_)
                | Value::Enumerate { .. }
        ) || value.instance_class().is_some()
            || value.is_weak_proxy()
        {
            return Ok(self.alloc(Value::LoopIterator {
                source,
                peeked: None,
            }));
        }
        Ok(source)
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
            Value::List(values) | Value::Tuple(values) if *reverse => {
                IteratorSlice::Reversed(values.get(..*offset).unwrap_or_default())
            }
            Value::List(values) | Value::Tuple(values) => {
                IteratorSlice::Values(values.get(*offset..).unwrap_or_default())
            }
            Value::SurrogateStr(points) if *reverse => {
                IteratorSlice::ReversedCodepoints(&points[..*offset])
            }
            Value::SurrogateStr(points) => IteratorSlice::Codepoints(&points[*offset..]),
            Value::Array(array) => IteratorSlice::Array(array, *offset),
            Value::Str(text) if *reverse => IteratorSlice::ReversedText(&text[..*offset]),
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
            return stdlib::os::scandir::advance(self, scan.clone());
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
            Value::Generator { code, .. } => {
                let code = *code;
                // Delegated generators resume their children on the native stack.
                return stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    match self.code_object(code).body {
                        value::Body::Native(_) => self.next_generator_item(id),
                        value::Body::Source(_) | value::Body::PendingGenerator { .. } => {
                            self.resume_source(id, NONE)
                        }
                    }
                });
            }
            Value::LoopIterator { peeked, .. } => return Ok(peeked.take()),
            Value::BufferedRandom(..) | Value::File(_) | Value::TextFile(_) => {
                let readline = self.lookup_attr(id, "readline")?;
                let line = self.call(readline, Vec::new(), Vec::new())?;
                return Ok(self.truth(line)?.then_some(line));
            }
            Value::CallableIterator(None) => return Ok(None),
            Value::CallableIterator(Some(state)) => {
                let [callable, sentinel] = *state;
                return self.next_callable_item(id, callable, sentinel);
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
        if !self.iterator_has_item(id)? {
            return Ok(None);
        }
        let (item, width) = match self.iterator_remaining(id)? {
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
                *offset += 1;
            }
        }
        Ok(Some(item))
    }

    /// Consume the values required by an assignment, probing one extra item for fixed targets.
    fn unpack_items(&mut self, value: Id, count: usize, star: Option<usize>) -> Result<Vec<Id>> {
        let required = count - usize::from(star.is_some());
        let prefix = star.unwrap_or(count);
        let iter = self.iterator(value)?;
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
    fn tuple_items(&self, id: Id) -> Result<Option<Vec<Id>>> {
        if let Value::Tuple(items) | Value::NamedTuple { values: items, .. } =
            self.heap.get(self.scalar_value(id))
        {
            return copy_ids(items).map(Some);
        }
        if let Value::Instance { class, fields, .. } = self.heap.get(id)
            && let Some(names) = self.namedtuple_fields(*class)
        {
            let mut items = Vec::new();
            items
                .try_reserve_exact(names.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            items.extend(names.iter().map(|name| fields[name]));
            return Ok(Some(items));
        }
        Ok(None)
    }

    /// Release the previous iteration's operands before collecting and advancing the iterator.
    fn comprehension_step(&mut self, mark: usize) {
        self.heap.release_temporaries_since(mark);
        self.heap.reserve_slot_or_request_collection();
        self.collect_garbage();
    }

    /// Keep yielded values in a traced sequence while discarding each step's other temporaries.
    fn collect_generator(&mut self, generator: Id, tuple: bool) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        self.heap.reserve_temporaries(2)?;
        self.heap.keep(generator);
        let buffer = self.alloc(Value::List(Vec::new()));
        let step = self.heap.temporary_mark();
        let result: Result<()> = (|| {
            while let Some(item) = self.next_item(generator)? {
                let Value::List(items) = self.heap.get_mut(buffer) else {
                    unreachable!()
                };
                items
                    .try_reserve(1)
                    .map_err(|_| crate::Error::memory_error())?;
                items.push(item);
                self.comprehension_step(step);
            }
            Ok(())
        })();
        self.heap.release_temporaries_since(mark);
        if let Err(error) = result {
            for root in error.roots() {
                self.heap.keep(root);
            }
            return Err(error);
        }
        if tuple {
            let Value::List(items) = self.heap.get_mut(buffer) else {
                unreachable!()
            };
            let items = std::mem::take(items);
            return Ok(self.alloc(Value::Tuple(items)));
        }
        self.heap.keep(buffer);
        Ok(buffer)
    }

    fn items(&mut self, id: Id) -> Result<Vec<Id>> {
        if matches!(self.heap.get(id), Value::TypeVarTuple { .. }) {
            return Ok(vec![typing::unpack_parameter(self, id)?]);
        }
        if matches!(self.heap.get(id), Value::Generator { .. }) {
            let buffer = self.collect_generator(id, false)?;
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
                | Value::SequenceIterator { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::Scandir(_)
                | Value::DictIterator(_)
                | Value::DictView { .. }
                | Value::Zip { .. }
                | Value::CallableIterator(_)
                | Value::FormatIterator { .. }
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::BufferedRandom(..)
                | Value::File(_)
                | Value::TextFile(_)
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
            if let Some(ordered) = self.float_enum_names(id) {
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
        let closure = Default::default();
        self.scope_with_closure(module.into(), parent, closure)
    }

    /// Allocate a scope using closure storage prepared by its caller.
    fn scope_with_closure(
        &mut self,
        module: String,
        parent: Option<Id>,
        closure: allocator_api2::boxed::Box<value::ClosureScope>,
    ) -> Id {
        self.alloc(Value::Scope(Scope {
            class_body: None,
            enum_namespace: None,
            is_abstract: false,
            comprehension: None,
            class_cell: None,
            class_namespace: None,
            first_argument: None,
            names: Default::default(),
            closure,
            namespace: None,
            globals: Default::default(),
            exec_globals: None,
            exec_locals: None,
            parent,
            module,
        }))
    }
    /// Comprehensions own their loop variables, but assignment expressions bind outside them.
    fn comprehension_scope(&mut self, parent: Id, targets: Id) -> Result<Id> {
        let targets = self
            .str_value(targets)?
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect();
        Ok(self.comprehension_scope_with_targets(parent, targets))
    }

    fn comprehension_scope_with_targets(
        &mut self,
        parent: Id,
        targets: std::collections::HashSet<String>,
    ) -> Id {
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
        frame.closure.declared = targets;
        frame.closure.declared_complete = true;
        scope
    }

    /// A walrus binds in the first enclosing code unit outside any eager comprehension.
    fn named_expression_scope(&self, mut scope: Id) -> Id {
        while self.scope_data(scope).comprehension.is_some() {
            scope = self
                .scope_data(scope)
                .parent
                .expect("enclosing comprehension scope");
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
        scope.names.get(name).copied()
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
            let mut entries = Vec::with_capacity(members.size_hint().0 + 1);
            for name in members.chain(["__module__"]) {
                let key = self.heap.code_name(name);
                let value = self.lookup_attr(class, name)?;
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
        let names: indexmap::IndexMap<String, Id> = self
            .dict_entries(body)
            .expect("class body dictionary")
            .iter()
            .filter_map(|&(key, value)| {
                self.str_value(key)
                    .ok()
                    .filter(|&name| annotation_cell.is_none() || name != "__classdictcell__")
                    .map(|name| (name.to_owned(), value))
            })
            .collect();
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_body = Some(names.keys().cloned().collect());
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

    /// Materialize once and keep that exact dict as the module's binding storage.
    fn module_globals(&mut self, scope: Id) -> Result<Id> {
        let scope = self.root_scope(scope);
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
            return Ok(self.heap.replace_cell(cell, None).is_some());
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
        // Deleting the class dictionary entry can collect the removed value.
        self.heap.weak_edge_removed(previous);
        if name != "__qualname__"
            && let Some(dictionary) = dictionary
        {
            self.delete_item(dictionary, key)?;
        }
        Ok(true)
    }

    /// Bind a parameter in a fresh function scope using checked name and table storage.
    fn bind_argument(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        let name = copy_label(name)?;
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
            self.heap.replace_cell(cell, Some(value));
            return;
        }
        if let Some(namespace) = self.scope_data(scope).binding_namespace(name) {
            let key = self.heap.code_name(name);
            let storage = self.dict_storage(namespace).expect("namespace dictionary");
            self.put_item(storage, key, value)
                .expect("string key in namespace dictionary");
            return;
        }
        let Value::Scope(s) = self.heap.get_mut(scope) else {
            unreachable!("compiled scope handle")
        };
        if let Some(order) = &mut s.class_body
            && !s.names.contains_key(name)
        {
            order.push(name.to_owned());
        }
        let dictionary = s
            .class_namespace
            .as_ref()
            .and_then(|class| class.dictionary);
        let previous = s.names.insert(name.to_owned(), value);
        if let Some(dictionary) = dictionary
            && (s.class_body.is_some() || name != "__qualname__")
        {
            let key = self.text(name);
            self.put_item(dictionary, key, value)
                .expect("class dictionary has string keys");
        }
        if let Some(previous) = previous
            && previous != value
        {
            self.heap.weak_edge_removed(previous);
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

    /// A class body can insert arbitrary keys into its live namespace through locals().
    fn lookup_scope_name(&mut self, scope: Id, name: &str) -> Result<Option<Id>> {
        if let Some(dict) = self.scope_data(scope).class_body_mapping() {
            let key = self.heap.code_name(name);
            let (_, index) = self.find_dictionary_key(dict, key, None)?;
            return Ok(index.map(|i| self.stored_entries(dict)[i].1));
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
            if s.closure.cells.contains_key(name) {
                return self
                    .scope_name(scope, name)
                    .ok_or_else(|| self.missing_binding(origin, scope, name));
            }
            if let Some(globals) = s.exec_globals {
                return self.source_global(globals, name);
            }
            if let Some(id) = self.lookup_scope_name(scope, name)? {
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
        let key = self.heap.code_name(name);
        let (_, index) = self.find_dictionary_key(builtins, key, None)?;
        index
            .map(|index| self.stored_entries(builtins)[index].1)
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
        if name == "object" {
            let descriptor = self.alloc(Value::LayoutDescriptor(value, "__class__"));
            self.builtins.insert("object.__class__".into(), descriptor);
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
        if name == "coroutine" {
            let origin = self.readonly_member("coroutine", "cr_origin");
            let key = self.heap.code_name("cr_origin");
            let dictionary = self.dict(vec![(key, origin)]);
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
        if builtins::is_exception(name) {
            self.builtin(&format!("{}.__new__", builtins::exception_new_owner(name)));
            self.builtin(builtins::exception_initializer(name));
            self.builtin("BaseException.add_note");
            self.builtin("BaseException.with_traceback");
        }
        if let Some(symbol) = object_size::descriptor(name) {
            self.builtin(symbol);
        }
        let methods = match name {
            "type" => &["mro", "__prepare__"][..],
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
            "function" => &["__call__"],
            "builtin_function_or_method" | "method" => &["__reduce__"],
            defaultdict::CLASS => {
                self.builtin("dict");
                defaultdict::METHODS
            }
            "list" => builtins::LIST_METHODS,
            "method_descriptor" | "wrapper_descriptor" | "classmethod_descriptor" => &["__get__"],
            "tuple" => &["__repr__", "__getitem__", "__len__", "count", "index"],
            "set" | "frozenset" => builtins::set_methods(name),
            "bytearray" => {
                self.builtin("bytearray.fromhex");
                builtins::BYTEARRAY_METHODS
            }
            "property" => property::METHODS,
            "memoryview" => builtins::MEMORYVIEW_METHODS,
            "bool" => &["__repr__"],
            "int" => {
                for field in ["numerator", "denominator"] {
                    let get = self.builtin(&format!("_int_getset.{field}"));
                    let descriptor = self.alloc(Value::GetSetDescriptor {
                        owner: "int",
                        name: field,
                        get,
                        set: None,
                    });
                    self.builtins.insert(format!("int.{field}"), descriptor);
                }
                &[
                    "__add__",
                    "__index__",
                    "bit_length",
                    "from_bytes",
                    "to_bytes",
                    "__format__",
                    "__repr__",
                ]
            }
            "float" => &[
                "__format__",
                "__getformat__",
                "as_integer_ratio",
                "__repr__",
            ],
            "complex" => &["__complex__"],
            "bytes" => {
                self.builtin("bytes.fromhex");
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
        for method in methods
            .iter()
            .chain(builtins::set_operators(name))
            .chain(builtins::real_methods(name))
        {
            self.builtin(&format!("{name}.{method}"));
        }
        value
    }
    fn execute(&mut self, code: Id, scope: Id, owner: FrameOwner) -> Result<Id> {
        let entry = match &self.code_object(code).body {
            value::Body::Native(entry) => *entry,
            value::Body::Source(_) | value::Body::PendingGenerator { .. } => {
                return self.call_source(code, scope, owner);
            }
        };
        self.heap.enter(scope, code, owner);
        // SAFETY: a compiler-generated initializer or function uses this ABI and
        // returns before this Context can move or outlive the native module.
        let result = unsafe { entry(self, scope) };
        self.leave_frame(result, self.pending.is_none());
        self.pending.take().map_or(Ok(result), Err)
    }

    /// Resolve a named import from a loaded module, including package children.
    fn import_from(&mut self, object: Id, member: Id) -> Result<Id> {
        let name = self.str_value(member)?.to_owned();
        let attribute_error = match self.lookup_named_attr(object, &name, Some(member)) {
            Err(error) if self.is_attribute_error(&error) => error,
            value => return value,
        };
        // The importer has already attempted package children. A circular import
        // can publish a child in sys.modules before binding it on the parent.
        let module_name = match self.lookup_attr(object, "__name__") {
            Ok(value) => Some(value),
            Err(err) if self.is_attribute_error(&err) => None,
            Err(err) => return Err(err),
        };
        if let Some(module) = module_name.and_then(|id| self.str_value(id).ok()) {
            let child = self.text(format!("{module}.{name}"));
            if let Some(value) = self.registered_module(child)? {
                return Ok(value);
            }
        }
        let Value::Module {
            scope,
            origin,
            initializing,
            ..
        } = self.heap.get(object)
        else {
            return Err(attribute_error);
        };
        let module = module_name.and_then(|id| self.str_value(id).ok());
        let quoted_module = string_repr(module.unwrap_or("<unknown module name>"));
        let origin = origin.or_else(|| self.scope_name(*scope, "__file__"));
        let path = origin.and_then(|id| self.str_value(id).ok());
        let prefix = format!(
            "cannot import name {} from {quoted_module}",
            self.repr(member)
        );
        let shadow = path.filter(|path| self.possibly_shadowing(path));
        let message = match shadow {
            Some(path)
                if module.is_some_and(|module| {
                    include_str!("../../native/stdlib-module-names.txt")
                        .lines()
                        .any(|name| name == module)
                }) =>
            {
                format!(
                    "{prefix} (consider renaming {} since it has the same name as the standard library module named {quoted_module} and prevents importing that standard library module)",
                    string_repr(path)
                )
            }
            Some(path) if *initializing => {
                format!(
                    "{prefix} (consider renaming {} if it has the same name as a library you intended to import)",
                    string_repr(path)
                )
            }
            _ if *initializing => {
                let message = format!(
                    "cannot import name {} from partially initialized module {quoted_module} (most likely due to a circular import)",
                    self.repr(member)
                );
                match path {
                    Some(path) => format!("{message} ({path})"),
                    None => message,
                }
            }
            _ => format!("{prefix} ({})", path.unwrap_or("unknown location")),
        };
        Err(Error::new("ImportError", message).with_import(
            module,
            path.map(str::to_owned),
            Some(&name),
        ))
    }

    /// Check whether the script's startup search path could shadow an installed module.
    fn possibly_shadowing(&self, path: &str) -> bool {
        if self.config["safe_path"].as_bool() == Some(true) {
            return false;
        }
        let path = std::path::Path::new(path);
        let root = if path.ends_with("__init__.py") {
            path.parent().and_then(std::path::Path::parent)
        } else {
            path.parent()
        };
        let first = self.config["paths"]
            .get(0)
            .and_then(serde_json::Value::as_str);
        let Some((root, first)) = root.zip(first) else {
            return false;
        };
        let first = if first.is_empty() {
            std::env::current_dir()
        } else {
            std::path::absolute(first)
        };
        first.is_ok_and(|first| first == root)
    }

    fn missing_module(name: &str) -> Error {
        Error::new(
            "ModuleNotFoundError",
            format!("No module named {}", string_repr(name)),
        )
        .with_import(Some(name), None, None)
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

    fn public_module_get(&mut self, name: Id, default: Option<Id>) -> Result<Id> {
        let modules = self.public_modules()?;
        if matches!(self.heap.get(modules), Value::Dict(_)) {
            let (_, index) = self.find_dictionary_key(modules, name, None)?;
            return Ok(index.map_or(default.unwrap_or(NONE), |i| {
                self.stored_entries(modules)[i].1
            }));
        }
        let get = self.lookup_attr(modules, "get")?;
        let args = match default {
            Some(default) => vec![name, default],
            None => vec![name],
        };
        self.call(get, args, Vec::new())
    }

    fn public_registered_module(&mut self, name: Id) -> Result<Option<Id>> {
        let value = self.public_module_get(name, Some(self.import_missing))?;
        Ok((value != self.import_missing).then_some(value))
    }

    fn module_initializing(&mut self, module: Id) -> Result<bool> {
        match self
            .lookup_attr(module, "__spec__")
            .and_then(|spec| self.lookup_attr(spec, "_initializing"))
        {
            Ok(flag) => self.truth(flag),
            Err(error) if self.is_attribute_error(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn cached_import(&mut self, name: &str) -> Result<Option<Id>> {
        let key = self.text(name);
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
            if self.public_module_get(key, None)? == module {
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
            .with_import(Some(name), None, None)),
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
        if let Some(module) = self.registered_module(key)?
            && module != NONE
        {
            return Ok(module);
        }
        self.find_module(name)?
            .ok_or_else(|| Self::missing_module(name))
    }

    /// Resolve registered and filesystem modules against the current import path.
    fn find_module(&mut self, name: &str) -> Result<Option<Id>> {
        self.find_module_from(name, NONE)
    }

    fn find_module_from(&mut self, mut name: &str, source_globals: Id) -> Result<Option<Id>> {
        let mut parents = Vec::new();
        let mut module = loop {
            if let Some(id) = self.cached_import(name)? {
                break Some(id);
            }
            let Some((parent, child)) = name.rsplit_once('.') else {
                break self.load_module(name, None, source_globals)?;
            };
            // Each unresolved parent consumes three importlib frames.
            self.check_call_depth(3 * (parents.len() + 1))?;
            parents
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            parents.push((name, parent, child));
            name = parent;
        };
        for (name, parent, child) in parents.into_iter().rev() {
            let id = module.ok_or_else(|| Self::missing_module(parent))?;
            module = self.load_module(name, Some((id, child)), source_globals)?;
        }
        Ok(module)
    }

    /// Run a module initializer after its parent has been resolved.
    fn load_module(
        &mut self,
        name: &str,
        parent: Option<(Id, &str)>,
        source_globals: Id,
    ) -> Result<Option<Id>> {
        if parent.is_some() {
            let key = self.text(name);
            let child = self.public_module_get(key, None)?;
            if child != NONE {
                return Ok(Some(child));
            }
        }
        if self.import_registry_installed {
            let path = match parent {
                Some((parent, _)) => self.parent_import_path(name, parent)?,
                None => NONE,
            };
            let name_id = self.text(name);
            let spec = self.meta_call(source_globals, "_find_spec", vec![name_id, path])?;
            if spec == NONE {
                return Ok(None);
            }
            let module = self.meta_call(source_globals, "_load_unlocked", vec![spec])?;
            self.bind_import_child(name, source_globals, module)?;
            return Ok(Some(module));
        }
        // Bootstrap initializers and live sys.modules hooks can reenter Python.
        stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
            self.load_bootstrap_module(name, parent, source_globals)
        })
    }

    /// Before installing the registry, execute its dependencies and the script's explicit entry.
    fn load_bootstrap_module(
        &mut self,
        name: &str,
        parent: Option<(Id, &str)>,
        source_globals: Id,
    ) -> Result<Option<Id>> {
        let mut locations = Vec::new();
        let source = if name == "__main__" {
            self.sources
                .get(name)
                .and_then(|sources| sources.first())
                .map(|m| (m.path.clone(), importlib::SourceBody::Compiled(m.code)))
        } else if stdlib::is_module(name) || self.is_frozen(name) {
            if matches!(name, "email" | "importlib" | "json" | "re" | "urllib") {
                locations.push(PathBuf::from(format!("<imre stdlib>/{name}")));
            } else if name == "collections" {
                locations.push(stdlib::package_path(self, name)?);
            }
            // A sealed namespace may have both native operations and bundled Python classes.
            // Only its bundled initializer may extend it, never a same-named file on sys.path.
            self.sources.get(name).and_then(|sources| {
                let root = std::path::Path::new(self.config["bundled_root"].as_str()?);
                sources
                    .iter()
                    .find(|source| source.path.starts_with(root))
                    .map(|source| {
                        (
                            source.path.clone(),
                            importlib::SourceBody::Compiled(source.code),
                        )
                    })
            })
        } else {
            let (search, child) = if let Some((parent, child)) = parent {
                (self.parent_import_path(name, parent)?, child)
            } else {
                (self.sys_path, name)
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
            // The executing script remains importable during interpreter shutdown.
            if name == "__main__" {
                return Err(error);
            }
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
        // A script can delete its own __main__ binding. Its exit status is determined by the script.
        if name == "__main__" {
            return Ok(Some(module));
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
        let scope = self.scope(name, None);
        // CPython's private module publishes its ABCs under the public name.
        let name_id = self.text(if name == "_collections_abc" {
            "collections.abc"
        } else {
            name
        });
        self.bind(scope, "__name__", name_id);
        if source.is_some() {
            self.bind(scope, "__doc__", NONE);
        } else if let Some(doc) = self.config["module_docs"].get(name) {
            let doc = doc
                .as_str()
                .map(str::to_owned)
                .map_or(NONE, |text| self.text(text));
            self.bind(scope, "__doc__", doc);
        }
        if !locations.is_empty() {
            let dirs = locations
                .iter()
                .map(|p| self.text(p.to_string_lossy()))
                .collect();
            let dirs = self.alloc(Value::List(dirs));
            self.bind(scope, "__path__", dirs);
        }
        let package = if name == "__main__" {
            NONE
        } else if !locations.is_empty() {
            name_id
        } else {
            self.text(name.rsplit_once('.').map_or("", |(pkg, _)| pkg))
        };
        self.bind(scope, "__package__", package);
        if name == "builtins" {
            self.install_builtins(scope)?;
        } else if source.is_some() {
            let builtins = if name == "__main__" {
                self.module("builtins")?
            } else {
                self.default_source_builtins()?
            };
            self.bind(scope, "__builtins__", builtins);
        }
        let origin = source.and_then(|(path, _)| {
            if locations.contains(path) {
                self.bind(scope, "__file__", NONE);
            } else if name != "__main__" || !self.config["command"].as_bool().unwrap_or(false) {
                let file = self.text(path.to_string_lossy());
                self.bind(scope, "__file__", file);
                return Some(file);
            }
            None
        });
        let module = self.alloc(Value::Module {
            name: name.to_owned(),
            scope,
            origin,
            initializing: source.is_some(),
            module_state: None,
            constructor: None,
        });
        Ok((module, scope, name_id, origin))
    }

    /// Run an importlib exception handler with its caught exception visible to Python callbacks.
    fn with_import_exception(
        &mut self,
        exception: Id,
        handler: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
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

    fn run_initializer(
        &mut self,
        name: &str,
        module: Id,
        scope: Id,
        name_id: Id,
        source_globals: Id,
        source: Option<(PathBuf, importlib::SourceBody, Option<Id>)>,
    ) -> Result<()> {
        #[cfg(target_os = "linux")]
        if source.is_none() && matches!(name, "os" | "posix") {
            for name in ["major", "minor", "makedev", "mknod"] {
                let function = self.builtin(&format!("posix.{name}"));
                self.bind(scope, name, function);
            }
            stdlib::os::eventfd::initialize(self, scope);
        }
        if name == "_thread" && source.is_none() {
            let rlock = self.builtin(thread_rlock::CLASS);
            self.bind(scope, "RLock", rlock);
        }
        #[cfg(target_os = "linux")]
        if source.is_none() && matches!(name, "os" | "posix") {
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
        if source.is_none() && matches!(name, "os" | "posix") {
            let posix = if name == "os" {
                Some(self.module("posix")?)
            } else {
                None
            };
            #[cfg(target_os = "linux")]
            stdlib::posix_spawn::initialize(self, scope, posix)?;
            for attr in [
                "fchdir",
                "chown",
                "fchown",
                "lchown",
                "getuid",
                "geteuid",
                "getgid",
                "getgroups",
                "getpgrp",
            ] {
                let value = match posix {
                    Some(posix) => match self.lookup_attr(posix, attr) {
                        Ok(value) => value,
                        Err(error) if self.is_attribute_error(&error) => continue,
                        Err(error) => return Err(error),
                    },
                    None => self.builtin(&format!("posix.{attr}")),
                };
                self.bind(scope, attr, value);
            }
            for attr in [
                "fsync",
                #[cfg(target_os = "linux")]
                "fdatasync",
                #[cfg(target_os = "linux")]
                "tcgetpgrp",
                #[cfg(target_os = "linux")]
                "ttyname",
                "getegid",
                #[cfg(target_os = "linux")]
                "getresgid",
                #[cfg(target_os = "linux")]
                "getresuid",
            ] {
                let value = match posix {
                    Some(module) => match self.lookup_attr(module, attr) {
                        Ok(value) => value,
                        Err(error) if self.is_attribute_error(&error) => continue,
                        Err(error) => return Err(error),
                    },
                    None => self.builtin(&format!("posix.{attr}")),
                };
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
            "_imre_itertools" => {
                for attr in [
                    "_accumulate_allocate",
                    "_accumulate_state",
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
                    "_chain_allocate",
                    "_chain_next",
                    "_chain_set",
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
                let cpu_count = self.builtin("posix.cpu_count");
                self.bind(scope, "cpu_count", cpu_count);
                stdlib::os::scheduler::initialize(self, scope);
                stdlib::os::sysconf::initialize(self, module, scope);
                for name in [
                    "getrandom",
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
                for name in ["allocate_lock", "allocate"] {
                    let factory = self.builtin(&format!("_thread.{name}"));
                    self.bind(scope, name, factory);
                }
                let limit = self.alloc(Value::Float(crate::objects::locks::TIMEOUT_MAX));
                self.bind(scope, "TIMEOUT_MAX", limit);
            }
            "threading" if source.is_none() => {
                let thread = self.module("_thread")?;
                let error = self.lookup_attr(thread, "error")?;
                self.bind(scope, "ThreadError", error);
                let limit = self.lookup_attr(thread, "TIMEOUT_MAX")?;
                self.bind(scope, "TIMEOUT_MAX", limit);
            }
            "os" => {
                #[cfg(target_os = "linux")]
                if source.is_none() {
                    let posix = self.module("posix")?;
                    for attr in stdlib::os::scheduler::SCHEDULER_EXPORTS
                        .into_iter()
                        .chain(["sched_getscheduler", "sched_param"])
                        .chain(stdlib::os::scheduler::POLICIES.map(|(name, _)| name))
                    {
                        match self.lookup_attr(posix, attr) {
                            Ok(value) => self.bind(scope, attr, value),
                            Err(error) if self.is_attribute_error(&error) => continue,
                            Err(error) => return Err(error),
                        }
                    }
                    for attr in [
                        "getrandom",
                        "GRND_NONBLOCK",
                        "GRND_RANDOM",
                        "get_terminal_size",
                        "posix_openpt",
                        "grantpt",
                        "unlockpt",
                        "ptsname",
                        "preadv",
                        "pwritev",
                        "readv",
                        "writev",
                        "RWF_APPEND",
                        "RWF_DSYNC",
                        "RWF_SYNC",
                        "RWF_HIPRI",
                        "RWF_NOWAIT",
                        "O_NOCTTY",
                        "splice",
                        "SPLICE_F_MOVE",
                        "SPLICE_F_NONBLOCK",
                        "SPLICE_F_MORE",
                        "getxattr",
                        "setxattr",
                        "listxattr",
                        "removexattr",
                        "XATTR_CREATE",
                        "XATTR_REPLACE",
                        "XATTR_SIZE_MAX",
                    ] {
                        let value = self.lookup_attr(posix, attr)?;
                        self.bind(scope, attr, value);
                    }
                    match self.lookup_attr(posix, "getgrouplist") {
                        Ok(value) => self.bind(scope, "getgrouplist", value),
                        Err(error) if self.is_attribute_error(&error) => (),
                        Err(error) => return Err(error),
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let posix = self.module("posix")?;
                    for attr in [
                        "confstr",
                        "confstr_names",
                        "fpathconf",
                        "pathconf",
                        "pathconf_names",
                    ] {
                        let value = self.lookup_attr(posix, attr)?;
                        self.bind(scope, attr, value);
                    }
                }
                #[cfg(unix)]
                stdlib::os::install_capabilities(self, module, scope)?;
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
            "math" if source.is_none() => {
                let pi = self.alloc(Value::Float(std::f64::consts::PI));
                self.bind(scope, "pi", pi);
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
                        .ok_or_else(|| Self::missing_module("_operator"))?;
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
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            "grp" => stdlib::grp::install(self, scope),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            "pwd" => stdlib::pwd::install(self, scope),
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
            "errno" => stdlib::install_errno(self, scope)?,
            "atexit" => atexit::install(self, scope),
            "faulthandler" => stdlib::faulthandler::install(self, scope),
            "_socket" => stdlib::install_lowlevel_socket(self, scope)?,
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
                stdlib::opcode::install(self, module, scope);
                // Imre emits native code and has no adaptive bytecode interpreter.
                let disabled = self.int(0);
                self.bind(scope, "ENABLE_SPECIALIZATION", disabled);
                self.bind(scope, "ENABLE_SPECIALIZATION_FT", disabled);
            }
            "_frozen_importlib" => {
                if self.bootstrap_globals == NONE {
                    self.bootstrap_globals = self.module_globals(scope)?;
                }
                let key = self.text("_frozen_importlib_external");
                let cached = self.public_module_get(key, Some(self.import_missing))?;
                let external = if cached == self.import_missing {
                    self.find_module("_frozen_importlib_external")?
                        .ok_or_else(|| Self::missing_module("_frozen_importlib_external"))?
                } else {
                    cached
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
            "_locale" => stdlib::locale::install(self, module, scope)?,
            "_warnings" => stdlib::warnings_accel::install(self, module, scope)?,
            "_tracemalloc" => stdlib::tracemalloc::install(self, module, scope),
            "time" => stdlib::time::install_timezone(self, scope)?,
            "struct" => stdlib::structure::install(self, scope)?,
            "_pickle" => stdlib::pickle::install(self, module, scope)?,
            "urllib.parse" => stdlib::urllib::initialize(self, scope),
            "_io" => stdlib::io::install_native(self, scope, module),
            "ast" => stdlib::ast::install(self, scope),
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
                    self.bind(scope, "modules", self.modules);
                }
                let byteorder = self.text(if cfg!(target_endian = "little") {
                    "little"
                } else {
                    "big"
                });
                self.bind(scope, "byteorder", byteorder);
                let thread_info = stdlib::fixed_structseq::thread_info(self);
                self.bind(scope, "thread_info", thread_info);
                for name in [
                    #[cfg(target_os = "linux")]
                    "_get_cpu_count_config",
                    "get_coroutine_origin_tracking_depth",
                    "set_coroutine_origin_tracking_depth",
                ] {
                    let callable = self.builtin(&format!("sys.{name}"));
                    self.bind(scope, name, callable);
                }
                for name in [
                    "audit",
                    "addaudithook",
                    "getsizeof",
                    "getrecursionlimit",
                    "get_asyncgen_hooks",
                    "set_asyncgen_hooks",
                    "is_finalizing",
                    "setrecursionlimit",
                    "_getframe",
                    "_getframemodulename",
                    "displayhook",
                    "excepthook",
                    "unraisablehook",
                    "get_int_max_str_digits",
                    "set_int_max_str_digits",
                ] {
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
                    if stderr && self.threads.system_module.is_none() {
                        self.threads.system_module = Some(module);
                        self.threads.default_stderr = Some(stream);
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
                let deque = self.builtin(deque::CLASS);
                self.bind(scope, "deque", deque);
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
            for function in ["start_new_thread", "_count", "get_ident"] {
                let value = self.builtin(&format!("_thread.{function}"));
                self.bind(scope, function, value);
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let id = self.builtin("_thread.get_native_id");
                self.bind(scope, "get_native_id", id);
            }
        }
        if let Some((path, body, origin)) = source {
            let result = match body {
                importlib::SourceBody::Namespace if self.sources.contains_key("importlib") => {
                    self.lookup_attr(module, "__path__").and_then(|locations| {
                        self.namespace_spec(name_id, locations, module, source_globals)
                    })
                }
                importlib::SourceBody::Namespace => Ok(NONE),
                importlib::SourceBody::Zip(loader) => self.initialize_source(
                    module,
                    scope,
                    name_id,
                    origin.expect("ZIP source has a filename"),
                    loader,
                ),
                importlib::SourceBody::Compiled(_)
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
                    self.initialize_source(module, scope, name_id, origin, NONE)
                }
                importlib::SourceBody::Compiled(entry) => {
                    let code = self.module_code(entry, &path);
                    self.execute(code, scope, FrameOwner::Module(name_id))
                }
                importlib::SourceBody::Filesystem => match origin {
                    Some(file) => self.initialize_source(module, scope, name_id, file, NONE),
                    // Namespace packages have no initializer.
                    None => Ok(NONE),
                },
            };
            let Value::Module { initializing, .. } = self.heap.get_mut(module) else {
                unreachable!()
            };
            *initializing = false;
            result?;
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
                    ("accumulate", "itertools.accumulate"),
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
                let member = self.alloc(Value::SlotMember { owner, name });
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
            if name == "decimal"
                && self.config["bundled_root"]
                    .as_str()
                    .is_some_and(|root| path == std::path::Path::new(root).join("decimal.py"))
                && let Some(class) = self.scope_name(scope, "Decimal")
            {
                self.classes.insert("decimal.Decimal".into(), class);
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
