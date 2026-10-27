//! Object operations for native Python functions with dynamically loaded modules.
//! The compiler lowers statements, branches, loops and function bodies to LLVM.

mod abstract_class;
mod builtins;
mod calls;
mod complex;
mod dictionary;
mod enums;
mod ffi;
mod function_fields;
mod iterators;
mod operators;
mod property;
mod slice;
mod source_code;
mod static_method;
mod stdlib;
mod super_object;
mod thread_local;
mod thread_lock;
mod typing;
mod value;
mod zip;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::objects::exceptions::string_repr;
use value::{
    Code, DictIterator, DictViewKind, Error, FALSE, Heap, Id, NONE, Result, Scope, TRUE, Value,
};

struct SourceModule {
    path: PathBuf,
    code: Code,
}

enum IteratorSlice<'a> {
    Values(&'a [Id]),
    Reversed(&'a [Id]),
    Bytes(&'a [u8]),
    Range { value: i128, len: usize },
}

impl IteratorSlice<'_> {
    /// Counts positions without allocating boxed integer elements.
    fn len(&self) -> usize {
        match self {
            Self::Values(values) | Self::Reversed(values) => values.len(),
            Self::Bytes(bytes) => bytes.len(),
            Self::Range { len, .. } => *len,
        }
    }
}

pub(crate) struct Context {
    heap: Heap,
    config: serde_json::Value,
    executable: PathBuf,
    argv: Id,
    sys_path: Id,
    // The active stack roots suspended contexts while Context.run executes native code.
    contexts: Vec<Id>,
    // A buffer export pins its bytearray's size while Python callbacks run.
    bytearray_exports: Vec<Id>,
    token_missing: Id,
    native_code_objects: HashMap<usize, Id>,
    source_builtins: Option<Id>,
    codec_entries: stdlib::codecs::Registry,
    modules: HashMap<String, Id>,
    classes: HashMap<String, Id>,
    typing: typing::Registry,
    // Python wrappers retain their registered logger for the lifetime of this heap.
    logger_wrappers: HashMap<*const crate::objects::logging::Logger, Id>,
    builtins: HashMap<String, Id>,
    sources: HashMap<String, Vec<SourceModule>>,
    call_depth: usize,
    pending: Option<Error>,
    // Fallback for targets whose native stack bounds cannot be queried.
    slot_depth: usize,
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

    /// Keep a mutable input exported while Python coerces a buffer API's flag.
    fn truth_with_buffer_export(&mut self, input: Id, flag: Id) -> Result<bool> {
        if !matches!(self.heap.get(input), Value::ByteArray(_)) {
            return self.truth(flag);
        }
        self.bytearray_exports.push(input);
        let result = self.truth(flag);
        self.bytearray_exports.pop();
        result
    }

    fn check_bytearray_resize(&self, id: Id) -> Result<()> {
        if self.bytearray_exports.contains(&id) {
            Err(Error::new(
                "BufferError",
                "Existing exports of data: object cannot be re-sized",
            ))
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
                | Value::DictIterator(_)
                | Value::AstWalk { .. }
                | Value::Map { .. }
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
            Value::List(_) => (id, "list_iterator"),
            Value::Tuple(_) => (id, "tuple_iterator"),
            Value::Bytes(_) => (id, "bytes_iterator"),
            Value::ByteArray(_) => (id, "bytearray_iterator"),
            Value::Range { .. } => (id, "range_iterator"),
            Value::Dict(_) | Value::DefaultDict { .. } => {
                return Ok(self.dict_iterator(id, DictViewKind::Keys));
            }
            Value::DictView { dict, kind } => return Ok(self.dict_iterator(dict, kind)),
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
    fn dict_iterator(&mut self, dict: Id, kind: DictViewKind) -> Id {
        let size = self
            .dict_entries(dict)
            .expect("dictionary iterator source")
            .len();
        self.alloc(Value::DictIterator(DictIterator::new(dict, kind, size)))
    }

    /// Compiled loops test for an item before consuming it. User code must advance only once.
    fn loop_iterator(&mut self, iterable: Id) -> Result<Id> {
        let source = self.iterator(iterable)?;
        if matches!(
            self.heap.get(source),
            Value::Instance { .. }
                | Value::DictIterator(_)
                | Value::CallableIterator(_)
                | Value::Map { .. }
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
            Value::Bytes(bytes) | Value::ByteArray(bytes) => {
                IteratorSlice::Bytes(bytes.get(*offset..).unwrap_or_default())
            }
            Value::Range { start, stop, step } => IteratorSlice::Range {
                value: i128::from(*start) + i128::from(*step) * *offset as i128,
                len: builtins::range_len(*start, *stop, *step)?.saturating_sub(*offset),
            },
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
        let more = self.iterator_remaining(id)?.len() != 0;
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
        match self.heap.get_mut(id) {
            Value::Iterator { .. } => {}
            Value::AstWalk { .. } => return stdlib::ast::advance(self, id),
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
            Value::Repeat(repeat) => return Ok(repeat.next()),
            Value::Map { .. } => return self.next_zip_or_map(id),
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
        let item = match self.iterator_remaining(id)? {
            IteratorSlice::Values(values) => values[0],
            IteratorSlice::Reversed(values) => values[values.len() - 1],
            IteratorSlice::Bytes(bytes) => self.int(i64::from(bytes[0])),
            IteratorSlice::Range { value, .. } => self.int(value as i64),
        };
        if let Value::Iterator {
            offset, reverse, ..
        } = self.heap.get_mut(id)
        {
            if *reverse {
                *offset -= 1;
            } else {
                *offset += 1;
            }
        }
        Ok(Some(item))
    }

    fn items(&mut self, id: Id) -> Result<Vec<Id>> {
        if matches!(
            self.heap.get(id),
            Value::AstWalk { .. }
                | Value::DictIterator(_)
                | Value::DictView { .. }
                | Value::Zip { .. }
                | Value::CallableIterator(_)
                | Value::FormatIterator { .. }
                | Value::Map { .. }
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
        if let Some(values) = self.enum_values(id) {
            let Value::Dict(entries) = self.heap.get(values) else {
                unreachable!()
            };
            return Ok(entries.iter().map(|&(_, member)| member).collect());
        }
        if let Value::MappingProxy(dict) = *self.heap.get(id) {
            return self.items(dict);
        }
        match self.heap.get(id).clone() {
            Value::List(items)
            | Value::Tuple(items)
            | Value::Set { items, .. }
            | Value::NamedTuple { values: items, .. } => Ok(items),
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => {
                Ok(entries.into_iter().map(|(key, _)| key).collect())
            }
            Value::Context(bindings) => Ok(bindings.keys().copied().collect()),
            Value::Range { start, stop, step } => builtins::range_items(self, start, stop, step),
            Value::Str(s) => Ok(s.chars().map(|c| self.text(c.to_string())).collect()),
            Value::Bytes(s) | Value::ByteArray(s) => {
                Ok(s.into_iter().map(|b| self.int(i64::from(b))).collect())
            }
            Value::Iterator { .. } => {
                let values = match self.iterator_remaining(id)? {
                    IteratorSlice::Values(values) => values.to_vec(),
                    IteratorSlice::Reversed(values) => values.iter().rev().copied().collect(),
                    remaining @ (IteratorSlice::Bytes(_) | IteratorSlice::Range { .. }) => {
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
            is_abstract: false,
            class_cell: None,
            class_lineage: None,
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
    fn scope_data(&self, scope: Id) -> &Scope {
        let Value::Scope(s) = self.heap.get(scope) else {
            unreachable!("compiled scope handle")
        };
        s
    }
    fn binding_scope(&self, mut scope: Id, name: &str) -> Id {
        if self.scope_data(scope).closure.nonlocals.contains(name) {
            let mut cursor = self.scope_data(scope).parent;
            while let Some(parent) = cursor {
                let frame = self.scope_data(parent);
                if let Some(&cell) = frame.closure.cells.get(name)
                    && let Value::Cell { scope, .. } = self.heap.get(cell)
                {
                    return *scope;
                }
                if frame.closure.declared.contains(name) {
                    return parent;
                }
                cursor = frame.parent;
            }
        }
        if self.scope_data(scope).globals.contains(name) {
            while let Some(parent) = self.scope_data(scope).parent {
                scope = parent;
            }
        }
        scope
    }

    fn scope_name(&self, scope: Id, name: &str) -> Option<Id> {
        let scope = self.scope_data(scope);
        if let Some(namespace) = scope.namespace {
            return self.dict_named_item(namespace, name);
        }
        scope.names.get(name).copied()
    }

    /// Materialize once and keep that exact dict as the module's binding storage.
    fn module_globals(&mut self, mut scope: Id) -> Id {
        while let Some(parent) = self.scope_data(scope).parent {
            scope = parent;
        }
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
            .map(|(name, v)| (self.text(name), v))
            .collect();
        let dict = self.dict(entries);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.namespace = Some(dict);
        dict
    }

    fn remove_binding(&mut self, scope: Id, name: &str, key: Id) -> Result<bool> {
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
        Ok(data.names.shift_remove(name).is_some())
    }

    fn bind(&mut self, scope: Id, name: &str, value: Id) {
        let scope = self.binding_scope(scope, name);
        if let Some(namespace) = self.scope_data(scope).namespace {
            let key = self.text(name);
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
        s.names.insert(name.to_owned(), value);
    }
    fn lookup(&mut self, mut scope: Id, name: &str) -> Result<Id> {
        loop {
            scope = self.binding_scope(scope, name);
            if let Some(locals) = self.scope_data(scope).exec_locals {
                match self.load_exec_name(locals, name) {
                    Err(error) if self.is_builtin_error(&error, "KeyError") => {}
                    value => return value,
                }
            }
            let s = self.scope_data(scope);
            if let Some(&cell) = s.closure.cells.get(name)
                && let Value::Cell {
                    scope: owner,
                    name: cell_name,
                } = self.heap.get(cell)
            {
                return self.scope_name(*owner, cell_name).ok_or_else(|| {
                    Error::new(
                        "NameError",
                        format!("free variable '{name}' is not associated with a value"),
                    )
                });
            }
            if let Some(globals) = s.exec_globals {
                return self.source_global(globals, name);
            }
            if let Some(id) = self.scope_name(scope, name) {
                return Ok(id);
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
        Err(Error::new(
            "NameError",
            format!("name '{name}' is not defined"),
        ))
    }
    fn builtin(&mut self, name: &str) -> Id {
        if let Some(&value) = self.builtins.get(name) {
            return value;
        }
        let value = self.alloc(Value::Native(name.to_owned()));
        self.builtins.insert(name.to_owned(), value);
        let methods = match name {
            "dict" => builtins::DICT_METHODS,
            "int" => &["to_bytes"],
            "str" => builtins::STR_METHODS,
            _ => &[],
        };
        for method in methods {
            self.builtin(&format!("{name}.{method}"));
        }
        value
    }
    fn execute(&mut self, code: Id, scope: Id) -> Result<Id> {
        let entry = match &self.code_object(code).body {
            value::Body::Native(entry) => *entry,
            value::Body::Source(_) | value::Body::PendingGenerator { .. } => {
                return self.call_source(code, scope);
            }
        };
        self.heap.enter(scope, code);
        // SAFETY: a compiler-generated initializer or function uses this ABI and
        // returns before this Context can move or outlive the native module.
        let result = unsafe { entry(self, scope) };
        self.heap.leave(result);
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

    fn module(&mut self, name: &str) -> Result<Id> {
        if let Some(value) = self.modules.get(name) {
            return Ok(*value);
        }
        // Execute the package initializer before its child. A regular package
        // constrains child modules to its __path__; namespace packages combine
        // their locations in search order.
        let parent = if let Some((parent, child)) = name.rsplit_once('.') {
            let id = self.module(parent)?;
            if let Some(id) = self.modules.get(name) {
                return Ok(*id);
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
                .map(|m| (m.path.clone(), m.code))
        } else if stdlib::is_module(name) {
            if matches!(name, "email" | "urllib") {
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
                    .map(|source| (source.path.clone(), source.code))
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
            let mut selected = None;
            let mut namespace = None;
            // A bundled package contributes its own __path__. A first-party
            // regular package can never acquire one of its children implicitly.
            let search = self.items(search)?;
            let bundle = self.config["bundled_root"]
                .as_str()
                .map(std::path::Path::new);
            let roots = search
                .iter()
                .filter_map(|&id| self.str_value(id).ok().map(std::path::Path::new));
            for root in roots.chain(bundle.filter(|_| parent.is_none())) {
                let root = if root.as_os_str().is_empty() {
                    std::env::current_dir()
                } else if bundle.is_some_and(|dir| root.starts_with(dir)) {
                    Ok(root.to_owned())
                } else {
                    std::path::absolute(root)
                };
                let Ok(root) = root else { continue };
                let stem = root.join(child);
                let Some(choices) = self.sources.get(name) else {
                    continue;
                };
                if let Some(module) = choices.iter().find(|m| {
                    m.path == stem.join("__init__.py") || m.path == stem.with_extension("py")
                }) {
                    locations.clear();
                    if module.path.ends_with("__init__.py") {
                        locations.push(stem);
                    }
                    selected = Some((module.path.clone(), module.code));
                    break;
                }
                if let Some(module) = choices.iter().find(|m| m.path == stem) {
                    locations.push(stem);
                    namespace.get_or_insert((module.path.clone(), module.code));
                }
            }
            selected.or(namespace)
        };
        if source.is_none() && !stdlib::is_module(name) {
            return Err(Error::new(
                "ModuleNotFoundError",
                format!("No module named {}", string_repr(name)),
            )
            .with_import(Some(name), None, None));
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
        });
        self.modules.insert(name.to_owned(), module);
        match name {
            "os" => {
                let path = self.module("posixpath")?;
                self.bind(scope, "path", path);
                self.modules.insert("os.path".to_owned(), path);
            }
            "io" => stdlib::io::install(self, scope),
            "_io" => {
                self.module("io")?;
                for name in ["BytesIO", "StringIO"] {
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
            "collections" => {
                let abc = self.module("_collections_abc")?;
                self.bind(scope, "abc", abc);
                self.modules.insert("collections.abc".to_owned(), abc);
            }
            _ => {}
        }
        if let Some((path, entry)) = source {
            let filename = self.text(path.to_string_lossy());
            let name_id = self.text("<module>");
            let code = self.alloc(Value::Code(value::CodeObject {
                body: value::Body::Native(entry),
                parameters: NONE,
                nested: Vec::new(),
                freevars: Vec::new(),
                closure_supported: false,
                name: name_id,
                qualname: name_id,
                doc: NONE,
                filename,
                first_line: 1,
                module_definition: true,
                coroutine: false,
            }));
            let result = self.execute(code, scope);
            let Value::Module { initializing, .. } = self.heap.get_mut(module) else {
                unreachable!()
            };
            *initializing = false;
            if let Err(error) = result {
                self.modules.remove(name);
                return Err(error);
            }
        }
        if let Some((parent, child)) = parent {
            self.set_attr(parent, child, module)?;
        }
        Ok(module)
    }
}
