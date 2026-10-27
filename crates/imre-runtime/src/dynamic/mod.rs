//! Object operations for native Python functions with dynamically loaded modules.
//! The compiler lowers statements, branches, loops and function bodies to LLVM.

mod abstract_class;
mod builtins;
mod calls;
mod complex;
mod deque;
mod dictionary;
mod enums;
mod ffi;
mod function_fields;
mod importlib;
mod iterators;
mod mapping_abc;
mod module_type;
mod namedtuple;
mod operators;
mod os_error;
mod property;
mod range;
mod slice;
mod source_code;
mod static_method;
mod stdlib;
mod super_object;
mod thread_local;
mod thread_lock;
mod typing;
mod union;
mod value;
mod weakref;
mod wrapper_descriptor;
mod zip;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::objects::exceptions::string_repr;
use value::{
    Code, DictIterator, DictViewKind, Error, FALSE, FrameOwner, Heap, Id, NONE, Result, Scope,
    TRUE, Value,
};

struct SourceModule {
    path: PathBuf,
    code: Code,
}

enum IteratorSlice<'a> {
    Values(&'a [Id]),
    Reversed(&'a [Id]),
    Bytes(&'a [u8]),
    ReversedBytes(&'a [u8]),
    ReversedText(&'a str),
}

impl IteratorSlice<'_> {
    /// Counts positions without allocating boxed integer elements.
    fn len(&self) -> usize {
        match self {
            Self::Values(values) | Self::Reversed(values) => values.len(),
            Self::Bytes(bytes) | Self::ReversedBytes(bytes) => bytes.len(),
            Self::ReversedText(text) => text.chars().count(),
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

pub(crate) struct Context {
    heap: Heap,
    config: serde_json::Value,
    executable: PathBuf,
    argv: Id,
    sys_path: Id,
    default_encoding: Id,
    // The active stack roots suspended contexts while Context.run executes native code.
    contexts: Vec<Id>,
    // A buffer export pins its bytearray's size while Python callbacks run.
    bytearray_exports: Vec<Id>,
    // Active sorts temporarily detach their list storage while Python callbacks run.
    sorting_lists: Vec<Id>,
    // Container ancestors visible to a reentrant Python __repr__.
    repr_ancestors: Vec<Id>,
    token_missing: Id,
    native_code_objects: HashMap<usize, Id>,
    source_builtins: Option<Id>,
    zip_directories: HashMap<String, crate::objects::ZipArchive>,
    codec_entries: stdlib::codecs::Registry,
    modules: HashMap<String, Id>,
    classes: HashMap<String, Id>,
    typing: typing::Registry,
    // Python wrappers retain their registered logger for the lifetime of this heap.
    logger_wrappers: HashMap<*const crate::objects::logging::Logger, Id>,
    builtins: HashMap<String, Id>,
    sources: HashMap<String, Vec<SourceModule>>,
    call_depth: usize,
    deque_comparison_repeats: usize,
    pending: Option<Error>,
    // Fallback for targets whose native stack bounds cannot be queried.
    slot_depth: usize,
}

fn copy_ids(values: &[Id]) -> Result<Vec<Id>> {
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
        self.heap.alloc(value)
    }
    fn text(&mut self, value: impl Into<String>) -> Id {
        self.alloc(Value::Str(value.into()))
    }
    fn int(&mut self, value: i64) -> Id {
        self.alloc(Value::Int(crate::Int::from(value)))
    }
    /// The builtin mapping payload is independent of a subclass instance's attributes.
    fn dict_storage(&self, value: Id) -> Option<Id> {
        match self.heap.get(value) {
            Value::Dict(_) | Value::DefaultDict { .. } => Some(value),
            Value::Instance { dictionary, .. } => *dictionary,
            _ => None,
        }
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

    /// Slot descriptors require the native layout even when a proxy passes isinstance.
    fn builtin_slot_accepts(&self, object: Id, owner: &str) -> bool {
        match (owner, self.heap.get(self.scalar_value(object))) {
            ("list", Value::List(_)) | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => {
                true
            }
            ("tuple", Value::Instance { class, .. }) => self.namedtuple_fields(*class).is_some(),
            ("str", Value::Str(_))
            | ("bytes", Value::Bytes(_))
            | ("bytearray", Value::ByteArray(_))
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. }) => true,
            _ => false,
        }
    }

    fn dict_entries(&self, value: Id) -> Option<&[(Id, Id)]> {
        match self.heap.get(self.dict_storage(value)?) {
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => Some(entries),
            _ => unreachable!("dictionary payload"),
        }
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
            Value::Str(_) => Some(id),
            Value::Instance {
                scalar: Some(value),
                ..
            } if matches!(self.heap.get(*value), Value::Str(_)) => Some(*value),
            _ => None,
        }
    }

    fn bytes_like(&self, id: Id) -> Result<&[u8]> {
        match self.heap.get(self.scalar_value(id)) {
            Value::Bytes(data) | Value::ByteArray(data) => Ok(data),
            _ => Err(Error::new(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    self.type_name(id)
                ),
            )),
        }
    }

    /// Keep the underlying bytearray exported while an argument conversion runs Python.
    fn with_buffer_export<T>(
        &mut self,
        input: Id,
        convert: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let input = self.scalar_value(input);
        if !matches!(self.heap.get(input), Value::ByteArray(_)) {
            return convert(self);
        }
        self.bytearray_exports.push(input);
        let result = convert(self);
        self.bytearray_exports.pop();
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

    fn check_bytearray_resize(&self, id: Id) -> Result<()> {
        if self.bytearray_exports.contains(&id) {
            Err(Self::exported_bytearray_error())
        } else {
            Ok(())
        }
    }

    fn str_value(&self, id: Id) -> Result<&str> {
        let id = self
            .string_storage(id)
            .ok_or_else(|| Error::new("TypeError", "expected str"))?;
        let Value::Str(value) = self.heap.get(id) else {
            unreachable!()
        };
        Ok(value)
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
        matches!(
            self.heap.get(id),
            Value::Iterator { .. }
                | Value::RangeIterator(_)
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::DictIterator(_)
                | Value::MappingViewIterator { .. }
                | Value::AstWalk { .. }
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::Zip { .. }
                | Value::CallableIterator(_)
                | Value::FormatIterator { .. }
                | Value::Generator { .. }
                | Value::File(_)
                | Value::TextFile(_)
                | Value::Enumerate { .. }
                | Value::Count { .. }
                | Value::Repeat(_)
        )
    }

    /// Keeps sequence iterators on their source and respects an existing iterator's cursor.
    fn iterator(&mut self, id: Id) -> Result<Id> {
        if self.is_native_iterator(id) {
            let closed = match self.heap.get(id) {
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
            Value::Context(_) => return Ok(self.context_iterator(id, "Context.keys")),
            Value::Deque(ref ring) => {
                let cursor = deque::iterator(id, ring, false);
                return Ok(self.alloc(cursor));
            }
            Value::List(_) => (id, "list_iterator"),
            Value::Tuple(_) => (id, "tuple_iterator"),
            Value::Bytes(_) => (id, "bytes_iterator"),
            Value::ByteArray(_) => (id, "bytearray_iterator"),
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
            Value::MappingView { mapping, kind } => {
                return Ok(self.alloc(Value::MappingViewIterator {
                    mapping: Some(mapping),
                    source: None,
                    kind,
                    running: false,
                }));
            }
            Value::Instance { class, .. }
                if let Some(slot) = self.class_attr(class, "__iter__") =>
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
            Value::Instance { .. } if let Some((storage, _)) = self.sequence_payload(id) => {
                return self.iterator(storage);
            }
            _ => {
                let values = self.items(id)?;
                (self.alloc(Value::Tuple(values)), "iterator")
            }
        };
        Ok(self.alloc(Value::Iterator {
            sequence: Some(sequence),
            offset: 0,
            reverse: false,
            kind,
            context: None,
        }))
    }

    /// Start a cursor at the first entry without copying keys or values.
    fn dict_iterator(&mut self, dict: Id, kind: DictViewKind, ordered: bool, reverse: bool) -> Id {
        let storage = self.dict_storage(dict).expect("dictionary iterator source");
        let entries = self.stored_entries(storage);
        let iter = DictIterator::new(
            dict,
            kind,
            entries.len(),
            ordered.then(|| entries.order_state()),
            reverse.then(|| entries.insertion_end()),
        );
        self.alloc(Value::DictIterator(iter))
    }

    /// Compiled loops test for an item before consuming it. User code must advance only once.
    fn loop_iterator(&mut self, iterable: Id) -> Result<Id> {
        let source = self.iterator(iterable)?;
        if matches!(
            self.heap.get(source),
            Value::Instance { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::DictIterator(_)
                | Value::MappingViewIterator { .. }
                | Value::CallableIterator(_)
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::Zip { .. }
                | Value::Generator { .. }
                | Value::File(_)
                | Value::TextFile(_)
                | Value::Enumerate { .. }
        ) {
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
        Ok(match self.heap.get(*sequence) {
            Value::List(values) | Value::Tuple(values) if *reverse => {
                IteratorSlice::Reversed(values.get(..*offset).unwrap_or_default())
            }
            Value::List(values) | Value::Tuple(values) => {
                IteratorSlice::Values(values.get(*offset..).unwrap_or_default())
            }
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

    fn reverse_sequence_len(&mut self, source: Id) -> Result<usize> {
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

    fn next_callable_item(&mut self, id: Id, callable: Id, sentinel: Id) -> Result<Option<Id>> {
        match self.call(callable, Vec::new(), Vec::new()) {
            Ok(item) => {
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
            Err(error) if self.is_stop_iteration(&error) => {}
            Err(error) => return Err(error),
        }
        *self.heap.get_mut(id) = Value::CallableIterator(None);
        Ok(None)
    }

    /// Advances a boxed iterator once, leaving its remaining values available to other callers.
    fn next_result(&mut self, id: Id) -> Result<Option<Id>> {
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
                let Value::Deque(ring) = self.heap.get(deque) else {
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
            Value::MappingViewIterator { .. } => return self.next_mapping_view_item(id),
            Value::DictIterator(iter) => {
                let mut iter = *iter;
                let item = iter.next(self);
                *self.heap.get_mut(id) = Value::DictIterator(iter);
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
            Value::Generator { code, .. } => {
                let code = *code;
                // Delegated generators resume their children on the native stack.
                return stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    match self.code_object(code).body {
                        value::Body::Native(_) => self.next_generator_item(id),
                        value::Body::Source(_) | value::Body::PendingGenerator { .. } => {
                            self.resume_source(id)
                        }
                    }
                });
            }
            Value::LoopIterator { peeked, .. } => return Ok(peeked.take()),
            Value::File(_) | Value::TextFile(_) => {
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
            IteratorSlice::Bytes(bytes) => (self.int(i64::from(bytes[0])), 1),
            IteratorSlice::ReversedBytes(bytes) => (self.int(i64::from(bytes[bytes.len() - 1])), 1),
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

    /// Advance a Mapping ABC view as a generator, without snapshotting its keys or values.
    fn next_mapping_view_item(&mut self, id: Id) -> Result<Option<Id>> {
        let (mapping, source, kind) = match *self.heap.get(id) {
            Value::MappingViewIterator { mapping: None, .. } => return Ok(None),
            Value::MappingViewIterator { running: true, .. } => {
                return Err(Error::new("ValueError", "generator already executing"));
            }
            Value::MappingViewIterator {
                mapping: Some(mapping),
                source,
                kind,
                ..
            } => (mapping, source, kind),
            _ => unreachable!(),
        };
        self.heap.keep(id);
        if let Value::MappingViewIterator { running, .. } = self.heap.get_mut(id) {
            *running = true;
        }
        let item = (|| {
            let source = match source {
                Some(source) => source,
                None => {
                    let source = self.iterator(mapping)?;
                    if let Value::MappingViewIterator { source: slot, .. } = self.heap.get_mut(id) {
                        *slot = Some(source);
                    }
                    source
                }
            };
            let Some(key) = self.next_item(source)? else {
                return Ok(None);
            };
            if matches!(kind, DictViewKind::Keys) {
                return Ok(Some(key));
            }
            let value = self.get_item(mapping, key)?;
            Ok(Some(match kind {
                DictViewKind::Values => value,
                DictViewKind::Items => self.alloc(Value::Tuple(vec![key, value])),
                DictViewKind::Keys => unreachable!(),
            }))
        })();
        if let Value::MappingViewIterator {
            mapping,
            source,
            running,
            ..
        } = self.heap.get_mut(id)
        {
            *running = false;
            if !matches!(&item, Ok(Some(_))) {
                *mapping = None;
                *source = None;
            }
        }
        if !matches!(&item, Ok(Some(_))) {
            self.heap.weak_edge_removed(mapping);
        }
        item
    }

    fn items(&mut self, id: Id) -> Result<Vec<Id>> {
        if matches!(
            self.heap.get(id),
            Value::AstWalk { .. }
                | Value::ReverseSequence { .. }
                | Value::DequeIterator { .. }
                | Value::DictIterator(_)
                | Value::MappingViewIterator { .. }
                | Value::DictView { .. }
                | Value::MappingView { .. }
                | Value::Zip { .. }
                | Value::CallableIterator(_)
                | Value::FormatIterator { .. }
                | Value::Map { .. }
                | Value::Filter { .. }
                | Value::Generator { .. }
                | Value::File(_)
                | Value::TextFile(_)
                | Value::Count { .. }
                | Value::Repeat(_)
                | Value::Enumerate { .. }
        ) || self.slot(id, "__iter__").is_some()
        {
            let iter = self.iterator(id)?;
            let mut items = Vec::new();
            while let Some(item) = self.next_item(iter)? {
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
        if let Some(values) = self.enum_values(id) {
            let Value::Dict(entries) = self.heap.get(values) else {
                unreachable!()
            };
            return Ok(entries.iter().map(|&(_, member)| member).collect());
        }
        if let Value::MappingProxy(dict) = *self.heap.get(id) {
            return self.items(dict);
        }
        if let Value::List(items) | Value::Tuple(items) | Value::NamedTuple { values: items, .. } =
            self.heap.get(self.scalar_value(id))
        {
            return copy_ids(items);
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
            Value::MappingView { mapping, kind } => mapping_abc::view_items(self, mapping, kind),
            Value::Str(s) => Ok(s.chars().map(|c| self.text(c.to_string())).collect()),
            Value::Bytes(s) | Value::ByteArray(s) => {
                Ok(s.into_iter().map(|b| self.int(i64::from(b))).collect())
            }
            Value::Iterator { .. } => {
                let values = match self.iterator_remaining(id)? {
                    IteratorSlice::Values(values) => values.to_vec(),
                    IteratorSlice::Reversed(values) => values.iter().rev().copied().collect(),
                    remaining @ (IteratorSlice::Bytes(_)
                    | IteratorSlice::ReversedBytes(_)
                    | IteratorSlice::ReversedText(_)) => {
                        let mut values = Vec::new();
                        values.try_reserve_exact(remaining.len()).map_err(|_| {
                            Error::new("MemoryError", "iterator is too large to materialize")
                        })?;
                        while let Some(item) = self.next_item(id)? {
                            values.push(item);
                        }
                        return Ok(values);
                    }
                };
                if let Value::Iterator { sequence, .. } = self.heap.get_mut(id) {
                    *sequence = None;
                }
                Ok(values)
            }
            Value::Instance { class, fields, .. } if self.namedtuple_fields(class).is_some() => {
                Ok(self
                    .namedtuple_fields(class)
                    .unwrap()
                    .into_iter()
                    .map(|k| fields[&k])
                    .collect())
            }
            Value::Instance { .. } if self.slot(id, "__iter__").is_some() => {
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
        self.alloc(Value::Scope(Scope {
            class_body: None,
            enum_namespace: None,
            is_abstract: false,
            is_comprehension: false,
            class_cell: None,
            class_namespace: None,
            first_argument: None,
            names: Default::default(),
            closure: Default::default(),
            namespace: None,
            globals: Default::default(),
            exec_globals: None,
            exec_locals: None,
            parent,
            module: module.to_owned(),
        }))
    }
    /// Comprehensions own their loop variables, but assignment expressions bind outside them.
    fn comprehension_scope(&mut self, parent: Id) -> Id {
        let module = self.scope_data(parent).module.clone();
        let scope = self.scope(&module, Some(parent));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.is_comprehension = true;
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
        if let Some(namespace) = scope.namespace {
            return self.dict_named_item(namespace, name);
        }
        scope.names.get(name).copied()
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

    /// Expose the class's own unbound members through a fresh read-only proxy.
    fn class_dictionary(&mut self, scope: Id) -> Id {
        let frame = self.scope_data(scope);
        let dictionary = match frame
            .class_namespace
            .as_ref()
            .and_then(|class| class.dictionary)
        {
            Some(existing) => existing,
            None => {
                let names = frame.names.clone();
                let entries = names
                    .into_iter()
                    .filter(|(name, _)| name != "__qualname__")
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
        };
        self.alloc(Value::MappingProxy(dictionary))
    }

    /// Materialize once and keep that exact dict as the module's binding storage.
    fn module_globals(&mut self, scope: Id) -> Id {
        let scope = self.root_scope(scope);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!("compiled scope handle")
        };
        if let Some(globals) = frame.exec_globals {
            return globals.dict;
        }
        if let Some(dict) = frame.namespace {
            return dict;
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
        dict
    }

    fn remove_binding(&mut self, scope: Id, name: &str, key: Id) -> Result<bool> {
        if let Some(&cell) = self.scope_data(scope).closure.cells.get(name) {
            return Ok(self.heap.replace_cell(cell, None).is_some());
        }
        if let Some(locals) = self.scope_data(scope).exec_locals {
            return Ok(self.delete_item(locals, key).is_ok());
        }
        if let Some(namespace) = self.scope_data(scope).namespace {
            if self.scope_name(scope, name).is_none() {
                return Ok(false);
            }
            let storage = self.dict_storage(namespace).expect("module namespace");
            self.delete_item(storage, key)?;
            return Ok(true);
        }
        let Value::Scope(data) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        if let Some(order) = &mut data.class_body {
            order.retain(|member| member != name);
        }
        let previous = data.names.shift_remove(name);
        if previous.is_some()
            && name != "__qualname__"
            && let Some(dictionary) = data
                .class_namespace
                .as_ref()
                .and_then(|class| class.dictionary)
        {
            self.delete_item(dictionary, key)?;
        }
        if let Some(value) = previous {
            self.heap.weak_edge_removed(value);
        }
        Ok(previous.is_some())
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
        if let Some(namespace) = self.scope_data(scope).namespace {
            let key = self.heap.code_name(name);
            let storage = self.dict_storage(namespace).expect("module namespace");
            self.put_item(storage, key, value)
                .expect("string key in module dictionary");
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
            && name != "__qualname__"
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
    fn missing_binding(&self, origin: Id, scope: Id, name: &str) -> Error {
        let frame = self.scope_data(scope);
        let declared = frame.class_body.is_none() && frame.closure.declared.contains(name);
        if declared && origin == scope {
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
        }
    }

    fn lookup(&mut self, mut scope: Id, name: &str) -> Result<Id> {
        let origin = scope;
        loop {
            scope = self.binding_scope(scope, name);
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
            if let Some(id) = self.scope_name(scope, name) {
                return Ok(id);
            }
            if s.class_body.is_none() && s.closure.declared.contains(name) {
                return Err(self.missing_binding(origin, scope, name));
            }
            if name == "__class__"
                && s.class_body.is_none()
                && let Some(cell) = s.class_cell
                && let Value::ClassCell(Some(class)) = self.heap.get(cell)
            {
                return Ok(*class);
            }
            if let Some(parent) = s.parent {
                scope = parent;
            } else {
                break;
            }
        }
        if name == "Ellipsis" {
            return Ok(value::ELLIPSIS);
        }
        if name == "NotImplemented" {
            return Ok(value::NOT_IMPLEMENTED);
        }
        if builtins::is_builtin(name) {
            return Ok(self.builtin(name));
        }
        Err(self.missing_binding(origin, scope, name))
    }
    fn builtin(&mut self, name: &str) -> Id {
        if let Some(&value) = self.builtins.get(name) {
            return value;
        }
        let value = self.alloc(Value::Native(name.to_owned()));
        self.builtins.insert(name.to_owned(), value);
        if builtins::is_exception(name) {
            self.builtin(&format!("{}.__new__", builtins::exception_new_owner(name)));
            self.builtin(builtins::exception_initializer(name));
        }
        let methods = match name {
            "object" => &["__hash__"],
            "dict" => builtins::DICT_METHODS,
            "list" => &["__repr__", "__init__", "append"],
            "tuple" => &["__repr__", "__getitem__"],
            "set" | "frozenset" | "bytes" | "bytearray" => &["__repr__"],
            "property" => property::METHODS,
            "int" => &["to_bytes"],
            "str" => {
                self.builtin("str.maketrans");
                builtins::STR_METHODS
            }
            weakref::REFERENCE_TYPE => weakref::REFERENCE_METHODS,
            _ => &[],
        };
        for method in methods {
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
        let attribute_error = match self.lookup_attr(object, &name) {
            Err(error) if error.kind == "AttributeError" => error,
            value => return value,
        };
        let Value::Module {
            name: module,
            scope,
            ..
        } = self.heap.get(object)
        else {
            return Err(attribute_error);
        };
        let names = &self.scope_data(*scope).names;
        let package = if names.contains_key("__path__") {
            names.get("__name__").map(|id| self.display(*id))
        } else {
            None
        };
        let child = format!("{}.{name}", package.as_deref().unwrap_or(module));
        // A child's initializer can raise ModuleNotFoundError for its own dependency.
        // Only a failure to find the requested child is a missing export.
        match self.module(&child) {
            Err(error)
                if error.kind == "ModuleNotFoundError"
                    && error
                        .import
                        .as_ref()
                        .and_then(|fields| fields.name.as_deref())
                        == Some(&child) => {}
            value => return value,
        }
        let Value::Module {
            scope,
            origin,
            initializing,
            ..
        } = self.heap.get(object)
        else {
            unreachable!()
        };
        let names = &self.scope_data(*scope).names;
        let module = names
            .get("__name__")
            .and_then(|id| self.str_value(*id).ok());
        let quoted_module = string_repr(module.unwrap_or("<unknown module name>"));
        let origin = origin.or_else(|| names.get("__file__").copied());
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

    fn module(&mut self, name: &str) -> Result<Id> {
        self.find_module(name, false)?
            .ok_or_else(|| Self::missing_module(name))
    }

    /// Package resource lookups can initialize filesystem sources added after compilation.
    fn find_module(&mut self, name: &str, filesystem: bool) -> Result<Option<Id>> {
        if let Some(value) = self.modules.get(name) {
            return Ok(Some(*value));
        }
        // Execute the package initializer before its child. A regular package
        // constrains child modules to its __path__; namespace packages combine
        // their locations in search order.
        let parent = if let Some((parent, child)) = name.rsplit_once('.') {
            let id = self
                .find_module(parent, filesystem)?
                .ok_or_else(|| Self::missing_module(parent))?;
            if let Some(id) = self.modules.get(name) {
                return Ok(Some(*id));
            }
            Some((id, child))
        } else {
            None
        };
        let mut locations = Vec::new();
        let source = if name == "__main__" {
            self.sources
                .get(name)
                .and_then(|sources| sources.first())
                .map(|m| (m.path.clone(), importlib::SourceBody::Compiled(m.code)))
        } else if stdlib::is_module(name) {
            if matches!(name, "email" | "importlib" | "urllib") {
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
                (
                    self.lookup_attr(parent, "__path__").map_err(|_| {
                        Error::new(
                            "ModuleNotFoundError",
                            format!(
                                "No module named {}; {} is not a package",
                                string_repr(name),
                                string_repr(name.rsplit_once('.').unwrap().0)
                            ),
                        )
                        .with_import(Some(name), None, None)
                    })?,
                    child,
                )
            } else {
                (self.sys_path, name)
            };
            self.source_on_path(
                name,
                child,
                search,
                parent.is_none(),
                filesystem,
                &mut locations,
            )?
        };
        if source.is_none() && !stdlib::is_module(name) {
            return Ok(None);
        }
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
        let origin = source.as_ref().and_then(|(path, _)| {
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
            constructor: None,
        });
        self.modules.insert(name.to_owned(), module);
        match name {
            "os" => {
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
                let pathsep = self.lookup_attr(path, "pathsep")?;
                self.bind(scope, "pathsep", pathsep);
                self.bind(scope, "path", path);
                self.modules.insert("os.path".to_owned(), path);
            }
            "posixpath" => {
                let pathsep = self.text(":");
                self.bind(scope, "pathsep", pathsep);
            }
            "_frozen_importlib" => {
                let external = self.module("_frozen_importlib_external")?;
                self.bind(scope, "_bootstrap_external", external);
            }
            "io" => stdlib::io::install(self, scope),
            "urllib.parse" => stdlib::urllib::initialize(self, scope),
            "_io" => {
                self.module("io")?;
                for name in ["BytesIO", "StringIO", "TextIOWrapper"] {
                    let class = self.builtins[&format!("_io.{name}")];
                    self.bind(scope, name, class);
                }
            }
            "ast" => stdlib::ast::install(self, scope),
            "selectors" => stdlib::readiness::initialize(self, scope),
            "sys" => {
                for (name, original, stderr) in [
                    ("stdout", "__stdout__", false),
                    ("stderr", "__stderr__", true),
                ] {
                    let stream =
                        self.alloc(Value::TextFile(crate::objects::TextFile::standard(stderr)));
                    self.bind(scope, name, stream);
                    self.bind(scope, original, stream);
                }
            }
            "array" => {
                let array = self.builtin("array.array");
                self.bind(scope, "array", array);
                let abc = self.module("_collections_abc")?;
                let sequence = self.lookup_attr(abc, "MutableSequence")?;
                let register = self.lookup_attr(sequence, "register")?;
                self.call(register, vec![array], Vec::new())?;
            }
            "collections" => {
                let deque = self.builtin(deque::CLASS);
                self.bind(scope, "deque", deque);
                let namedtuple = self.builtin("collections.namedtuple");
                self.bind(scope, "namedtuple", namedtuple);
                let abc = self.module("_collections_abc")?;
                self.bind(scope, "abc", abc);
                self.modules.insert("collections.abc".to_owned(), abc);
            }
            _ => {}
        }
        if let Some((path, body)) = source {
            let result = match body {
                importlib::SourceBody::Zip(NONE) => Err(Error::new(
                    "NotImplementedError",
                    "ZIP namespace packages are unsupported",
                )),
                importlib::SourceBody::Zip(loader) => self.initialize_source(
                    module,
                    scope,
                    name_id,
                    origin.expect("ZIP source has a filename"),
                    loader,
                ),
                importlib::SourceBody::Compiled(_) | importlib::SourceBody::Filesystem
                    if let Some(origin) = origin
                        && name != "__main__"
                        && !stdlib::is_module(name)
                        && self.sources.contains_key("importlib")
                        && path.is_absolute() =>
                {
                    self.initialize_source(module, scope, name_id, origin, NONE)
                }
                importlib::SourceBody::Compiled(entry) => {
                    let code = self.module_code(entry, &path);
                    self.execute(code, scope, FrameOwner::Module(name_id))
                }
                importlib::SourceBody::Filesystem => Ok(NONE),
            };
            let Value::Module { initializing, .. } = self.heap.get_mut(module) else {
                unreachable!()
            };
            *initializing = false;
            if let Err(error) = result {
                self.modules.remove(name);
                return Err(error);
            }
            if name == "decimal"
                && self.config["bundled_root"]
                    .as_str()
                    .is_some_and(|root| path == std::path::Path::new(root).join("decimal.py"))
                && let Some(class) = self.scope_name(scope, "Decimal")
            {
                self.classes.insert("decimal.Decimal".into(), class);
            }
        }
        if let Some((parent, child)) = parent {
            self.set_attr(parent, child, module)?;
        }
        Ok(Some(module))
    }
}
