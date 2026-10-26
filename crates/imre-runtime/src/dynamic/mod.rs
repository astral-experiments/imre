//! Object operations for native Python functions with dynamically loaded modules.
//! The compiler lowers statements, branches, loops and function bodies to LLVM.

mod builtins;
mod calls;
mod dictionary;
mod ffi;
mod function_fields;
mod operators;
mod property;
mod sets;
mod static_method;
mod stdlib;
mod super_object;
mod value;

use std::collections::HashMap;
use std::path::PathBuf;

use value::{Code, Error, FALSE, Heap, Id, NONE, Result, Scope, TRUE, Value};

struct SourceModule {
    path: PathBuf,
    code: Code,
}

enum IteratorSlice<'a> {
    Values(&'a [Id]),
    Reversed(&'a [Id]),
    Bytes(&'a [u8]),
}

impl IteratorSlice<'_> {
    /// Counts positions without allocating boxed integers for bytes.
    fn len(&self) -> usize {
        match self {
            Self::Values(values) | Self::Reversed(values) => values.len(),
            Self::Bytes(bytes) => bytes.len(),
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
    token_missing: Id,
    native_code_objects: HashMap<usize, Id>,
    codec_entries: stdlib::codecs::Registry,
    modules: HashMap<String, Id>,
    classes: HashMap<String, Id>,
    builtins: HashMap<String, Id>,
    sources: HashMap<String, Vec<SourceModule>>,
    pending: Option<Error>,
}

impl Context {
    fn code_object(&self, id: Id) -> value::CodeObject {
        let Value::Code(code) = self.heap.get(id) else {
            unreachable!("function must retain its code object")
        };
        *code
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

    fn str_value(&self, id: Id) -> Result<&str> {
        if let Value::Str(value) = self.heap.get(id) {
            Ok(value)
        } else {
            Err(Error::new("TypeError", "expected str"))
        }
    }
    fn bytes_like(&self, id: Id) -> Result<&[u8]> {
        match self.heap.get(id) {
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
    fn as_int(&self, id: Id) -> Result<crate::Int> {
        match self.heap.get(id) {
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
    /// Keeps sequence iterators on their source and respects an existing iterator's cursor.
    fn iterator(&mut self, id: Id) -> Result<Id> {
        let sequence = match self.heap.get(id) {
            Value::Iterator { .. } | Value::AstWalk(_) | Value::DictionaryIterator(_) => {
                return Ok(id);
            }
            Value::DictView { dict, kind } => return Ok(self.dictionary_iterator(*dict, *kind)),
            Value::Dict(_) | Value::DefaultDict { .. } => {
                return Ok(self.dictionary_iterator(id, value::DictViewKind::Keys));
            }
            Value::List(_) | Value::Tuple(_) | Value::Bytes(_) | Value::ByteArray(_) => id,
            Value::Instance {
                dictionary: Some(_),
                ..
            } => {
                let method = self
                    .bound_slot(id, "__iter__")?
                    .expect("dict iterator slot");
                let result = self.call(method, Vec::new(), Vec::new())?;
                return if matches!(
                    self.heap.get(result),
                    Value::Iterator { .. } | Value::AstWalk(_) | Value::DictionaryIterator(_)
                ) {
                    Ok(result)
                } else {
                    Err(Error::new(
                        "TypeError",
                        format!(
                            "iter() returned non-iterator of type '{}'",
                            self.type_name(result)
                        ),
                    ))
                };
            }
            _ => {
                let values = self.items(id)?;
                self.alloc(Value::Tuple(values))
            }
        };
        Ok(self.alloc(Value::Iterator {
            sequence: Some(sequence),
            offset: 0,
            reverse: false,
        }))
    }

    /// Borrows the unconsumed part of the iterator's current sequence.
    fn iterator_remaining(&self, id: Id) -> Result<IteratorSlice<'_>> {
        let Value::Iterator {
            sequence,
            offset,
            reverse,
        } = self.heap.get(id)
        else {
            return Err(Error::new("TypeError", "expected iterator"));
        };
        let Some(sequence) = sequence else {
            return Ok(IteratorSlice::Values(&[]));
        };
        Ok(match self.heap.get(*sequence) {
            Value::List(values) if *reverse => {
                IteratorSlice::Reversed(values.get(..*offset).unwrap_or_default())
            }
            Value::List(values) | Value::Tuple(values) => {
                IteratorSlice::Values(values.get(*offset..).unwrap_or_default())
            }
            Value::Bytes(bytes) | Value::ByteArray(bytes) => {
                IteratorSlice::Bytes(bytes.get(*offset..).unwrap_or_default())
            }
            _ => unreachable!("iterator sequence"),
        })
    }

    /// Reads the current position and permanently detaches the source on exhaustion.
    fn iterator_has_item(&mut self, id: Id) -> Result<bool> {
        if matches!(self.heap.get(id), Value::DictionaryIterator(_)) {
            return Ok(self.dictionary_entry(id)?.is_some());
        }
        if let Value::AstWalk(queue) = self.heap.get(id) {
            return Ok(!queue.is_empty());
        }
        let more = self.iterator_remaining(id)?.len() != 0;
        if !more && let Value::Iterator { sequence, .. } = self.heap.get_mut(id) {
            *sequence = None;
        }
        Ok(more)
    }

    /// Advances a boxed iterator once, leaving its remaining values available to other callers.
    fn next_item(&mut self, id: Id) -> Result<Option<Id>> {
        if matches!(self.heap.get(id), Value::DictionaryIterator(_)) {
            return self.next_dictionary_item(id);
        }
        if matches!(self.heap.get(id), Value::AstWalk(_)) {
            return stdlib::ast::advance(self, id);
        }
        if !self.iterator_has_item(id)? {
            return Ok(None);
        }
        let item = match self.iterator_remaining(id)? {
            IteratorSlice::Values(values) => values[0],
            IteratorSlice::Reversed(values) => values[values.len() - 1],
            IteratorSlice::Bytes(bytes) => self.int(i64::from(bytes[0])),
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
            Value::AstWalk(_) | Value::DictionaryIterator(_)
        ) {
            let mut items = Vec::new();
            while let Some(item) = self.next_item(id)? {
                items.push(item);
            }
            return Ok(items);
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
            Value::DictView { dict, kind } => builtins::view_items(self, dict, kind),
            Value::Str(s) => Ok(s.chars().map(|c| self.text(c.to_string())).collect()),
            Value::Bytes(s) | Value::ByteArray(s) => {
                Ok(s.into_iter().map(|b| self.int(i64::from(b))).collect())
            }
            Value::Iterator { .. } => {
                let values = match self.iterator_remaining(id)? {
                    IteratorSlice::Values(values) => values.to_vec(),
                    IteratorSlice::Reversed(values) => values.iter().rev().copied().collect(),
                    IteratorSlice::Bytes(bytes) => {
                        let mut values = Vec::with_capacity(bytes.len());
                        while let Some(byte) = self.next_item(id)? {
                            values.push(byte);
                        }
                        return Ok(values);
                    }
                };
                if let Value::Iterator { sequence, .. } = self.heap.get_mut(id) {
                    *sequence = None;
                }
                Ok(values)
            }
            Value::Instance {
                dictionary: Some(_),
                ..
            } => {
                let iterator = self.iterator(id)?;
                self.items(iterator)
            }
            Value::Instance { class, fields, .. } if self.namedtuple_fields(class).is_some() => {
                Ok(self
                    .namedtuple_fields(class)
                    .unwrap()
                    .into_iter()
                    .map(|k| fields[&k])
                    .collect())
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
            class_cell: None,
            first_argument: None,
            names: Default::default(),
            namespace: None,
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
    fn scope_name(&self, scope: Id, name: &str) -> Option<Id> {
        let scope = self.scope_data(scope);
        if let Some(namespace) = scope.namespace {
            return self
                .dict_entries(namespace)?
                .iter()
                .find_map(|&(key, value)| {
                    matches!(self.heap.get(key), Value::Str(key) if key == name).then_some(value)
                });
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

    fn bind(&mut self, scope: Id, name: &str, value: Id) {
        if let Some(namespace) = self.scope_data(scope).namespace {
            let key = self.text(name);
            self.put_item(namespace, key, value)
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
            let s = self.scope_data(scope);
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
            "str" => builtins::STR_METHODS,
            _ => &[],
        };
        for method in methods {
            self.builtin(&format!("{name}.{method}"));
        }
        value
    }
    fn execute(&mut self, code: Id, scope: Id) -> Result<Id> {
        let entry = self.code_object(code).entry;
        self.heap.enter(scope, code);
        // SAFETY: a compiler-generated initializer or function uses this ABI and
        // returns before this Context can move or outlive the native module.
        let result = unsafe { entry(self, scope) };
        self.heap.leave(result);
        self.pending.take().map_or(Ok(result), Err)
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
            None
        } else {
            let (search, child) = if let Some((parent, child)) = parent {
                (
                    self.lookup_attr(parent, "__path__").map_err(|_| {
                        Error::new(
                            "ModuleNotFoundError",
                            format!("parent of {name:?} is not a package"),
                        )
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
                format!("no module named {name:?}"),
            ));
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
        let module = self.alloc(Value::Module {
            name: name.to_owned(),
            scope,
        });
        self.modules.insert(name.to_owned(), module);
        match name {
            "io" => stdlib::io::install(self, scope),
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
            if locations.contains(&path) {
                self.bind(scope, "__file__", NONE);
            } else if name != "__main__" || !self.config["command"].as_bool().unwrap_or(false) {
                let file = self.text(path.to_string_lossy());
                self.bind(scope, "__file__", file);
            }
            let filename = self.text(path.to_string_lossy());
            let name_id = self.text("<module>");
            let code = self.alloc(Value::Code(value::CodeObject {
                entry,
                parameters: NONE,
                name: name_id,
                filename,
                first_line: 1,
                module_definition: true,
            }));
            if let Err(error) = self.execute(code, scope) {
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
