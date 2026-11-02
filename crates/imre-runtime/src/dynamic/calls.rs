use crate::objects::HashState;

use super::wrapper_descriptor::BuiltinDescriptor;
use super::*;

const KEY_WRAPPER: &str = "functools.KeyWrapper";

pub(super) enum MappingOperation {
    Copy,
    Update,
    Unpack,
    Keywords(Id),
}

#[derive(Default)]
struct InstanceLayout {
    dictionary: bool,
    declared_slot: bool,
    active_slot: bool,
}

/// Metadata for native callables owned by these module objects.
pub(super) fn module_function(symbol: &str) -> Option<(&str, &str)> {
    if HashState::is_provider_class(symbol) {
        return None;
    }
    let (module, function) = symbol.split_once('.')?;
    match module {
        "posix"
            if !function.contains('.')
                && !matches!(
                    symbol,
                    stdlib::os::scandir::ENTRY | stdlib::os::scandir::ITER
                ) =>
        {
            Some((module, function))
        }
        "functools" if matches!(function, "cmp_to_key" | "reduce") => {
            Some(("_functools", function))
        }
        "codecs" if matches!(function, "encode" | "escape_decode") => Some(("_codecs", function)),
        "_thread" if matches!(function, "allocate" | "allocate_lock") => Some((module, function)),
        #[cfg(target_os = "linux")]
        "_signal" if function == "sigpending" => Some((module, function)),
        "_contextvars" if function == "copy_context" => Some((module, function)),
        "_io" if function == "text_encoding" => Some((module, function)),
        #[cfg(target_os = "linux")]
        "resource" if function == "getrlimit" => Some((module, function)),
        "gc" if matches!(function, "enable" | "disable" | "isenabled") => Some((module, function)),
        "zlib" if matches!(function, "adler32" | "crc32") => Some((module, function)),
        "hashlib"
            if matches!(function, "new" | "file_digest")
                || crate::objects::HashState::algorithms().any(|name| name == function) =>
        {
            Some((module, function))
        }
        "_thread" if super::lowlevel_threads::function_name(symbol).is_some() => {
            Some((module, function))
        }
        _ => None,
    }
}

fn excess_positional_error(
    name: &str,
    allowed: usize,
    defaults: usize,
    given: usize,
    keywords: usize,
) -> Error {
    let limit = if defaults == 0 {
        allowed.to_string()
    } else {
        format!("from {} to {allowed}", allowed as isize - defaults as isize)
    };
    let unit = if allowed == 1 && defaults == 0 {
        "argument"
    } else {
        "arguments"
    };
    let (count, verb) = if keywords == 0 {
        (given.to_string(), if given == 1 { "was" } else { "were" })
    } else {
        let positional_unit = if given == 1 { "argument" } else { "arguments" };
        let keyword_unit = if keywords == 1 {
            "argument"
        } else {
            "arguments"
        };
        (
            format!(
                "{given} positional {positional_unit} (and {keywords} keyword-only {keyword_unit})"
            ),
            "were",
        )
    };
    Error::new(
        "TypeError",
        format!("{name}() takes {limit} positional {unit} but {count} {verb} given"),
    )
}

fn missing_arguments_error(name: &str, kind: &str, missing: &[&str]) -> Error {
    let count = missing.len();
    let unit = if count == 1 { "argument" } else { "arguments" };
    let names = match missing {
        [one] => format!("'{one}'"),
        [one, two] => format!("'{one}' and '{two}'"),
        [rest @ .., last] => format!("'{}', and '{last}'", rest.join("', '")),
        [] => unreachable!(),
    };
    Error::new(
        "TypeError",
        format!("{name}() missing {count} required {kind} {unit}: {names}"),
    )
}

/// Build a non-callable diagnostic while keeping allocation failure catchable.
fn non_callable_error(name: &str) -> Error {
    let suffix = "' object is not callable";
    let mut message = String::new();
    if message
        .try_reserve_exact(1 + name.len() + suffix.len())
        .is_err()
    {
        return Error::new("MemoryError", "");
    }
    message.push('\'');
    message.push_str(name);
    message.push_str(suffix);
    Error::new_static("TypeError", message)
}

impl Context {
    pub(super) fn class_attr(&self, class: Id, name: &str) -> Option<Id> {
        if let Some(symbol) = builtins::base_exception_method(name) {
            if matches!(self.heap.get(class), Value::Native(kind) if builtins::is_exception(kind)) {
                return self.builtins.get(symbol).copied();
            }
            if let Some(mro) = self.published_mro(class) {
                return mro.iter().find_map(|&base| self.own_class_attr(base, name));
            }
        }
        if name == "__hash__" {
            if matches!(self.heap.get(class), Value::Native(_)) {
                return self.own_class_attr(class, name);
            }
            return self
                .find_in_mro(class, |base| {
                    let slot = self.own_class_attr(base, name);
                    // Modules and exceptions inherit object.__hash__ after any intervening mixins.
                    let native_hash = matches!(self.heap.get(base), Value::Native(kind)
                        if kind != "module" && !builtins::is_exception(kind));
                    (slot.is_some() || native_hash).then_some(slot)
                })
                .flatten();
        }
        let attr = self.find_in_mro(class, |base| self.own_class_attr(base, name));
        if attr.is_none()
            && name == "__reduce_ex__"
            && self.own_class_attr(class, "__reduce__").is_some()
        {
            // Exact native classes have no stored MRO but inherit object's delegating reducer.
            return self.builtins.get("object.__reduce_ex__").copied();
        }
        attr
    }

    /// Visit the class and its bases in Python's method resolution order.
    pub(super) fn find_in_mro<T>(
        &self,
        class: Id,
        mut lookup: impl FnMut(Id) -> Option<T>,
    ) -> Option<T> {
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return lookup(class).or_else(|| {
                if matches!(self.heap.get(class), Value::Native(name) if name == "datetime.datetime") {
                    self.builtins.get("datetime.date").and_then(|&base| lookup(base))
                } else {
                    None
                }
            });
        };
        std::iter::once(class)
            .chain(ancestors.iter().copied())
            .find_map(lookup)
    }

    pub(super) fn own_class_attr(&self, class: Id, name: &str) -> Option<Id> {
        match self.heap.get(class) {
            Value::Class { scope, .. } => self.namespace_binding(self.scope_data(*scope), name),
            Value::Native(kind) if kind == "object" => match name {
                "__hash__" => self.builtins.get("object.__hash__").copied(),
                "__format__" => self.builtins.get("object.__format__").copied(),
                "__getstate__" => self.builtins.get("object.__getstate__").copied(),
                "__reduce__" => self.builtins.get("object.__reduce__").copied(),
                "__reduce_ex__" => self.builtins.get("object.__reduce_ex__").copied(),
                "__init_subclass__" => self.builtins.get("object.__init_subclass__").copied(),
                _ => None,
            },
            Value::Native(kind) if kind == "collections.defaultdict" => {
                let owner = match name {
                    "__init__" | "__missing__" | "__repr__" | "copy" | "__copy__"
                    | "__reduce__" => kind.as_str(),
                    _ => "dict",
                };
                self.builtins.get(&format!("{owner}.{name}")).copied()
            }
            Value::Native(kind) if builtins::is_enum_base(kind) => {
                self.builtins.get(&format!("enum.Enum.{name}")).copied()
            }
            Value::Native(kind)
                if name == "__hash__"
                    && matches!(
                        kind.as_str(),
                        "list" | "dict" | "set" | "bytearray" | KEY_WRAPPER
                    ) =>
            {
                Some(NONE)
            }
            Value::Native(kind) if builtins::is_exception(kind) => match name {
                name if kind == "BaseException"
                    && let Some(symbol) = builtins::base_exception_method(name) =>
                {
                    self.builtins.get(symbol).copied()
                }
                "__new__" => self
                    .builtins
                    .get(&format!("{}.__new__", builtins::exception_new_owner(kind)))
                    .copied(),
                "__init__" => self
                    .builtins
                    .get(builtins::exception_initializer(kind))
                    .copied(),
                _ => None,
            },
            Value::Native(kind)
                if matches!(
                    kind.as_str(),
                    "dict"
                        | "list"
                        | "tuple"
                        | "bool"
                        | "int"
                        | "float"
                        | "bytes"
                        | "bytearray"
                        | "memoryview"
                        | "str"
                        | "property"
                        | "module"
                        | weakref::REFERENCE_TYPE
                ) || stdlib::datetime::members(kind)
                    .is_some_and(|members| members.owns(name))
                    || (kind == "function" && name == "__call__")
                    || (kind == "set" && name == "update")
                    || (matches!(name, "__repr__" | "__reduce__" | "issubset")
                        && matches!(kind.as_str(), "set" | "frozenset")) =>
            {
                self.builtins.get(&format!("{kind}.{name}")).copied()
            }
            _ => None,
        }
    }
    /// Prepare dictionary deletion before removing the oldest cached call.
    fn evict_cached_call(
        &mut self,
        callable: Id,
        table: Id,
        capacity: usize,
    ) -> Result<Option<value::CachedCall>> {
        let Value::Cached { entries, .. } = self.heap.get(callable) else {
            unreachable!()
        };
        if entries.len() < capacity {
            return Ok(None);
        }
        let Value::Dict(table) = self.heap.get_mut(table) else {
            unreachable!()
        };
        // The dictionary can refuse its first deletion. Keep the eviction order intact on failure.
        table.prepare_removal()?;
        let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
            unreachable!()
        };
        Ok(Some(entries.remove(0)))
    }

    /// Borrow the native member storage of an exception or a Python subclass.
    fn exception_storage(&mut self, object: Id, kind: &str) -> Option<&mut Error> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        if !self.is_builtin_error(error, kind) {
            return None;
        }
        let Value::Raised(error) = self.heap.get_mut(object) else {
            unreachable!()
        };
        Some(error)
    }

    /// Lazily allocate the exact dictionary exposed through BaseException.__dict__.
    fn exception_dictionary(&mut self, obj: Id) -> Id {
        let Value::Raised(error) = self.heap.get(obj) else {
            unreachable!()
        };
        if let Some(dictionary) = error.trace.as_ref().and_then(|trace| trace.dictionary) {
            return dictionary;
        }
        let dictionary = self.dict(Vec::new());
        let Value::Raised(error) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        error.tracing().dictionary = Some(dictionary);
        dictionary
    }

    /// Keep the dictionary absent until first used, so wrapping an untouched partial can flatten it.
    fn partial_dictionary(&mut self, obj: Id, dictionary: Option<Id>) -> Id {
        if let Some(dictionary) = dictionary {
            return dictionary;
        }
        let dictionary = self.dict(Vec::new());
        let Value::Partial {
            dictionary: field, ..
        } = self.heap.get_mut(obj)
        else {
            unreachable!()
        };
        *field = Some(dictionary);
        dictionary
    }

    /// Instance attributes use raw dict storage even when __dict__ was set to a subclass.
    fn dictionary_field(&mut self, dictionary: Option<Id>, name: &str) -> Result<Option<Id>> {
        let Some(dictionary) = dictionary else {
            return Ok(None);
        };
        let storage = self.dict_storage(dictionary).expect("instance dictionary");
        let key = self.text(name);
        let (_, index) = self.find_dictionary_key(storage, key, None)?;
        Ok(index.map(|i| self.dict_entries(storage).expect("instance dictionary")[i].1))
    }

    fn update_exception_field(&mut self, obj: Id, name: &str, value: Option<Id>) -> Result<()> {
        let dictionary = if value.is_some() {
            self.exception_dictionary(obj)
        } else {
            let Value::Raised(error) = self.heap.get(obj) else {
                unreachable!()
            };
            let Some(dictionary) = error.trace.as_ref().and_then(|trace| trace.dictionary) else {
                return Err(self.missing_attribute(obj, name));
            };
            dictionary
        };
        self.update_dictionary_field(obj, dictionary, name, value)
    }

    fn update_dictionary_field(
        &mut self,
        obj: Id,
        dictionary: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        let storage = self.dict_storage(dictionary).expect("instance dictionary");
        if let Some(value) = value {
            let key = self.text(name);
            self.put_item(storage, key, value)
        } else {
            self.delete_dict_attribute(obj, storage, name)
        }
    }

    /// Native member descriptors take precedence over entries in the instance dictionary.
    fn native_exception_descriptor(&self, error: &Error, name: &str) -> bool {
        let inherits = |kind| {
            self.builtins
                .get(kind)
                .is_some_and(|class| self.raised_error_matches(error, *class))
        };
        match name {
            "args"
            | "__class__"
            | "__context__"
            | "__cause__"
            | "__traceback__"
            | "__suppress_context__" => true,
            "value" => inherits("StopIteration"),
            "msg" => inherits("ImportError") || inherits("SyntaxError"),
            "name" => {
                inherits("ImportError") || inherits("AttributeError") || inherits("NameError")
            }
            "obj" => inherits("AttributeError"),
            "path" | "name_from" => inherits("ImportError"),
            "code" => inherits("SystemExit"),
            "filename" => inherits("OSError") || inherits("SyntaxError"),
            "errno" | "strerror" | "filename2" | "characters_written" => inherits("OSError"),
            "lineno"
            | "offset"
            | "text"
            | "end_lineno"
            | "end_offset"
            | "print_file_and_line"
            | "_metadata" => inherits("SyntaxError"),
            "encoding" | "object" | "start" | "end" | "reason" => {
                inherits("UnicodeEncodeError")
                    || inherits("UnicodeDecodeError")
                    || inherits("UnicodeTranslateError")
            }
            "message" | "exceptions" => inherits("BaseExceptionGroup"),
            _ => false,
        }
    }

    /// Find a Python-declared member on an exception that retains its original class.
    fn exception_member(&self, object: Id, name: &str) -> Option<Id> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        self.class_attr(error.class()?, name)
    }

    /// Bind an override only if it precedes the built-in numeric method in the MRO.
    pub(super) fn bound_numeric_slot(&mut self, value: Id, slot: &str) -> Result<Option<Id>> {
        let class = match *self.heap.get(value) {
            Value::Instance { class, .. } => class,
            Value::Raised(_) => self.exact_type(value)?,
            _ => return Ok(None),
        };
        let method = self.find_in_mro(class, |base| match self.heap.get(base) {
            Value::Native(name) if matches!(name.as_str(), "int" | "float") => Some(None),
            _ => self.own_class_attr(base, slot).map(Some),
        });
        method
            .flatten()
            .map(|method| self.bind_attribute(method, value, class))
            .transpose()
    }

    /// Special methods are looked up on the type, even if the instance shadows them.
    pub(super) fn slot(&self, object: Id, name: &str) -> Option<Id> {
        self.class_attr(self.heap.get(object).instance_class()?, name)
    }

    /// Construct a cache with an independently traced dictionary and keyword marker.
    pub(super) fn cached_function(&mut self, function: Id) -> Id {
        self.new_cache(function, Some(value::CACHE_CAPACITY), false)
    }

    pub(super) fn new_cache(&mut self, function: Id, capacity: Option<usize>, typed: bool) -> Id {
        let mark = self.heap.temporary_mark();
        self.heap.keep(function);
        let table = self.dict(Vec::new());
        self.heap.keep(table);
        let marker = self.alloc(Value::Native("functools._cache_marker".into()));
        self.heap.keep(marker);
        let dictionary = self.dict(Vec::new());
        self.heap.keep(dictionary);
        let result = self.alloc(Value::Cached {
            function,
            capacity,
            typed,
            hits: 0,
            misses: 0,
            dictionary,
            entries: Vec::new(),
            table,
            marker,
        });
        self.heap.release_temporaries_since(mark);
        result
    }

    /// Return dictionary storage when the receiver still uses the builtin iterator.
    pub(super) fn raw_dict_source(&self, value: Id) -> Option<Id> {
        self.dict_storage(value).filter(|_| {
            self.slot(value, "__iter__").is_none_or(
                |slot| matches!(self.heap.get(slot), Value::Native(name) if name == "dict.__iter__"),
            )
        })
    }

    /// CPython reports the qualified callable in errors while unpacking its keywords.
    pub(super) fn keyword_callable(&mut self, callable: Id) -> Result<String> {
        let name = match self.lookup_attr(callable, "__qualname__") {
            Ok(id) => {
                let name = builtins::call(self, "str", &[id], &[])?;
                self.str_value(name)?.to_owned()
            }
            Err(e) if self.is_attribute_error(&e) => {
                let repr = builtins::call(self, "repr", &[callable], &[])?;
                return Ok(self.str_value(repr)?.to_owned());
            }
            Err(e) => return Err(e),
        };
        match self.lookup_attr(callable, "__module__") {
            Ok(id) if id != NONE && self.str_value(id).ok() != Some("builtins") => {
                let module = builtins::call(self, "str", &[id], &[])?;
                Ok(format!("{}.{name}()", self.str_value(module)?))
            }
            Ok(_) => Ok(format!("{name}()")),
            Err(e) if self.is_attribute_error(&e) => Ok(format!("{name}()")),
            Err(e) => Err(e),
        }
    }

    /// Dict copying uses raw entries for a dict with its original iterator.
    /// An overridden iterator opts into the mapping protocol, including key lookup.
    pub(super) fn for_each_mapping_item(
        &mut self,
        input: Id,
        target: Id,
        operation: MappingOperation,
        mut visit: impl FnMut(&mut Self, Id, Id, Option<i64>) -> Result<()>,
    ) -> Result<bool> {
        let check_duplicate = |ctx: &mut Self, key, hash| -> Result<()> {
            if let MappingOperation::Keywords(callable) = operation
                && ctx.find_dictionary_key(target, key, hash)?.1.is_some()
            {
                let name = ctx.keyword_callable(callable)?;
                let key = builtins::call(ctx, "str", &[key], &[])?;
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{name} got multiple values for keyword argument '{}'",
                        ctx.str_value(key)?
                    ),
                ));
            }
            Ok(())
        };
        // dict.update probes for keys before choosing a merge or a sequence of pairs.
        // A later mapping lookup may return a different method or raise.
        if matches!(operation, MappingOperation::Update)
            && !matches!(self.heap.get(input), Value::Dict(_))
            && let Err(error) = self.lookup_attr(input, "keys")
        {
            return if error.kind == "AttributeError" {
                Ok(false)
            } else {
                Err(error)
            };
        }
        if let Some(storage) = self.raw_dict_source(input) {
            if storage == target {
                return Ok(true);
            }
            let used = self
                .dict_entries(storage)
                .expect("dictionary storage")
                .len();
            let mut position = 0;
            while let Some((current, key, value, hash)) = self.dictionary_pair(storage, position) {
                self.heap.keep(key);
                self.heap.keep(value);
                check_duplicate(self, key, hash)?;
                visit(self, key, value, hash)?;
                if self
                    .dict_entries(storage)
                    .expect("dictionary storage")
                    .len()
                    != used
                {
                    return Err(Error::new("RuntimeError", "dict mutated during update"));
                }
                position = current + 1;
            }
            return Ok(true);
        }
        if matches!(
            self.heap.get(input),
            Value::List(_)
                | Value::Tuple(_)
                | Value::Set { .. }
                | Value::Str(_)
                | Value::Bytes(_)
                | Value::Range(_)
                | Value::Iterator { .. }
                | Value::DictIterator(_)
        ) {
            return Ok(false);
        }
        let keys = match self.lookup_attr(input, "keys") {
            Ok(keys) => keys,
            Err(error)
                if error.kind == "AttributeError"
                    && matches!(
                        operation,
                        MappingOperation::Unpack | MappingOperation::Keywords(_)
                    ) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let result = self.call(keys, Vec::new(), Vec::new())?;
        // PyMapping_Keys keeps an exact list returned by keys() and materializes other results.
        let keys = if matches!(self.heap.get(result), Value::List(_)) {
            result
        } else {
            let items = self.items(result)?;
            self.alloc(Value::List(items))
        };
        let iterator = self.iterator(keys)?;
        while let Some(key) = self.next_item(iterator)? {
            self.heap.keep(key);
            check_duplicate(self, key, None)?;
            let value = self.get_item(input, key)?;
            visit(self, key, value, None)?;
        }
        Ok(true)
    }

    pub(super) fn bound_slot(&mut self, object: Id, name: &str) -> Result<Option<Id>> {
        let Some(class) = self.heap.get(object).instance_class() else {
            return Ok(None);
        };
        self.class_attr(class, name)
            .map(|slot| self.bind_attribute(slot, object, class))
            .transpose()
    }

    pub(super) fn callable(&self, value: Id) -> bool {
        matches!(
            self.heap.get(value),
            Value::Class { .. }
                | Value::GenericAlias(_)
                | Value::Function { .. }
                | Value::AttrGetter(_)
                | Value::ItemGetter(_)
                | Value::StaticMethod { .. }
                | Value::CacheParameters { .. }
                | Value::CacheDecorator { .. }
                | Value::KeyWrapper { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::Native(_)
                | Value::CodecDecoder(_)
                | Value::IoMethod { .. }
                | Value::Method { .. }
        ) || matches!(self.heap.get(value),Value::Weak(wr) if wr.kind != weakref::Kind::Proxy)
            || self.slot(value, "__call__").is_some()
    }

    pub(super) fn native_data_descriptor(&self, descriptor: Id) -> bool {
        matches!(
            self.heap.get(descriptor),
            Value::Property(_)
                | Value::TupleGetter { .. }
                | Value::TupleGetterDoc
                | Value::ReadOnlyMember(_, _)
                | Value::SlotMember { .. }
                | Value::GetSetDescriptor { .. }
                | Value::WeakrefDescriptor(_)
        )
    }

    fn data_descriptor(&self, descriptor: Id) -> bool {
        self.native_data_descriptor(descriptor)
            || self.slot(descriptor, "__set__").is_some()
            || self.slot(descriptor, "__delete__").is_some()
    }

    fn readable_data_descriptor(&self, member: Id) -> bool {
        self.native_data_descriptor(member)
            || (self.slot(member, "__get__").is_some() && self.data_descriptor(member))
    }

    /// Bind a native class method to the class through which it was retrieved.
    fn bind_native_classmethod(&mut self, callable: Id, owner: Id) -> Id {
        self.alloc(Value::Method {
            callable,
            receiver: owner,
            explicit: false,
        })
    }

    pub(super) fn bind_attribute(&mut self, value: Id, receiver: Id, owner: Id) -> Result<Id> {
        if matches!(self.heap.get(value), Value::Native(name) if matches!(name.as_str(), "dict.fromkeys" | "int.from_bytes" | "float.__getformat__" | "bytes.fromhex" | "bytearray.fromhex"))
        {
            return Ok(self.bind_native_classmethod(value, owner));
        }
        if matches!(self.heap.get(value), Value::Native(name) if name == "object.__init_subclass__")
        {
            return Ok(self.alloc(Value::Method {
                callable: value,
                receiver: owner,
                explicit: false,
            }));
        }
        if let Value::ClassMethod(callable) = *self.heap.get(value) {
            return Ok(self.alloc(Value::Method {
                callable,
                receiver: owner,
                explicit: true,
            }));
        }
        if let Some(get) = self.slot(value, "__get__") {
            return self.call(get, vec![value, receiver, owner], Vec::new());
        }
        if let Value::StaticMethod { callable, .. } = *self.heap.get(value) {
            return Ok(callable);
        }
        if let Value::Native(symbol) = self.heap.get(value)
            && let Some(slot) = BuiltinDescriptor::lookup(symbol)
        {
            return slot.bind(self, value, receiver);
        }
        if receiver != NONE {
            let method = match self.heap.get(value) {
                Value::SlotMember { .. } => return self.read_slot_member(value, receiver),
                Value::TupleGetterDoc => return super::namedtuple::document(self, receiver),
                Value::ReadOnlyMember(owner, field) => {
                    return self.read_member(owner, field, receiver);
                }
                Value::TupleGetter { index, .. } => {
                    return super::namedtuple::read_field(self, *index, receiver);
                }
                Value::Property(property) => return property.read(self, receiver),
                Value::WeakrefDescriptor(owner) => return self.weakref_list(*owner, receiver),
                Value::GetSetDescriptor { get, .. } => {
                    return self.call(*get, vec![receiver], Vec::new());
                }
                Value::Function { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::IoMethod { .. } => true,
                Value::Native(name) => {
                    matches!(
                        name.as_str(),
                        "ast.AST.__init__"
                            | "bytes.startswith"
                            | "object.__getattribute__"
                            | "enum.auto.__init__"
                            | "enum.auto.__repr__"
                            | "enum.Enum.__copy__"
                            | "enum.Enum.__deepcopy__"
                            | "object.__setattr__"
                            | "object.__delattr__"
                            | "object.__init__"
                            | "BaseException.__init__"
                            | "ImportError.__init__"
                            | "OSError.__init__"
                            | "StopIteration.__init__"
                            | "module.__init__"
                            | "module.__dir__"
                            | "module.__repr__"
                            | "module.__getattribute__"
                            | "object.__str__"
                            | "bytes.decode"
                            | "bytes.rfind"
                            | "bytes.rsplit"
                            | "bytes.rstrip"
                            | "int.bit_length"
                            | "int.to_bytes"
                            | "list.index"
                            | "list.clear"
                            | "list.copy"
                            | "list.reverse"
                            | "list.sort"
                            | "map.__iter__"
                            | "map.__next__"
                    ) || name
                        .strip_prefix("weakref.ReferenceType.")
                        .is_some_and(|method| {
                            method != "__new__" && weakref::REFERENCE_METHODS.contains(&method)
                        })
                        || name.starts_with("collections.defaultdict.")
                        || name.starts_with("property.")
                        || (name.starts_with("dict.") && name != "dict.__new__")
                        || name.starts_with("abc.")
                        || name
                            .strip_prefix("str.")
                            .is_some_and(|method| builtins::STR_METHODS.contains(&method))
                }
                _ => false,
            };
            if method {
                return Ok(self.alloc(Value::Method {
                    callable: value,
                    receiver,
                    explicit: matches!(self.heap.get(value), Value::Cached { .. }),
                }));
            }
        }
        Ok(value)
    }

    pub(super) fn instance_attribute(
        &mut self,
        object: Id,
        class: Id,
        field: Option<Id>,
        name: &str,
    ) -> Result<Id> {
        // The class namespace holds type.__qualname__ for native class lookup.
        // An instance can supply its own value, but cannot inherit that metadata.
        let member = if name == "__qualname__" {
            None
        } else {
            self.class_attr(class, name)
        };
        if member.is_none()
            && matches!(name, "fget" | "fset" | "fdel" | "__name__")
            && let Some(property) = self.heap.get(object).property().copied()
            && let Some(value) = property.attribute(self, name)?
        {
            return Ok(value);
        }
        match (member, field) {
            (Some(member), _) if self.readable_data_descriptor(member) => {
                self.bind_attribute(member, object, class)
            }
            // Without a Python class member, object.__class__ is a data descriptor.
            (None, _) if name == "__class__" => Ok(class),
            (None, _)
                if self.exposes_defaultdict_factory(object, name)
                    && let Some(factory) = self.defaultdict_factory(object) =>
            {
                Ok(factory)
            }
            (None, _)
                if name == "__callback__"
                    && let Some(wr) = self.heap.get(object).weak_reference() =>
            {
                Ok(wr.callback)
            }
            (_, Some(field)) => Ok(field),
            (Some(member), None) => self.bind_attribute(member, object, class),
            (None, None) if matches!(name, "__getattribute__" | "__delattr__") => {
                let method = self.builtin(&format!("object.{name}"));
                self.bind_attribute(method, object, class)
            }
            (None, None) => self.builtin_attribute(object, name),
        }
    }

    pub(super) fn python_subclass_check(&mut self, child: Id, parent: Id) -> Result<bool> {
        if let Value::Union(union) = self.heap.get(parent) {
            return self.python_subclass_check(child, union.args);
        }
        if let Value::Tuple(classes) = self.heap.get(parent) {
            for class in classes.clone() {
                if self.python_subclass_check(child, class)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if let Value::GenericAlias(alias) = self.heap.get(parent) {
            return Err(alias.kind.class_check_error("issubclass"));
        }
        if let Some(check) = self.bound_slot(parent, "__subclasscheck__")? {
            let result = self.call(check, vec![child], Vec::new())?;
            return self.truth(result);
        }
        let is_class = |id| {
            matches!(self.builtin_type_name(id), Some("type" | "typing._AnyMeta"))
                || matches!(self.heap.get(id), Value::Native(name) if super::builtins::is_builtin_type(name))
        };
        if !is_class(child) {
            return Err(Error::new(
                "TypeError",
                "issubclass() arg 1 must be a class",
            ));
        }
        if !is_class(parent) {
            return Err(Error::new(
                "TypeError",
                "issubclass() arg 2 must be a class, a tuple of classes, or a union",
            ));
        }
        self.check_runtime_protocol(parent, true)?;
        self.abc_subclass_check(child, parent)
    }

    /// Consult ABC hooks and registrations without the public protocol argument guards.
    pub(super) fn abc_subclass_check(&mut self, child: Id, parent: Id) -> Result<bool> {
        let protocol = self.is_protocol_declaration(parent);
        let key = [parent, child];
        if protocol && let Some(&result) = self.heap.protocol_subclass_cache.get(&key) {
            return Ok(result);
        }
        let result = if self.has_abc_subclass_hook(parent)
            && let Some(matched) = self.abc_hook_result(child, parent)?
        {
            matched
        } else {
            self.subclass(child, parent)
        };
        if protocol {
            self.heap
                .protocol_subclass_cache
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            self.heap.protocol_subclass_cache.insert(key, result);
        }
        Ok(result)
    }

    pub(super) fn subclass(&self, class: Id, target: Id) -> bool {
        self.direct_subclass(class, target)
            || super::mapping_abc::registered_subclass(self, class, target)
    }

    /// Test native or C3 inheritance without traversing the ABC registry.
    pub(super) fn direct_subclass(&self, class: Id, target: Id) -> bool {
        if self.equal_class(class, target) || self.structural_context_manager(class, target) {
            return true;
        }
        if let Value::Class { ancestors, .. } = self.heap.get(class) {
            return ancestors.iter().any(|&base| {
                self.equal_class(base, target)
                    || (matches!(self.heap.get(base), Value::Native(_))
                        && self.direct_subclass(base, target))
            });
        }
        if let (Value::Native(child), Value::Native(parent)) =
            (self.heap.get(class), self.heap.get(target))
        {
            if (child == stdlib::flags::TYPE
                || stdlib::fixed_structseq::fixed_type(child).is_some())
                && parent == "tuple"
            {
                return true;
            }
            if parent == "object" {
                return self.builtin_type_name(class) == Some("type");
            }
            if let Some(result) = builtins::exception_subclass(child, parent) {
                return result;
            }
            return matches!(
                (child.as_str(), parent.as_str()),
                ("bool", "int")
                    | ("datetime.datetime", "datetime.date")
                    | ("datetime.timezone", stdlib::datetime::TZINFO)
                    | ("typing._UnpackGenericAlias", "typing._GenericAlias")
                    | (HashState::XOF_CLASS, HashState::CLASS)
                    | (stdlib::os::stat::CLASS, "tuple")
                    | ("collections.defaultdict", "dict")
                    | ("odict_keys", "dict_keys")
                    | ("odict_items", "dict_items")
                    | ("odict_values", "dict_values")
                    | ("enum.IntEnum" | "enum.IntFlag", "int" | "enum.Enum")
                    | ("enum.StrEnum", "str" | "enum.Enum")
            );
        }
        false
    }
    fn equal_class(&self, a: Id, b: Id) -> bool {
        a == b
            || matches!((self.heap.get(a),self.heap.get(b)),(Value::Native(x),Value::Native(y)) if x==y)
    }
    pub(super) fn has_base(&self, class: Id, target: &str) -> bool {
        match self.heap.get(class) {
            Value::Class { ancestors, .. } => ancestors
                .iter()
                .any(|&b| matches!(self.heap.get(b), Value::Native(name) if name == target)),
            Value::Native(name) => name == target,
            _ => false,
        }
    }
    pub(super) fn namedtuple_fields(&self, class: Id) -> Option<std::rc::Rc<[String]>> {
        match self.heap.get(class) {
            Value::Class {
                namedtuple_fields, ..
            } => namedtuple_fields.clone(),
            _ => None,
        }
    }
    pub(super) fn readonly_member(&mut self, owner: &'static str, field: &'static str) -> Id {
        let symbol = format!("{owner}.{field}");
        if let Some(&id) = self.builtins.get(&symbol) {
            return id;
        }
        let descriptor = self.alloc(Value::ReadOnlyMember(owner, field));
        self.builtins.insert(symbol, descriptor);
        descriptor
    }

    pub(super) fn read_member(&mut self, owner: &str, field: &str, receiver: Id) -> Result<Id> {
        if owner == "datetime.timedelta" {
            return stdlib::datetime::read_field(self, owner, field, receiver);
        }
        let ty = self.exact_type(receiver)?;
        if ty == self.builtin(owner) {
            self.lookup_attr(receiver, field)
        } else {
            Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{field}' for '{owner}' objects doesn't apply to a '{}' object",
                    self.type_name(receiver)
                ),
            ))
        }
    }

    /// Check whether an inline value is an immutable tuple position.
    fn has_namedtuple_field(&self, class: Id, name: &str) -> bool {
        matches!(self.heap.get(class), Value::Class { namedtuple_fields: Some(names), .. } if names.iter().any(|field| field == name))
    }

    pub(super) fn builtin_type_name(&self, obj: Id) -> Option<&str> {
        Some(match self.heap.get(obj) {
            Value::Module { .. } => self.type_name(obj),
            Value::DefaultDict { .. } => "collections.defaultdict",
            Value::Partial { .. } => "functools.partial",
            Value::Cached { .. } => "functools._lru_cache_wrapper",
            Value::KeyWrapper { .. } => KEY_WRAPPER,
            Value::PartialPlaceholder => "functools._PlaceholderType",
            Value::Count { .. } => "itertools.count",
            Value::Repeat(_) => "itertools.repeat",
            Value::TupleGetter { .. } => super::namedtuple::GETTER,
            Value::TypeVar(_) => "typing.TypeVar",
            Value::TypeVarTuple { .. } => "typing.TypeVarTuple",
            Value::GenericAlias(alias) => alias.kind.class(),
            Value::Socket(_) => "socket.socket",
            Value::Union(_) => "typing.Union",
            Value::Super { .. } => "super",
            Value::Method { explicit: true, .. } => "method",
            Value::Method { callable, .. } => match self.heap.get(*callable) {
                Value::Function { .. } => "method",
                Value::Native(symbol)
                    if BuiltinDescriptor::lookup(symbol).is_some_and(|slot| !slot.method) =>
                {
                    "method-wrapper"
                }
                Value::IoMethod {
                    name: "__init__" | "__repr__" | "__iter__" | "__next__" | "__del__",
                    ..
                } => "method-wrapper",
                Value::Native(_) | Value::IoMethod { .. } => "builtin_function_or_method",
                _ => return None,
            },
            Value::Native(name) if let Some(slot) = BuiltinDescriptor::lookup(name) => slot.class(),
            Value::Native(name) if module_function(name).is_some() => "builtin_function_or_method",
            Value::Native(name) if self.builtins.get(name) == Some(&obj) => {
                if name == "typing.Any" {
                    return Some("typing._AnyMeta");
                }
                if name == "typing.Callable" {
                    return Some("typing._CallableType");
                }
                if HashState::is_provider_class(name)
                    || builtins::is_builtin_type(name)
                    || builtins::is_iterator_type(name)
                    || name == stdlib::fixed_structseq::UNRAISABLE_ARGS
                    || matches!(
                        name.as_str(),
                        "NoneType"
                            | "NotImplementedType"
                            | "ellipsis"
                            | "float"
                            | "code"
                            | "coroutine"
                            | "cell"
                            | "function"
                            | "module"
                            | "method"
                            | "builtin_function_or_method"
                            | "method-wrapper"
                            | "wrapper_descriptor"
                            | "method_descriptor"
                            | "getset_descriptor"
                            | "member_descriptor"
                            | "dict_keys"
                            | "dict_items"
                            | "dict_values"
                            | "odict_keys"
                            | "odict_items"
                            | "odict_values"
                            | "mappingproxy"
                            | KEY_WRAPPER
                            | "functools._PlaceholderType"
                            | "functools._lru_cache_wrapper"
                            | "functools.partial"
                            | HashState::CLASS
                            | HashState::XOF_CLASS
                            | "operator.attrgetter"
                            | super::namedtuple::GETTER
                            | "operator.itemgetter"
                            | "datetime.date"
                            | "datetime.datetime"
                            | "datetime.time"
                            | "datetime.timedelta"
                            | "datetime.timezone"
                            | stdlib::datetime::TZINFO
                            | "collections.defaultdict"
                            | "_contextvars.Context"
                            | "_contextvars.ContextVar"
                            | "_contextvars.Token"
                            | stdlib::os::stat::CLASS
                            | "array.array"
                            | "selectors.PollSelector"
                            | "selectors.SelectSelector"
                            | super::deque::CLASS
                            | "weakref.ReferenceType"
                            | "weakref.ProxyType"
                            | "weakref.CallableProxyType"
                            | super::thread_local::CLASS
                            | stdlib::os::scandir::ENTRY
                            | stdlib::os::scandir::ITER
                            | super::thread_rlock::CLASS
                            | super::thread_lock::CLASS
                            | value::REGEX_PATTERN_TYPE
                            | value::REGEX_MATCH_TYPE
                            | value::TEMPLATE_TYPE
                            | value::TEMPLATE_ITER_TYPE
                            | value::INTERPOLATION_TYPE
                            | "traceback"
                            | "frame"
                            | "typing.TypeVar"
                            | "typing.TypeVarTuple"
                            | "typing._UnpackGenericAlias"
                            | "NoDefaultType"
                            | "typing._GenericAlias"
                            | "typing._LiteralGenericAlias"
                            | "typing._CallableGenericAlias"
                            | "typing._CallableType"
                            | "typing._AnyMeta"
                            | "types.GenericAlias"
                            | "collections.abc._CallableGenericAlias"
                    )
                {
                    "type"
                } else if builtins::is_builtin(name)
                    || builtins::constructor_owner(name).is_some()
                    || matches!(
                        name.as_str(),
                        "binascii.a2b_base64"
                            | "binascii.b2a_base64"
                            | "binascii.a2b_hex"
                            | "binascii.unhexlify"
                    )
                    || name.starts_with("_weakref.")
                {
                    "builtin_function_or_method"
                } else {
                    return None;
                }
            }
            value => return value.intrinsic_type_name(),
        })
    }

    /// Return the actual class independently of an instance's Python-level __class__ attribute.
    pub(super) fn exact_type(&mut self, obj: Id) -> Result<Id> {
        if let Some(name) = self.heap.get(obj).native_io_class_name() {
            if !self.builtins.contains_key("_io._IOBase") {
                self.module("_io")?;
            }
            return self.builtins.get(name).copied().ok_or_else(|| {
                Error::new("ImportError", format!("native class {name} is unavailable"))
            });
        }
        match self.heap.get(obj) {
            Value::Union(_) => Ok(super::union::class(self)),
            Value::Instance { class, .. }
            | Value::Class {
                metaclass: Some(class),
                ..
            }
            | Value::Module {
                constructor: Some(class),
                ..
            } => Ok(*class),
            Value::NamedTuple {
                class:
                    Some(
                        class @ ("time.struct_time"
                        | stdlib::fixed_structseq::UNRAISABLE_ARGS
                        | stdlib::fixed_structseq::HASH_INFO
                        | stdlib::fixed_structseq::TERMINAL_SIZE
                        | stdlib::flags::TYPE
                        | stdlib::os::stat::CLASS),
                    ),
                ..
            } => Ok(self.builtin(class)),
            Value::NamedTuple {
                class:
                    Some(
                        name @ ("SplitResult" | "SplitResultBytes" | "ParseResult"
                        | "ParseResultBytes"),
                    ),
                ..
            } => Ok(self.builtin(&format!("urllib.parse.{name}"))),
            Value::Raised(error) => {
                if let Some(class) = error.class() {
                    return Ok(class);
                }
                let name = error.kind.clone();
                Ok(self.builtin(&name))
            }
            Value::Calendar { value, .. } => {
                let class = format!("datetime.{}", value.class());
                Ok(self.builtin(&class))
            }
            _ => {
                let name = self.builtin_type_name(obj).ok_or_else(|| {
                    Error::new(
                        "NotImplementedError",
                        "type() is unsupported for this native object",
                    )
                })?;
                if let Some(&class) = self.builtins.get(name) {
                    return Ok(class);
                }
                let name = name.to_owned();
                Ok(self.builtin(&name))
            }
        }
    }

    pub(super) fn instance_of(&self, value: Id, class: Id) -> bool {
        if self.classes.get("typing.Union") == Some(&class) {
            return matches!(self.heap.get(value), Value::Union(_));
        }
        if self.classes.get("collections.abc.Callable") == Some(&class) {
            return self.callable(value);
        }
        if let Value::Tuple(classes) = self.heap.get(class) {
            return classes.iter().any(|&c| self.instance_of(value, c));
        }
        if let Some(name) = self.heap.get(value).native_io_class_name() {
            if let Some(&actual) = self.builtins.get(name)
                && self.subclass(actual, class)
            {
                return true;
            }
        } else if let Some(actual) = self
            .builtin_type_name(value)
            .and_then(|name| self.builtins.get(name))
            && super::mapping_abc::registered_subclass(self, *actual, class)
        {
            return true;
        }
        if let Some(actual) = self.heap.get(value).instance_class()
            && self.subclass(actual, class)
        {
            return true;
        }
        let name = match self.heap.get(class) {
            Value::Class { name, .. } | Value::Native(name) => name.as_str(),
            _ => return false,
        };
        if let Value::Weak(wr) = self.heap.get(value) {
            if wr.kind.class() == name {
                return true;
            }
            if wr.kind != weakref::Kind::Reference
                && let Some(target) = wr.target
            {
                return self.instance_of(target, class);
            }
        }
        match (name, self.heap.get(value)) {
            (HashState::CLASS, Value::Hash(state)) => {
                !HashState::is_provider_class(state.lock().class_name())
            }
            (class, Value::Hash(state)) if HashState::is_provider_class(class) => {
                state.lock().class_name() == class
            }
            (HashState::XOF_CLASS, Value::Hash(state)) => state.lock().class_name() == name,
            (_, Value::Raised(error)) => {
                matches!(self.heap.get(class), Value::Native(name) if name == "object")
                    || self.raised_error_matches(error, class)
            }
            (name, Value::Selector(handle)) if name == handle.borrow().class_name() => true,
            ("array.array", Value::Array(_))
            | ("bytearray", Value::ByteArray(_))
            | ("memoryview", Value::MemoryView { .. })
            | ("object", _)
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("float", Value::Float(_))
            | ("complex", Value::Complex(_))
            | ("str", Value::Str(_) | Value::SurrogateStr(_))
            | ("bytes", Value::Bytes(_))
            | ("struct.Struct", Value::Struct { .. })
            | ("hmac.HMAC", Value::Hmac(_))
            | ("socket.socket", Value::Socket(_))
            | ("_io.TextIOWrapper", Value::TextFile(_))
            | ("list", Value::List(_))
            | ("dict", Value::Dict(_) | Value::DefaultDict { .. })
            | ("collections.defaultdict", Value::DefaultDict { .. })
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | ("slice", Value::Slice(_))
            | ("range", Value::Range(_))
            | ("map", Value::Map { .. })
            | ("filter", Value::Filter { .. })
            | ("_contextvars.ContextVar", Value::ContextVar { .. })
            | ("_contextvars.Context", Value::Context(_))
            | ("functools.partial", Value::Partial { .. })
            | ("functools._lru_cache_wrapper", Value::Cached { .. })
            | ("_contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | (super::namedtuple::GETTER, Value::TupleGetter { .. })
            | (
                "member_descriptor",
                Value::TupleGetterDoc | Value::ReadOnlyMember(_, _) | Value::SlotMember { .. },
            )
            | ("operator.itemgetter", Value::ItemGetter(_))
            | ("itertools.count", Value::Count { .. })
            | (super::deque::CLASS, Value::Deque(_))
            | (super::deque::ITERATOR, Value::DequeIterator { reverse: false, .. })
            | (super::deque::REVERSE_ITERATOR, Value::DequeIterator { reverse: true, .. })
            | ("itertools.repeat", Value::Repeat(_))
            | (super::thread_local::CLASS, Value::ThreadLocal(_))
            | (stdlib::os::scandir::ENTRY, Value::DirEntry(_))
            | (stdlib::os::scandir::ITER, Value::Scandir(_))
            | (super::thread_rlock::CLASS, Value::HandlerLock(_))
            | (super::thread_lock::CLASS, Value::PrimitiveLock(_))
            | (value::TEMPLATE_TYPE, Value::Template { .. })
            | (value::TEMPLATE_ITER_TYPE, Value::TemplateIter { .. })
            | (value::INTERPOLATION_TYPE, Value::Interpolation { .. })
            | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => true,
            (
                qualified,
                Value::NamedTuple {
                    class: Some(name), ..
                },
            ) if matches!(
                *name,
                "SplitResult" | "SplitResultBytes" | "ParseResult" | "ParseResultBytes"
            ) =>
            {
                qualified.strip_prefix("urllib.parse.") == Some(*name)
            }
            (
                stdlib::os::stat::CLASS,
                Value::NamedTuple {
                    class: Some(stdlib::os::stat::CLASS),
                    ..
                },
            ) => true,
            ("typing._GenericAlias", Value::GenericAlias(alias))
                if alias.kind == super::typing::AliasKind::Unpack =>
            {
                true
            }
            ("tuple", Value::Instance { class, .. }) => self.namedtuple_fields(*class).is_some(),
            (
                name,
                Value::NamedTuple {
                    class:
                        Some(
                            class @ ("time.struct_time"
                            | stdlib::fixed_structseq::UNRAISABLE_ARGS
                            | stdlib::fixed_structseq::HASH_INFO
                            | stdlib::fixed_structseq::TERMINAL_SIZE
                            | stdlib::flags::TYPE),
                        ),
                    ..
                },
            ) if name == *class => true,
            (class, Value::Calendar { value, .. }) => {
                class.strip_prefix("datetime.").is_some_and(|c| {
                    c == value.class()
                        || (c == "date" && value.class() == "datetime")
                        || (c == "tzinfo" && value.class() == "timezone")
                })
            }
            _ => {
                matches!(self.heap.get(class), Value::Native(_))
                    && self.builtin_type_name(value) == Some(name)
            }
        }
    }
    pub(super) fn error_matches(&self, kind: &str, class: Id) -> bool {
        self.error_kind_matches(kind, class, self.classes.get(kind).copied())
    }

    pub(super) fn raised_error_matches(&self, error: &Error, class: Id) -> bool {
        match self.heap.get(class) {
            Value::Tuple(classes) => classes.iter().any(|&c| self.raised_error_matches(error, c)),
            _ if let Some(actual) = error.class() => self.subclass(actual, class),
            Value::Native(_) => {
                let kind = error.kind.as_ref();
                let builtin = self.builtins.get(kind).copied();
                if self.classes.get(kind).copied() == builtin {
                    self.error_matches(kind, class)
                } else {
                    // A user class with the same name cannot change a native error's ancestry.
                    self.error_kind_matches(kind, class, builtin)
                }
            }
            _ => false,
        }
    }

    fn error_kind_matches(&self, kind: &str, class: Id, registered: Option<Id>) -> bool {
        if let Value::Native(parent) = self.heap.get(class)
            && let Some(result) = builtins::exception_subclass(kind, parent)
        {
            return result;
        }
        match self.heap.get(class) {
            Value::Tuple(classes) => classes
                .iter()
                .any(|&c| self.error_kind_matches(kind, c, registered)),
            Value::Class { name, .. } | Value::Native(name) => {
                kind == name
                    || name == "BaseException"
                    || (name == "Exception" && kind != "SystemExit")
                    || (name == "ValueError"
                        && matches!(kind, "JSONDecodeError" | "binascii.Error"))
                    || (name == "OSError" && kind == "socket.gaierror")
                    || registered.is_some_and(|actual| self.subclass(actual, class))
            }
            _ => false,
        }
    }
    pub(super) fn missing_attribute(&self, obj: Id, name: &str) -> Error {
        let message = if matches!(self.heap.get(obj), Value::Class { .. }) {
            let class = self.class_name(obj);
            let class = &class[..class.floor_char_boundary(100)];
            format!("type object '{class}' has no attribute '{name}'")
        } else {
            format!("'{}' object has no attribute '{name}'", self.type_name(obj))
        };
        Error::new("AttributeError", message)
    }

    /// Match an attribute lookup failure using the ordinary exception hierarchy.
    pub(super) fn is_attribute_error(&self, error: &Error) -> bool {
        self.is_builtin_error(error, "AttributeError")
    }

    /// Includes runtime-loaded subclasses of the named builtin exception.
    pub(super) fn is_builtin_error(&self, error: &Error, kind: &str) -> bool {
        (error.class().is_none()
            && builtins::exception_subclass(&error.kind, kind).unwrap_or(error.kind == kind))
            || self
                .builtins
                .get(kind)
                .is_some_and(|&base| self.raised_error_matches(error, base))
    }

    /// Materialize native exception arguments before their backing fields can change.
    fn exception_argument_tuple(&mut self, obj: Id, error: Error) -> Id {
        if let Some(args) = error.args {
            return args;
        }
        let fields = if error.message.is_empty() {
            Vec::new()
        } else {
            vec![
                self.import_error_message(obj, None)
                    .unwrap_or_else(|| self.text(error.message)),
            ]
        };
        let tuple = self.alloc(Value::Tuple(fields));
        let Value::Raised(error) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        error.args = Some(tuple);
        tuple
    }

    /// Preserve ImportError's original message when its arguments are replaced.
    pub(super) fn import_error_message(&mut self, obj: Id, replacement: Option<Id>) -> Option<Id> {
        let Value::Raised(error) = self.heap.get(obj) else {
            return None;
        };
        if !self.is_builtin_error(error, "ImportError") {
            return None;
        }
        let message = if let Some(message) = replacement {
            if error.args.is_none() {
                self.exception_argument_tuple(obj, error.clone());
            }
            message
        } else {
            if let Some(message) = error.trace.as_ref().and_then(|trace| trace.import_message) {
                return Some(message);
            }
            match self.exception_arguments(error) {
                Some([one]) => *one,
                Some(_) => NONE,
                None => self.text(error.message.clone()),
            }
        };
        let Value::Raised(error) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        error.tracing().import_message = Some(message);
        Some(message)
    }

    fn attribute_hook(
        &mut self,
        hook: Id,
        obj: Id,
        class: Id,
        name: &str,
        original: Option<Id>,
    ) -> Result<Id> {
        let callable = self.bind_attribute(hook, obj, class)?;
        let key = original.unwrap_or_else(|| self.text(name));
        self.call(callable, vec![key], Vec::new())
    }

    pub(super) fn lookup_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        self.lookup_named_attr(obj, name, None)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn lookup_named_attr(
        &mut self,
        obj: Id,
        name: &str,
        original: Option<Id>,
    ) -> Result<Id> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self
                .local_attribute(obj, super::thread_local::Name::from_lookup(name, original));
        }
        let class = match *self.heap.get(obj) {
            Value::Instance { class, .. }
            | Value::Class {
                metaclass: Some(class),
                ..
            }
            | Value::Module {
                constructor: Some(class),
                ..
            } => class,
            Value::Raised(ref error) if let Some(class) = error.class() => class,
            Value::Module { scope, .. } => {
                return self.module_get_attribute(obj, scope, name, original);
            }
            _ => return self.default_get_attr(obj, name),
        };
        let attribute = if let Some(hook) = self.class_attr(class, "__getattribute__") {
            self.attribute_hook(hook, obj, class, name, original)
        } else if let Value::Module { scope, .. } = *self.heap.get(obj) {
            self.module_get_attribute(obj, scope, name, original)
        } else {
            self.default_get_attr(obj, name)
        };
        match attribute {
            Err(error) if self.is_attribute_error(&error) => {
                let Some(hook) = self.class_attr(class, "__getattr__") else {
                    return Err(error);
                };
                self.attribute_hook(hook, obj, class, name, original)
            }
            result => result,
        }
    }

    /// Bind a context manager's special method on its type before entering the suite.
    pub(super) fn context_method(&mut self, obj: Id, name: &str) -> Result<Id> {
        if let Some(method) = self.bound_slot(obj, name)? {
            return Ok(method);
        }
        let base = self.scalar_value(obj);
        if builtins::method_name(self.heap.get(base), name).is_some() {
            return self.builtin_attribute(obj, name);
        }
        let mut kind = self
            .builtin_type_name(obj)
            .unwrap_or_else(|| self.type_name(obj));
        let mut module = None;
        if let Some(class) = self.heap.get(obj).instance_class()
            && let Value::Class { scope, .. } = self.heap.get(class)
        {
            if let Some(qualified) = self.namespace_binding(self.scope_data(*scope), "__qualname__") {
                kind = self.str_value(qualified).unwrap_or(kind);
            }
            module = match self.namespace_binding(self.scope_data(*scope), "__module__") {
                Some(id) => self.str_value(id).ok(),
                None => Some(self.scope_data(*scope).module.as_str()),
            }
            .filter(|name| !matches!(*name, "builtins" | "__main__"));
        }
        // CPython checks descriptor slots without binding the other protocol's methods.
        let asynchronous = matches!(name, "__aenter__" | "__aexit__");
        let other_methods = if asynchronous {
            ["__enter__", "__exit__"]
        } else {
            ["__aenter__", "__aexit__"]
        };
        let supports_other = other_methods.iter().all(|name| {
            self.slot(obj, name).is_some_and(|slot| {
                matches!(
                    self.builtin_type_name(slot),
                    Some(
                        "classmethod"
                            | "method"
                            | "method_descriptor"
                            | "wrapper_descriptor"
                            | "super"
                    )
                ) || self.native_data_descriptor(slot)
                    || self.slot(slot, "__get__").is_some()
                    || builtins::method_name(self.heap.get(slot), "__get__").is_some()
            })
        });
        let suggestion = match (supports_other, asynchronous) {
            (true, true) => {
                " but it supports the context manager protocol. Did you mean to use 'with'?"
            }
            (true, false) => {
                " but it supports the asynchronous context manager protocol. Did you mean to use 'async with'?"
            }
            _ => "",
        };
        let parts = [
            "'",
            module.unwrap_or(""),
            if module.is_some() { "." } else { "" },
            kind,
            "' object does not support the ",
            if asynchronous { "asynchronous " } else { "" },
            "context manager protocol (missed ",
            name,
            " method)",
            suggestion,
        ];
        let mut message = String::new();
        message
            .try_reserve_exact(parts.iter().map(|s| s.len()).sum())
            .map_err(|_| Error::new("MemoryError", ""))?;
        for part in parts {
            message.push_str(part);
        }
        Err(Error::new_static("TypeError", message))
    }

    /// Resolve an immediately called attribute before its arguments are evaluated.
    pub(super) fn lookup_method(&mut self, obj: Id, name: &str) -> Result<Id> {
        if name == "writelines" && matches!(self.heap.get(obj), Value::File(_)) {
            // A direct call reports the defining IOBase descriptor in argument errors.
            let callable = self.builtin("_IOBase.writelines");
            return Ok(self.alloc(Value::Method {
                callable,
                receiver: obj,
                explicit: false,
            }));
        }
        if let Value::Instance { class, .. } = *self.heap.get(obj)
            && self.class_attr(class, "__getattr__").is_none()
            && self.class_attr(class, "__getattribute__").is_none_or(|hook| matches!(self.heap.get(hook), Value::Native(symbol) if symbol == "object.__getattribute__"))
        {
            self.default_attr_for_call(obj, name, true)
        } else {
            self.lookup_attr(obj, name)
        }
    }

    /// Base lookup bypasses Python's __getattribute__ and __getattr__ hooks.
    pub(super) fn default_get_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        self.default_attr_for_call(obj, name, false)
    }

    /// Keep non-data wrapper receiver checks at call time for direct method calls.
    fn default_attr_for_call(&mut self, obj: Id, name: &str, for_call: bool) -> Result<Id> {
        if self.heap.get(obj).native_io_class_name() == Some("_io.BufferedReader") {
            let class = self.exact_type(obj)?;
            if let Some(member) = self.class_attr(class, name)
                && matches!(
                    self.heap.get(member),
                    Value::IoMethod { .. } | Value::GetSetDescriptor { .. }
                )
            {
                return self.bind_attribute(member, obj, class);
            }
        }
        let metaclass_member = match *self.heap.get(obj) {
            Value::Class {
                metaclass: Some(meta),
                ..
            } => self.class_attr(meta, name).map(|member| (meta, member)),
            _ => None,
        };
        if let Some((meta, member)) = metaclass_member
            && self.readable_data_descriptor(member)
        {
            return self.bind_attribute(member, obj, meta);
        }
        if let Value::Native(kind) = self.heap.get(obj)
            && stdlib::datetime::members(kind).is_some()
            && let Some(value) = self.class_attr(obj, name)
        {
            return Ok(value);
        }
        if let Value::Union(union) = *self.heap.get(obj) {
            match name {
                "__args__" => return Ok(union.args),
                "__parameters__" => return Ok(union.parameters),
                "__class__" | "__origin__" => return Ok(super::union::class(self)),
                "__module__" => return Ok(self.text("typing")),
                "__name__" | "__qualname__" => return Ok(self.text("Union")),
                _ => {}
            }
        }
        if let Value::Instance {
            class,
            fields,
            namespace,
            ..
        } = self.heap.get(obj)
        {
            let (class, inline, namespace) = (*class, fields.get(name).copied(), *namespace);
            if matches!(name, "name" | "value")
                && let Some((_, member_name, value)) = self.enum_member(obj)
            {
                return Ok(if name == "name" { member_name } else { value });
            }
            let layout = self.instance_layout(class, name);
            if layout.active_slot {
                return inline.ok_or_else(|| self.missing_attribute(obj, name));
            }
            let member = self.class_attr(class, name);
            if name == "__dict__" && member.is_none() && layout.dictionary {
                return Ok(self.instance_dictionary(obj, class));
            }
            if let Some(member) = member
                && self.native_data_descriptor(member)
            {
                return self.bind_attribute(member, obj, class);
            }
            let inline =
                inline.filter(|_| !layout.declared_slot && !self.has_namedtuple_field(class, name));
            let field = if inline.is_some()
                || namespace.is_none()
                || member.is_some_and(|member| self.data_descriptor(member))
            {
                inline
            } else {
                self.dictionary_field(namespace, name)?
            };
            if for_call
                && field.is_none()
                && let Some(member) = member
                && let Value::Native(symbol) = self.heap.get(member)
                && BuiltinDescriptor::lookup(symbol).is_some()
            {
                return Ok(self.alloc(Value::Method {
                    callable: member,
                    receiver: obj,
                    explicit: false,
                }));
            }
            return self.instance_attribute(obj, class, field, name);
        }
        if let Value::Raised(error) = self.heap.get(obj) {
            let dictionary = error.trace.as_ref().and_then(|trace| trace.dictionary);
            if let Some(class) = error.class()
                && let Some(member) = self.class_attr(class, name)
            {
                let field = if self.readable_data_descriptor(member) {
                    None
                } else {
                    self.dictionary_field(dictionary, name)?
                };
                return self.instance_attribute(obj, class, field, name);
            }
            if let Some(fields) = error
                .trace
                .as_ref()
                .and_then(|trace| trace.source_syntax.as_ref())
                && let Some(slot) = super::source_code::syntax_member(name)
            {
                return Ok(fields[slot].get());
            }
            if name == "__dict__" {
                return Ok(self.exception_dictionary(obj));
            }
        }
        if name == "name"
            && let Some(error) = self.exception_storage(obj, "NameError")
        {
            return Ok(error
                .trace
                .as_ref()
                .and_then(|t| t.name_error_name)
                .unwrap_or(NONE));
        }
        if name == "value"
            && let Some(error) = self.exception_storage(obj, "StopIteration")
        {
            return Ok(error.stop_value());
        }
        if matches!(name, "__new__" | "__init__" | "with_traceback")
            && matches!(self.heap.get(obj), Value::Raised(_))
        {
            let class = self.exact_type(obj)?;
            return self.instance_attribute(obj, class, None, name);
        }
        if name == "__class__" {
            return self.exact_type(obj);
        }
        if let Value::Deque(ring) = self.heap.get(obj) {
            if name == "maxlen" {
                return Ok(ring.maxlen.map_or(NONE, |n| self.int(n as i64)));
            }
            return self.builtin_attribute(obj, name);
        }
        if name == "msg"
            && let Some(message) = self.import_error_message(obj, None)
        {
            return Ok(message);
        }
        if let Some(value) = super::typing::attribute(self, obj, name) {
            return value;
        }
        if let Some(value) = super::array::attribute(self, obj, name) {
            return Ok(value);
        }
        if let Some(value) = self.os_attribute(obj, name)? {
            return Ok(value);
        }
        match *self.heap.get(obj) {
            Value::Complex(z) if matches!(name, "real" | "imag") => {
                return Ok(self.alloc(Value::Float(if name == "real" { z.real } else { z.imag })));
            }
            Value::Raised(ref error)
                if matches!(
                    name,
                    "__traceback__" | "__context__" | "__cause__" | "__suppress_context__"
                ) =>
            {
                match name {
                    "__traceback__" => return self.exception_traceback(obj),
                    "__context__" => {
                        return Ok(error.context);
                    }
                    "__cause__" => {
                        return Ok(error.trace.as_ref().and_then(|t| t.cause).unwrap_or(NONE));
                    }
                    "__suppress_context__" => {
                        return Ok(if error.trace.as_ref().is_some_and(|t| t.cause.is_some()) {
                            TRUE
                        } else {
                            FALSE
                        });
                    }
                    _ => unreachable!(),
                }
            }
            Value::Traceback {
                frame,
                next,
                lineno,
                ..
            } => match name {
                "tb_frame" => return Ok(frame),
                "tb_next" => return Ok(next),
                "tb_lineno" => return Ok(self.int(lineno)),
                _ => {}
            },
            Value::Native(ref class) if class == "frame" && name == "clear" => {
                return Ok(self.builtin("frame.clear"));
            }
            Value::PythonFrame { .. } if name == "clear" => {
                let method = self.builtin("frame.clear");
                let owner = self.builtin("frame");
                return self.bind_attribute(method, obj, owner);
            }
            Value::PythonFrame { back, .. } if name == "f_back" => {
                return match back {
                    Some(caller) => Ok(caller),
                    None => self.generator_frame_back(obj),
                };
            }
            Value::PythonFrame { globals, .. } if name == "f_globals" => return Ok(globals),
            Value::PythonFrame { code, .. } if name == "f_code" => {
                if !self.audit_hooks.is_empty() {
                    let attribute = self.text(name);
                    super::stdlib::audit::event(self, "object.__getattr__", &[obj, attribute])?;
                }
                return Ok(code);
            }
            Value::PythonFrame { lineno, .. } if name == "f_lineno" => {
                return Ok(self.int(i64::from(lineno)));
            }
            Value::Native(ref class)
                if ((builtins::is_builtin_type(class) && !class.contains('.'))
                    || matches!(class.as_str(), "traceback" | "frame" | "mappingproxy"))
                    && matches!(name, "__name__" | "__qualname__" | "__module__") =>
            {
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(class.clone())),
                    "__module__" => return Ok(self.text("builtins")),
                    _ => {}
                }
            }
            Value::KeyWrapper { object, .. } => match name {
                "obj" => return Ok(object.unwrap_or(NONE)),
                "__hash__" | "__doc__" => return Ok(NONE),
                "__module__" => return Ok(self.text("functools")),
                "__text_signature__" => return Ok(self.text("(obj)")),
                "__new__" => return Ok(self.builtin("object.__new__")),
                _ => {
                    let symbol = format!("{KEY_WRAPPER}.{name}");
                    if let Some(slot) = BuiltinDescriptor::lookup(&symbol) {
                        let method = self.builtin(&symbol);
                        return slot.bind(self, method, obj);
                    }
                }
            },
            Value::Cached { dictionary, .. } => {
                if name == "__dict__" {
                    return Ok(dictionary);
                }
                if let Some(value) = self.dictionary_field(Some(dictionary), name)? {
                    return Ok(value);
                }
                if matches!(name, "cache_clear" | "cache_info") {
                    let callable = self.builtin(&format!("functools._lru_cache_wrapper.{name}"));
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: false,
                    }));
                }
            }
            Value::ClassMethod(callable) if name == "__isabstractmethod__" => {
                return Ok(if self.is_abstract_method(callable)? {
                    TRUE
                } else {
                    FALSE
                });
            }
            Value::ClassMethod(callable) if matches!(name, "__func__" | "__wrapped__") => {
                return Ok(callable);
            }
            Value::StaticMethod {
                callable,
                dictionary,
            } => {
                if let Some(value) =
                    super::static_method::attribute(self, callable, dictionary, name)?
                {
                    return Ok(value);
                }
            }
            Value::None if name == "__doc__" => {
                return Ok(self.text("The type of the None singleton."));
            }
            Value::MemoryView { owner, .. } if matches!(name, "obj" | "readonly" | "nbytes") => {
                let owner = Self::memoryview_owner(owner)?;
                return Ok(match name {
                    "obj" => owner,
                    "readonly" => TRUE,
                    _ => self.int(self.bytes_like(owner)?.len() as i64),
                });
            }
            Value::Slice(parts) if matches!(name, "start" | "stop" | "step") => {
                return Ok(parts[match name {
                    "start" => 0,
                    "stop" => 1,
                    _ => 2,
                }]);
            }
            Value::Range(ref range) if matches!(name, "start" | "stop" | "step") => {
                let bound = match name {
                    "start" => range.start.clone(),
                    "stop" => range.stop.clone(),
                    _ => range.step.clone(),
                };
                return Ok(self.alloc(Value::Int(bound)));
            }
            Value::Native(ref class) if class == "module" => match name {
                "__doc__" => return Ok(self.text(module_type::DOC)),
                "__init__" | "__new__" | "__dir__" | "__repr__" | "__getattribute__" => {
                    return Ok(self.builtin(&format!("module.{name}")));
                }
                "__name__" | "__qualname__" => return Ok(self.text("module")),
                "__module__" => return Ok(self.text("builtins")),
                _ => {}
            },
            Value::SlotMember { owner, name: field } => match name {
                "__name__" => return Ok(field),
                "__objclass__" => return Ok(owner),
                "__doc__" => return Ok(NONE),
                "__qualname__" => {
                    let qualname = self.lookup_attr(owner, "__qualname__")?;
                    return Ok(self.text(format!(
                        "{}.{}",
                        self.str_value(qualname)?,
                        self.str_value(field)?
                    )));
                }
                _ => {}
            },
            Value::ReadOnlyMember(owner, field) => match name {
                "__name__" => return Ok(self.text(field)),
                "__qualname__" => {
                    return Ok(self.text(format!("{}.{field}", owner.rsplit('.').next().unwrap())));
                }
                "__objclass__" => return Ok(self.builtin(owner)),
                _ => {}
            },
            Value::TupleGetterDoc => match name {
                "__name__" => return Ok(self.text("__doc__")),
                "__qualname__" => return Ok(self.text("_tuplegetter.__doc__")),
                "__doc__" => return Ok(NONE),
                "__objclass__" => return Ok(self.builtin(super::namedtuple::GETTER)),
                _ => {}
            },
            Value::Native(ref class) if class == "member_descriptor" && name == "__module__" => {
                return Ok(self.text("builtins"));
            }
            Value::TupleGetter { doc, .. } if name == "__doc__" => return Ok(doc.unwrap_or(NONE)),
            Value::WeakrefDescriptor(owner) => match name {
                "__objclass__" => return Ok(owner),
                "__name__" => return Ok(self.text("__weakref__")),
                "__doc__" => return Ok(self.text("list of weak references to the object")),
                "__qualname__" => {
                    let name = self.lookup_attr(owner, "__qualname__")?;
                    return Ok(self.text(format!("{}.__weakref__", self.str_value(name)?)));
                }
                "__get__" | "__set__" | "__delete__" => {
                    let callable = self.builtin(&format!("weakref.descriptor.{name}"));
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: true,
                    }));
                }
                _ => {}
            },
            Value::Property(property) => {
                if let Some(attribute) = property.attribute(self, name)? {
                    return Ok(attribute);
                }
            }
            Value::Native(ref symbol)
                if builtins::is_builtin(symbol)
                    && !builtins::is_builtin_type(symbol)
                    && matches!(name, "__name__" | "__qualname__" | "__module__") =>
            {
                if name == "__module__" {
                    return Ok(self.text(if symbol == "open" { "_io" } else { "builtins" }));
                }
                return Ok(self.text(symbol.clone()));
            }
            Value::Native(ref symbol) if let Some((module, function)) = module_function(symbol) => {
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(function.to_owned())),
                    "__module__" => return Ok(self.text(module.to_owned())),
                    "__doc__" if symbol == "_signal.sigpending" => {
                        return Ok(self.text("Examine pending signals.\n\nReturns a set of signal numbers that are pending for delivery to\nthe calling thread."));
                    }
                    "__doc__" if symbol == "_thread.allocate" => {
                        return Ok(self.text("An obsolete synonym of allocate_lock()."));
                    }
                    "__doc__" if symbol == "_thread.allocate_lock" => {
                        return Ok(self.text("Create a new lock object. See help(type(threading.Lock())) for\ninformation about locks."));
                    }
                    "__doc__" if symbol == "_io.text_encoding" => {
                        return Ok(self.text(concat!(
                            "A helper function to choose the text encoding.\n",
                            "\n",
                            "When encoding is not None, this function returns it.\n",
                            "Otherwise, this function returns the default text encoding\n",
                            "(i.e. \"locale\" or \"utf-8\" depends on UTF-8 mode).\n",
                            "\n",
                            "This function emits an EncodingWarning if encoding is None and\n",
                            "sys.flags.warn_default_encoding is true.\n",
                            "\n",
                            "This can be used in APIs with an encoding=None parameter.\n",
                            "However, please consider using encoding=\"utf-8\" for new APIs.",
                        )));
                    }
                    "__text_signature__" if symbol == "_io.text_encoding" => {
                        return Ok(self.text("($module, encoding, stacklevel=2, /)"));
                    }
                    "__doc__" if symbol == "posix.urandom" => {
                        return Ok(self.text("Return a bytes object containing random bytes suitable for cryptographic use."));
                    }
                    "__text_signature__" if symbol == "posix.urandom" => {
                        return Ok(self.text("($module, size, /)"));
                    }
                    "__doc__" if symbol == "functools.cmp_to_key" => {
                        return Ok(self.text("Convert a cmp= function into a key= function.\n\n  mycmp\n    Function that compares two objects."));
                    }
                    "__text_signature__" if symbol == "functools.cmp_to_key" => {
                        return Ok(self.text("($module, /, mycmp)"));
                    }
                    "__doc__" if symbol == "functools.reduce" => {
                        return Ok(self.text(concat!(
                            "Apply a function of two arguments cumulatively to the items of an iterable, from left to right.\n\n",
                            "This effectively reduces the iterable to a single value.  If initial is present,\n",
                            "it is placed before the items of the iterable in the calculation, and serves as\n",
                            "a default when the iterable is empty.\n\n",
                            "For example, reduce(lambda x, y: x+y, [1, 2, 3, 4, 5])\n",
                            "calculates ((((1 + 2) + 3) + 4) + 5)."
                        )));
                    }
                    "__text_signature__" if symbol == "functools.reduce" => {
                        return Ok(self
                            .text("($module, function, iterable, /, initial=<unrepresentable>)"));
                    }
                    "__doc__"
                        if let Some(function) = super::lowlevel_threads::function_name(symbol) =>
                    {
                        return Ok(self.text(super::lowlevel_threads::doc(function)));
                    }
                    "__text_signature__" if symbol == "_thread.start_new_thread" => {
                        return Ok(self.text("($module, function, args, kwargs={}, /)"));
                    }
                    "__text_signature__" if symbol == "fcntl.fcntl" => {
                        return Ok(self.text("($module, fd, cmd, arg=0, /)"));
                    }
                    "__doc__" if symbol == "resource.getrlimit" => return Ok(NONE),
                    "__text_signature__" if symbol == "resource.getrlimit" => {
                        return Ok(self.text("($module, resource, /)"));
                    }
                    "__doc__" if module == "gc" => {
                        let doc = match function {
                            "enable" => "Enable automatic garbage collection.",
                            "disable" => "Disable automatic garbage collection.",
                            "isenabled" => "Returns true if automatic garbage collection is enabled.",
                            _ => unreachable!("registered GC control"),
                        };
                        return Ok(self.text(doc));
                    }
                    "__doc__" if symbol == "posix.getpid" => {
                        return Ok(self.text("Return the current process id."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "posix.getpid"
                                | "gc.enable"
                                | "gc.disable"
                                | "gc.isenabled"
                                | "_signal.sigpending"
                                | "_thread.allocate"
                                | "_thread.allocate_lock"
                                | "_thread._count"
                        ) =>
                    {
                        return Ok(self.text("($module, /)"));
                    }
                    "__self__" => {
                        let module = module.to_owned();
                        return self.module(&module);
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if super::generator_protocol::coroutine_method(class, name) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if class == "generator" && super::generator_protocol::generator_method(name) =>
            {
                return Ok(self.builtin(&format!("generator.{name}")));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "classmethod" | "staticmethod")
                    && name == "__get__" =>
            {
                return Ok(self.builtin(&format!("{class}.__get__")));
            }
            Value::Native(ref class)
                if class == "property" && super::property::METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("property.{name}")));
            }
            Value::Code(ref code) => match name {
                "co_flags" => {
                    let flags = code.flags.ok_or_else(|| {
                        Error::new(
                            "NotImplementedError",
                            "co_flags unavailable for this code object",
                        )
                    })?;
                    return Ok(self.int(i64::from(flags)));
                }
                "co_name" => return Ok(code.name),
                "co_qualname" => return Ok(code.qualname),
                "co_filename" => return Ok(code.filename),
                "co_firstlineno" => return Ok(self.int(i64::from(code.first_line))),
                "co_freevars" => {
                    if !code.closure_supported && !code.module_definition {
                        return Err(Error::new(
                            "NotImplementedError",
                            "code lacks closure metadata",
                        ));
                    }
                    let names = code
                        .freevars
                        .clone()
                        .into_iter()
                        .map(|name| self.text(name))
                        .collect();
                    return Ok(self.alloc(Value::Tuple(names)));
                }
                _ => {}
            },
            Value::Cell(contents) if name == "cell_contents" => {
                return contents.ok_or_else(|| Error::new("ValueError", "Cell is empty"));
            }
            Value::Generator { code, .. } if name == "gi_code" => return Ok(code),
            Value::Coroutine(frame) if name == "cr_code" => {
                let Value::Generator { code, .. } = *self.heap.get(frame) else {
                    unreachable!("coroutine frame")
                };
                return Ok(code);
            }
            Value::Function { code, .. } if name == "__code__" => return Ok(code),
            Value::Function {
                scope,
                fields,
                closure,
                code,
                ..
            } => {
                if name == "__closure__" {
                    let code = self.code_object(code);
                    if !code.closure_supported && !code.module_definition {
                        return Err(Error::new(
                            "NotImplementedError",
                            "code lacks closure metadata",
                        ));
                    }
                    return Ok(closure);
                }
                if name == "__globals__" {
                    return self.module_globals(scope);
                }
                if name == "__builtins__" {
                    return match self.scope_data(self.root_scope(scope)).exec_globals {
                        Some(globals) => Ok(globals.builtins),
                        None => self.default_source_builtins(),
                    };
                }
                if let Some(value) = self.function_attr(obj, fields, name)? {
                    return Ok(value);
                }
                if name == "__call__" {
                    let class = self.builtin("function");
                    let descriptor = self.builtin("function.__call__");
                    return self.bind_attribute(descriptor, obj, class);
                }
            }
            Value::Method {
                callable, receiver, ..
            } if let Value::Native(ref symbol) = *self.heap.get(callable)
                && let Some(slot) = BuiltinDescriptor::lookup(symbol) =>
            {
                if name == "__self__" {
                    return Ok(receiver);
                }
                if let Some(value) = slot.attribute(self, name, Some(receiver)) {
                    return Ok(value);
                }
            }
            Value::Native(ref symbol) if let Some(slot) = BuiltinDescriptor::lookup(symbol) => {
                if let Some(value) = slot.attribute(self, name, None) {
                    return Ok(value);
                }
                if name == "__get__" {
                    let callable = self.builtin(if slot.method {
                        "method_descriptor.__get__"
                    } else {
                        "wrapper_descriptor.__get__"
                    });
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: false,
                    }));
                }
            }
            Value::Method {
                callable,
                receiver,
                explicit,
            } => match name {
                "__self__" => return Ok(receiver),
                _ if matches!(
                    self.heap.get(callable),
                    Value::IoMethod { name: "open", .. }
                ) =>
                {
                    let original = self.builtin("open");
                    return self.lookup_attr(original, name);
                }
                "__func__"
                    if explicit || matches!(self.heap.get(callable), Value::Function { .. }) =>
                {
                    return Ok(callable);
                }
                _ if explicit || matches!(self.heap.get(callable), Value::Function { .. }) => {
                    return self.lookup_attr(callable, name);
                }
                _ => {
                    if let Value::Native(method) = self.heap.get(callable) {
                        let module_bound = matches!(self.heap.get(receiver), Value::Module { .. });
                        let value = match name {
                            "__qualname__" if module_bound => method.rsplit('.').next(),
                            "__module__" if module_bound => {
                                method.rsplit_once('.').map(|(module, _)| module)
                            }
                            "__text_signature__" if method == "sys._getframe" => {
                                Some("($module, depth=0, /)")
                            }
                            "__text_signature__" if method == "sys._getframemodulename" => {
                                Some("($module, /, depth=0)")
                            }
                            "__text_signature__" if method == "sys.audit" => {
                                Some("($module, event, /, *args)")
                            }
                            "__text_signature__" if method == "sys.addaudithook" => {
                                Some("($module, /, hook)")
                            }
                            "__doc__" if method == "sys.audit" => {
                                Some("Passes the event to any audit hooks that are attached.")
                            }
                            "__doc__" if method == "sys.addaudithook" => {
                                Some("Adds a new audit hook callback.")
                            }
                            "__text_signature__" if method.starts_with("sys._jit.") => {
                                Some("($module, /)")
                            }
                            "__doc__" if method.starts_with("sys._jit.") => match method.as_str() {
                                "sys._jit.is_available" => Some(
                                    "Return True if the current Python executable supports JIT compilation, and False otherwise.",
                                ),
                                "sys._jit.is_enabled" => Some(
                                    "Return True if JIT compilation is enabled for the current Python process (implies sys._jit.is_available()), and False otherwise.",
                                ),
                                "sys._jit.is_active" => Some(
                                    "Return True if the topmost Python frame is currently executing JIT code (implies sys._jit.is_enabled()), and False otherwise.",
                                ),
                                _ => None,
                            },
                            "__name__" => method.rsplit('.').next(),
                            "__qualname__" => Some(method.as_str()),
                            "__module__" => return Ok(NONE),
                            _ => None,
                        };
                        if let Some(value) = value {
                            return Ok(self.text(value.to_owned()));
                        }
                    }
                }
            },
            Value::RegexScanner { pattern, .. } if name == "pattern" => return Ok(pattern),
            Value::RegexMatch(ref m) if name == "re" => return Ok(m.pattern),
            Value::RegexMatch(ref m) if name == "string" => return Ok(m.subject),
            Value::RegexMatch(ref m) if name == "lastindex" => {
                if let Some(branch) = m.lexicon_branch(self) {
                    return Ok(self.int(branch as i64));
                }
            }
            Value::RegexPattern(ref pattern) if name == "flags" => {
                return Ok(self.int(pattern.flags));
            }
            Value::RegexPattern(ref pattern) if name == "groups" => {
                return Ok(self.int(pattern.group_count() as i64));
            }
            Value::RegexPattern(ref pattern) if name == "pattern" => {
                return Ok(pattern.source);
            }
            Value::Calendar { ref value, .. } => {
                let kind = value.class();
                let class = self.exact_type(obj)?;
                if let Some(member) = self.class_attr(class, name) {
                    return self.bind_attribute(member, obj, class);
                }
                return Err(Error::new(
                    "AttributeError",
                    format!("'datetime.{kind}' object has no attribute '{name}'"),
                ));
            }
            Value::Native(ref class) if class == stdlib::os::stat::CLASS => match name {
                "__reduce__" => return Ok(self.builtin("os.stat_result.__reduce__")),
                field if let Some(id) = stdlib::os::stat::member(self, field) => return Ok(id),
                "__module__" => return Ok(self.text("os")),
                "__name__" | "__qualname__" => return Ok(self.text("stat_result")),
                "__base__" => return Ok(self.builtin("tuple")),
                "__match_args__" => {
                    let names = stdlib::os::stat::FIELDS[..7]
                        .iter()
                        .map(|&n| self.text(n))
                        .collect();
                    return Ok(self.alloc(Value::Tuple(names)));
                }
                field if let Some(n) = stdlib::os::stat::count_attribute(field) => {
                    return Ok(self.int(n));
                }
                _ => {}
            },
            Value::Native(ref class)
                if class == "datetime.datetime"
                    && matches!(name, "now" | "fromtimestamp" | "combine") =>
            {
                return Ok(self.builtin(&format!("datetime.datetime.{name}")));
            }
            Value::GetSetDescriptor {
                owner, name: field, ..
            } if HashState::is_provider_class(owner)
                || owner == value::TEMPLATE_TYPE
                || stdlib::datetime::members(owner).is_some() =>
            {
                match name {
                    "__objclass__" => return Ok(self.builtin(owner)),
                    "__name__" => return Ok(self.text(field)),
                    "__qualname__" => {
                        return Ok(
                            self.text(format!("{}.{field}", owner.rsplit_once('.').unwrap().1))
                        );
                    }
                    "__get__" => {
                        let get = self.builtin("getset_descriptor.__get__");
                        return Ok(self.alloc(Value::Method {
                            callable: get,
                            receiver: obj,
                            explicit: false,
                        }));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class) if let Some(owner) = HashState::provider_class(class) => {
                let (module, short) = owner.rsplit_once('.').unwrap();
                match name {
                    "name" | "block_size" | "digest_size" => {
                        let key = format!("{owner}.{name}");
                        if let Some(&descriptor) = self.builtins.get(&key) {
                            return Ok(descriptor);
                        }
                        let field = match name {
                            "name" => "name",
                            "block_size" => "block_size",
                            _ => "digest_size",
                        };
                        let get = self.builtin(&format!("{owner}.get_{field}"));
                        let descriptor = self.alloc(Value::GetSetDescriptor {
                            get,
                            set: None,
                            owner,
                            name: field,
                        });
                        self.builtins.insert(key, descriptor);
                        return Ok(descriptor);
                    }
                    "__init__" => return Ok(self.builtin("object.__init__")),
                    "__new__" | "copy" | "update" | "digest" | "hexdigest" => {
                        return Ok(self.builtin(&format!("{owner}.{name}")));
                    }
                    "__module__" => return Ok(self.text(module)),
                    "__name__" | "__qualname__" => return Ok(self.text(short)),
                    "__base__" => return Ok(self.builtin("object")),
                    "__bases__" | "__mro__" => {
                        let base = self.builtin("object");
                        let objects = if name == "__mro__" {
                            vec![obj, base]
                        } else {
                            vec![base]
                        };
                        return Ok(self.alloc(Value::Tuple(objects)));
                    }
                    attr if let Some(value) = HashState::class_constant(owner, attr) => {
                        return Ok(self.int(value as i64));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "array.array"
                        | value::REGEX_PATTERN_TYPE
                        | value::REGEX_MATCH_TYPE
                        | "_contextvars.Context"
                        | "_contextvars.ContextVar"
                        | "_contextvars.Token"
                        | KEY_WRAPPER
                        | "functools._lru_cache_wrapper"
                        | HashState::CLASS
                        | HashState::XOF_CLASS
                        | super::namedtuple::GETTER
                        | super::deque::CLASS
                        | super::deque::ITERATOR
                        | super::deque::REVERSE_ITERATOR
                ) =>
            {
                let class = class.as_str().to_owned();
                let (module, short) = class.rsplit_once('.').unwrap();
                if class == "array.array" && matches!(name, "append" | "tolist" | "tobytes") {
                    return Ok(self.builtin(&format!("array.array.{name}")));
                }
                if class == super::namedtuple::GETTER
                    && let Some(value) = super::namedtuple::class_attribute(self, obj, name)
                {
                    return Ok(value);
                }
                match name {
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(short.to_owned()));
                    }
                    "__module__" => return Ok(self.text(module.to_owned())),
                    "MISSING" if class == "_contextvars.Token" => return Ok(self.token_missing),
                    "__hash__" | "__doc__" | "__text_signature__" if class == KEY_WRAPPER => {
                        return Ok(NONE);
                    }
                    "__new__"
                        if matches!(
                            class.as_str(),
                            KEY_WRAPPER | value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE
                        ) =>
                    {
                        return Ok(self.builtin("object.__new__"));
                    }
                    _ if class == KEY_WRAPPER
                        && BuiltinDescriptor::lookup(&format!("{class}.{name}")).is_some() =>
                    {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ if class == super::namedtuple::GETTER
                        && super::namedtuple::GETTER_METHODS.contains(&name) =>
                    {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ if class == super::deque::CLASS && super::deque::METHODS.contains(&name) => {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if class == "tuple" && matches!(name, "__new__" | "__getitem__") =>
            {
                return Ok(self.builtin(&format!("tuple.{name}")));
            }
            Value::Native(ref class) if matches!(class.as_str(), "map" | "filter") => match name {
                "__iter__" | "__next__" => return Ok(self.builtin(&format!("{class}.{name}"))),
                "__name__" | "__qualname__" => return Ok(self.text(class.clone())),
                "__module__" => return Ok(self.text("builtins")),
                _ => {}
            },
            Value::Native(ref class) if class == "datetime.time" => match name {
                "__name__" | "__qualname__" => return Ok(self.text("time")),
                "__module__" => return Ok(self.text("datetime")),
                _ => {}
            },
            Value::Template { strings, .. } if name == "strings" => return Ok(strings),
            Value::Template { interpolations, .. } if name == "interpolations" => {
                return Ok(interpolations);
            }
            Value::Template { interpolations, .. } if name == "values" => {
                return self.template_values(interpolations);
            }
            Value::Interpolation { value, .. } if name == "value" => return Ok(value),
            Value::Interpolation { expression, .. } if name == "expression" => {
                return Ok(expression);
            }
            Value::Interpolation { conversion, .. } if name == "conversion" => {
                return Ok(conversion);
            }
            Value::Interpolation { format_spec, .. } if name == "format_spec" => {
                return Ok(format_spec);
            }
            Value::Native(ref class) if let Some(class) = super::templatelib::class_name(class) => {
                if let Some(attr) = self.template_class_attr(class, name) {
                    return Ok(attr);
                }
            }
            Value::DirEntry(ref entry) if name == "name" => return Ok(entry.name),
            Value::DirEntry(ref entry) if name == "path" => return Ok(entry.path),
            Value::DirEntry(_) if matches!(name, "__new__" | "__reduce__" | "__reduce_ex__") => {
                let method = self.builtin(&format!("object.{name}"));
                let class = self.builtin(stdlib::os::scandir::ENTRY);
                return self.bind_attribute(method, obj, class);
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    stdlib::os::scandir::ENTRY | stdlib::os::scandir::ITER
                ) =>
            {
                match name {
                    "__new__" | "__reduce__" | "__reduce_ex__"
                        if class == stdlib::os::scandir::ENTRY =>
                    {
                        return Ok(self.builtin(&format!("object.{name}")));
                    }
                    "name" if class == stdlib::os::scandir::ENTRY => {
                        return Ok(self.readonly_member(stdlib::os::scandir::ENTRY, "name"));
                    }
                    "path" if class == stdlib::os::scandir::ENTRY => {
                        return Ok(self.readonly_member(stdlib::os::scandir::ENTRY, "path"));
                    }
                    "__module__" => return Ok(self.text("posix")),
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(class.trim_start_matches("posix.").to_owned()));
                    }
                    attr if (class == stdlib::os::scandir::ENTRY
                        && stdlib::os::scandir::ENTRY_METHODS.contains(&attr))
                        || (class == stdlib::os::scandir::ITER
                            && stdlib::os::scandir::ITER_METHODS.contains(&attr)) =>
                    {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class) if class == super::thread_lock::CLASS => match name {
                "__module__" => return Ok(self.text("_thread")),
                "__name__" | "__qualname__" => return Ok(self.text("lock")),
                attr if super::thread_lock::is_method(attr) => {
                    return Ok(self.builtin(&format!("{class}.{attr}")));
                }
                _ => {}
            },
            Value::Native(ref class) if class == super::thread_rlock::CLASS => match name {
                "__module__" => return Ok(self.text("_thread")),
                "__name__" | "__qualname__" => return Ok(self.text("RLock")),
                attr if super::thread_rlock::is_method(attr) => {
                    return Ok(self.builtin(&format!("{class}.{attr}")));
                }
                _ => {}
            },
            Value::Native(ref class) if class == super::thread_local::CLASS => match name {
                "__module__" => return Ok(self.text("_thread")),
                "__name__" | "__qualname__" => return Ok(self.text("_local")),
                attr if super::thread_local::is_method(attr) => {
                    return Ok(self.builtin(&format!("{class}.{attr}")));
                }
                _ => {}
            },
            Value::Native(ref class)
                if matches!(class.as_str(), "itertools.count" | "itertools.repeat") =>
            {
                match name {
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(class.trim_start_matches("itertools.").to_owned()));
                    }
                    "__module__" => return Ok(self.text("itertools")),
                    _ => {}
                }
            }
            Value::Native(ref class)
                if class == stdlib::datetime::TZINFO || class == "datetime.timezone" =>
            {
                let base = class == stdlib::datetime::TZINFO;
                match name {
                    "__module__" => return Ok(self.text("datetime")),
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(if base { "tzinfo" } else { "timezone" }));
                    }
                    "__base__" => {
                        let parent = if base {
                            "object"
                        } else {
                            stdlib::datetime::TZINFO
                        };
                        return Ok(self.builtin(parent));
                    }
                    "__bases__" | "__mro__" => return Ok(self.builtin_hierarchy_tuple(obj, name)),
                    "utc" if !base => return Ok(stdlib::datetime::utc(self)),
                    _ => {}
                }
            }
            Value::Native(ref class)
                if class == "mappingproxy" && matches!(name, "__repr__" | "__str__") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::MappingProxy(_) if matches!(name, "__repr__" | "__str__") => {
                let descriptor = self.builtin(&format!("mappingproxy.{name}"));
                let owner = self.builtin("mappingproxy");
                return self.bind_attribute(descriptor, obj, owner);
            }
            Value::MappingProxy(_)
                if matches!(name, "keys" | "items" | "values" | "get" | "copy") =>
            {
                return self.lookup_attr(self.mapping_proxy_target(obj), name);
            }
            Value::Native(ref symbol) if symbol == "open" && name == "__self__" => {
                if !self.builtins.contains_key("_io.open.__self__") {
                    self.module("_io")?;
                }
                return self.builtins.get("_io.open.__self__").copied().ok_or_else(|| {
                    Error::new("ImportError", "native _io module is unavailable")
                });
            }
            Value::Native(ref symbol)
                if name == "__self__"
                    && let Some(owner) = builtins::constructor_owner(symbol) =>
            {
                let owner = owner.to_owned();
                return Ok(self.builtin(&owner));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "int" | "float") && name == "__new__" =>
            {
                return Ok(self.builtin(&format!("{class}.__new__")));
            }
            Value::Super {
                start,
                receiver,
                owner,
            } => return self.super_attribute(start, receiver, owner, name),
            Value::Partial {
                function,
                args,
                keywords,
                dictionary,
            } => match name {
                "func" => return Ok(function),
                "args" => return Ok(args),
                "keywords" => return Ok(keywords),
                "__dict__" => return Ok(self.partial_dictionary(obj, dictionary)),
                _ => {
                    if let Some(value) = self.dictionary_field(dictionary, name)? {
                        return Ok(value);
                    }
                }
            },
            Value::File(ref handle)
                if matches!(name, "raw" | "name" | "mode")
                    && let super::stdlib::BinaryFile::Descriptor(fd) = handle.as_ref() =>
            {
                let raw = fd.raw;
                return if name == "raw" {
                    Ok(raw)
                } else {
                    self.lookup_attr(raw, name)
                };
            }
            Value::File(ref handle)
                if matches!(name, "name" | "mode")
                    && let Some((path, mode)) = handle.path_metadata() =>
            {
                return Ok(if name == "name" {
                    path
                } else {
                    self.text(mode)
                });
            }
            Value::File(ref handle)
                if name == "name"
                    && let Some(fd) = handle.socket_reader_name() =>
            {
                return Ok(self.int(i64::from(fd)));
            }
            Value::File(ref handle) if name == "closed" => {
                return Ok(if handle.closed() { TRUE } else { FALSE });
            }
            Value::Native(ref class) if class == "socket.socket" && name == "__name__" => {
                return Ok(self.text("socket"));
            }
            Value::Socket(ref socket) if name == "family" => {
                return Ok(self.int(i64::from(crate::objects::socket::family(socket))));
            }
            Value::Socket(ref socket) if name == "proto" => {
                return Ok(self.int(i64::from(crate::objects::socket::proto(socket))));
            }
            Value::Socket(ref socket) if name == "type" => {
                return Ok(self.int(i64::from(crate::objects::socket::kind(socket))));
            }
            Value::BufferedRandom(_, file_name) if name == "name" => return Ok(file_name),
            Value::BufferedRandom(..) if name == "mode" => return Ok(self.text("rb+")),
            Value::BufferedRandom(ref file, _) if name == "closed" => {
                return Ok(if file.borrow().closed() { TRUE } else { FALSE });
            }
            Value::TextFile(ref file)
                if name == "buffer"
                    && let Some(buffer) = file.buffer =>
            {
                return Ok(buffer);
            }
            Value::TextFile(ref file) if name == "closed" => {
                return Ok(if file.closed() { TRUE } else { FALSE });
            }
            Value::Raised(ref error)
                if let Some(index) = builtins::IMPORT_KEYWORDS
                    .iter()
                    .position(|&key| key == name)
                    && self.is_builtin_error(error, "ImportError") =>
            {
                let Some(fields) = &error.import else {
                    return Ok(error
                        .trace
                        .as_ref()
                        .and_then(|trace| trace.import_names)
                        .map_or(NONE, |names| names[index]));
                };
                let value = match name {
                    "name" => fields.name.as_deref(),
                    "path" => fields.path.as_deref(),
                    _ => fields.name_from.as_deref(),
                };
                return Ok(value
                    .map(str::to_owned)
                    .map_or(NONE, |value| self.text(value)));
            }
            Value::Raised(ref error) => {
                if let Some(state) = &error.decode {
                    match name {
                        "encoding" => return Ok(state.encoding),
                        "object" => return Ok(state.object),
                        "start" => return Ok(self.int(state.start)),
                        "end" => return Ok(self.int(state.end)),
                        "reason" => return Ok(state.reason),
                        _ => {}
                    }
                }
                if name == "args" {
                    return Ok(self.exception_argument_tuple(obj, error.clone()));
                }
                if matches!(&*error.kind, "UnicodeDecodeError" | "UnicodeEncodeError")
                    && let Some([encoding, object, start, end, reason]) =
                        self.exception_arguments(error)
                {
                    match name {
                        "encoding" => return Ok(*encoding),
                        "object" => return Ok(*object),
                        "start" => return Ok(*start),
                        "end" => return Ok(*end),
                        "reason" => return Ok(*reason),
                        _ => {}
                    }
                }
            }
            Value::DefaultDict { factory, .. } if name == "default_factory" => return Ok(factory),
            Value::Int(_) | Value::Bool(_) if name == "from_bytes" => {
                let owner = self.exact_type(obj)?;
                let method = self.builtin("int.from_bytes");
                return Ok(self.bind_native_classmethod(method, owner));
            }
            Value::Native(ref class)
                if name == "from_bytes" && matches!(class.as_str(), "int" | "bool") =>
            {
                let method = self.builtin("int.from_bytes");
                return Ok(self.bind_native_classmethod(method, obj));
            }
            Value::Float(_) if name == "__getformat__" => {
                let owner = self.builtin("float");
                let method = self.builtin("float.__getformat__");
                return Ok(self.bind_native_classmethod(method, owner));
            }
            Value::Native(ref class) if class == "float" && name == "__getformat__" => {
                let method = self.builtin("float.__getformat__");
                return Ok(self.bind_native_classmethod(method, obj));
            }
            Value::Native(ref class) if class == stdlib::flags::TYPE => {
                if let Some(value) = stdlib::flags::type_attribute(self, name, true) {
                    return Ok(value);
                }
            }
            Value::Native(ref symbol)
                if let Some(class) = stdlib::fixed_structseq::fixed_type(symbol) =>
            {
                if let Some(value) =
                    stdlib::fixed_structseq::type_attribute(self, class, name, true)
                {
                    return Ok(value);
                }
            }
            Value::Native(ref class)
                if name == "fromhex" && matches!(class.as_str(), "bytes" | "bytearray") =>
            {
                let method = self.builtin(&format!("{class}.fromhex"));
                return Ok(self.bind_native_classmethod(method, obj));
            }
            Value::Bytes(_) | Value::ByteArray(_) if name == "fromhex" => {
                let owner = self.exact_type(obj)?;
                let symbol = if matches!(self.heap.get(obj), Value::Bytes(_)) {
                    "bytes.fromhex"
                } else {
                    "bytearray.fromhex"
                };
                let method = self.builtin(symbol);
                return Ok(self.bind_native_classmethod(method, owner));
            }
            Value::Native(ref class)
                if name == "fromkeys"
                    && matches!(class.as_str(), "dict" | "collections.defaultdict") =>
            {
                let method = self.builtin("dict.fromkeys");
                return Ok(self.bind_native_classmethod(method, obj));
            }
            Value::Native(ref class)
                if class == "collections.defaultdict" && name == "__module__" =>
            {
                return Ok(self.text("collections"));
            }
            Value::Native(ref class)
                if class == "collections.defaultdict"
                    && matches!(name, "__name__" | "__qualname__") =>
            {
                return Ok(self.text("defaultdict"));
            }
            Value::Native(ref class)
                if class == "collections.defaultdict"
                    && let Some(member) = self.own_class_attr(obj, name) =>
            {
                return Ok(member);
            }
            Value::Struct {
                ref format,
                ref layout,
            } => match name {
                "format" => return Ok(self.text(format.clone())),
                "size" => return Ok(self.int(layout.size as i64)),
                _ => {}
            },
            Value::Hash(ref hash) => {
                let owner = hash.lock().class_name();
                match name {
                    "__init__" if HashState::is_provider_class(owner) => {
                        let init = self.builtin("object.__init__");
                        return Ok(self.alloc(Value::Method {
                            callable: init,
                            receiver: obj,
                            explicit: false,
                        }));
                    }
                    attr if let Some(value) = HashState::class_constant(owner, attr) => {
                        return Ok(self.int(value as i64));
                    }
                    "name" => {
                        let name = hash.lock().name();
                        return Ok(self.text(name));
                    }
                    "digest_size" => {
                        let size = hash.lock().digest_size();
                        return Ok(self.int(size as i64));
                    }
                    "block_size" => {
                        let size = hash.lock().block_size();
                        return Ok(self.int(size as i64));
                    }
                    _ => {}
                }
            }
            Value::CodecInfo { ref entry, decode } => match name {
                "name" => return Ok(self.text(entry.codec().name())),
                "decode" => return Ok(decode),
                _ => {}
            },
            Value::Hmac(mac) => match name {
                "digest_size" => return Ok(mac.digest_size),
                "block_size" => return Ok(mac.block_size),
                "name" => {
                    let name = self.lookup_attr(mac.inner, "name")?;
                    return Ok(self.text(format!("hmac-{}", self.str_value(name)?)));
                }
                _ => {}
            },
            Value::Weak(wr) if name == "__callback__" => return Ok(wr.callback),
            Value::Weak(_) if weakref::REFERENCE_METHODS.contains(&name) => {
                let callable = self.builtin(&format!("{}.{name}", weakref::REFERENCE_TYPE));
                let class = self.builtin(weakref::REFERENCE_TYPE);
                return self.bind_attribute(callable, obj, class);
            }
            Value::Native(ref class)
                if class == weakref::REFERENCE_TYPE
                    && weakref::REFERENCE_METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("{}.{name}", weakref::REFERENCE_TYPE)));
            }
            Value::Native(ref method)
                if method == weakref::REFERENCE_HASH && name == "__qualname__" =>
            {
                return Ok(self.text("ReferenceType.__hash__"));
            }
            Value::Native(ref builtin)
                if name == "__module__"
                    && (builtin.starts_with("weakref.") || builtin.starts_with("_weakref.")) =>
            {
                return Ok(self.text(builtin.split('.').next().unwrap().to_owned()));
            }
            Value::ContextVar { name: var_name, .. } if name == "name" => return Ok(var_name),
            Value::ContextToken { variable, .. } if name == "var" => return Ok(variable),
            Value::ContextToken { old, .. } if name == "old_value" => {
                return Ok(old.unwrap_or(self.token_missing));
            }
            Value::Native(ref builtin)
                if matches!(builtin.as_str(), "binascii.a2b_hex" | "binascii.unhexlify") =>
            {
                match name {
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(builtin.rsplit('.').next().unwrap().to_owned()));
                    }
                    "__module__" => return Ok(self.text("binascii")),
                    "__self__" => return self.module("binascii"),
                    "__text_signature__" => return Ok(self.text("($module, hexstr, /)")),
                    _ => {}
                }
            }
            Value::Native(ref builtin)
                if matches!(
                    builtin.as_str(),
                    "binascii.Error" | "binascii.Incomplete" | "socket.gaierror" | "socket.herror"
                ) =>
            {
                let (module, short) = builtin.split_once('.').unwrap();
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(short.to_owned())),
                    "__module__" => return Ok(self.text(module.to_owned())),
                    "__base__" | "__bases__" => {
                        let base = self.builtin(match short {
                            "gaierror" | "herror" => "OSError",
                            "Error" => "ValueError",
                            _ => "Exception",
                        });
                        return Ok(if name == "__base__" {
                            base
                        } else {
                            self.alloc(Value::Tuple(vec![base]))
                        });
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if matches!(name, "__bases__" | "__mro__") && builtins::is_exported_type(class) =>
            {
                return Ok(self.builtin_hierarchy_tuple(obj, name));
            }
            Value::Native(ref builtin)
                if name == "__name__" && self.builtins.get(builtin) == Some(&obj) =>
            {
                return Ok(self.text(builtin.rsplit('.').next().unwrap_or(builtin).to_owned()));
            }
            Value::Native(ref method)
                if method.starts_with("abc.abstract.") && name == "__isabstractmethod__" =>
            {
                return Ok(TRUE);
            }
            Value::Native(ref class)
                if class == "object"
                    && matches!(name, "__getstate__" | "__reduce__" | "__reduce_ex__") =>
            {
                return Ok(self.builtin(&format!("object.{name}")));
            }
            Value::Native(ref class)
                if name == "__format__" && self.builtin_type_name(obj) == Some("type") =>
            {
                let owner = builtins::format_owner(class);
                return Ok(self.builtin(&format!("{owner}.__format__")));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "bytes" | "bytearray")
                    && matches!(name, "__repr__" | "maketrans" | "translate") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Bytes(_) if name == "maketrans" => return Ok(self.builtin("bytes.maketrans")),
            Value::ByteArray(_) if name == "maketrans" => {
                return Ok(self.builtin("bytearray.maketrans"));
            }
            Value::Native(ref class)
                if class == "bool" && matches!(name, "__add__" | "bit_length" | "to_bytes") =>
            {
                return Ok(self.builtin(&format!("int.{name}")));
            }
            Value::Native(ref class) if class == "method_descriptor" && name == "__get__" => {
                return Ok(self.builtin("method_descriptor.__get__"));
            }
            Value::Native(ref class) if class == "collections.defaultdict" && name == "clear" => {
                return Ok(self.builtin("dict.clear"));
            }
            Value::Native(ref class)
                if class == "collections.defaultdict" && matches!(name, "copy" | "__copy__") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if name == "__reduce__"
                    && matches!(
                        class.as_str(),
                        "set" | "frozenset" | "collections.defaultdict"
                    ) =>
            {
                return Ok(self.builtin(&format!("{class}.__reduce__")));
            }
            Value::Dict(_) | Value::DefaultDict { .. } if name == "fromkeys" => {
                let owner = self.exact_type(obj)?;
                let method = self.builtin("dict.fromkeys");
                return Ok(self.bind_native_classmethod(method, owner));
            }
            Value::Native(ref class) if class == "set" && name == "copy" => {
                return Ok(self.builtin("set.copy"));
            }
            Value::Native(ref class)
                if class == "set" && matches!(name, "discard" | "difference_update" | "update") =>
            {
                return Ok(self.builtin(&format!("set.{name}")));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "dict" | "list" | "tuple")
                    || (class == "function" && name == "__call__")
                    || builtins::is_enum_base(class)
                    || (matches!(name, "__repr__" | "issubset")
                        && matches!(class.as_str(), "set" | "frozenset"))
                    || (builtins::is_exception(class)
                        && matches!(
                            name,
                            "__new__" | "__init__" | "add_note" | "with_traceback"
                        ))
                    || (class == "memoryview" && builtins::MEMORYVIEW_METHODS.contains(&name))
                    || (class == "bytes" && builtins::BYTES_METHODS.contains(&name))
                    || (class == "bytearray" && builtins::BYTEARRAY_METHODS.contains(&name))
                    || (name == "__repr__" && matches!(class.as_str(), "bool" | "int" | "float"))
                    || (class == "int" && matches!(name, "__add__" | "bit_length" | "to_bytes"))
                    || (class == "float" && name == "as_integer_ratio")
                    || (class == "str"
                        && (name == "maketrans" || builtins::STR_METHODS.contains(&name))) =>
            {
                if let Some(method) = self.class_attr(obj, name) {
                    return Ok(method);
                }
            }
            Value::Native(ref class)
                if class == "object"
                    && matches!(
                        name,
                        "__hash__"
                            | "__repr__"
                            | "__str__"
                            | "__getattribute__"
                            | "__setattr__"
                            | "__delattr__"
                            | "__init__"
                            | "__new__"
                    ) =>
            {
                return Ok(self.builtin(&format!("object.{name}")));
            }
            Value::Native(ref class)
                if DictIterator::methods(class).contains(&name)
                    && DictIterator::class(class).is_some() =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if class == "slice" && super::slice::METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("slice.{name}")));
            }
            Value::Native(ref class)
                if class == super::namedtuple::GETTER
                    && super::namedtuple::GETTER_METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("{}.{name}", super::namedtuple::GETTER)));
            }
            Value::Native(ref class) if class == "operator.itemgetter" && name == "__reduce__" => {
                return Ok(self.builtin("operator.itemgetter.__reduce__"));
            }
            Value::Native(ref class)
                if (class == "range" && super::range::METHODS.contains(&name))
                    || (matches!(class.as_str(), "range_iterator" | "longrange_iterator")
                        && matches!(name, "__iter__" | "__next__" | "__length_hint__")) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if class == "enumerate" && matches!(name, "__iter__" | "__next__") =>
            {
                return Ok(self.builtin(&format!("enumerate.{name}")));
            }
            Value::Native(ref class) if class == "str" && name == "__new__" => {
                return Ok(self.builtin("str.__new__"));
            }
            Value::Module {
                name: ref module,
                scope,
                constructor,
                ..
            } => {
                if name == "__dict__" {
                    return self.module_globals(scope);
                }
                if name == "__class__" {
                    return Ok(constructor.unwrap_or_else(|| self.builtin("module")));
                }
                if constructor.is_none() && stdlib::has_lazy_attributes(module) {
                    if let Some(value) = self.scope_name(scope, name) {
                        return Ok(value);
                    }
                    match stdlib::attribute(self, &module.to_owned(), name) {
                        Ok(value) => {
                            self.bind(scope, name, value);
                            return Ok(value);
                        }
                        Err(error) if self.is_attribute_error(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                let class = constructor.unwrap_or_else(|| self.builtin("module"));
                return self.module_instance_attribute(obj, class, scope, name);
            }
            Value::Class {
                name: ref class_name,
                scope,
                ..
            } => {
                if name == "__dict__" {
                    return Ok(self.class_dictionary(scope));
                }
                if matches!(name, "__annotations__" | "__annotate__")
                    && self.has_class_annotation_provider(scope)
                {
                    return self.class_annotation_attr(obj, scope, name);
                }
                if name == "__type_params__" {
                    return Ok(self
                        .own_class_attr(obj, name)
                        .unwrap_or_else(|| self.heap.packed_tuple(Vec::new(), false)));
                }
                if name == "__annotations__"
                    && let Some(sites) = self
                        .scope_data(scope)
                        .class_namespace
                        .as_ref()
                        .and_then(|namespace| namespace.annotation_sites.as_ref())
                {
                    let sites: Vec<_> = sites.values().copied().collect();
                    // An annotation can replace __annotations__ and collect the old thunks.
                    let mark = self.heap.temporary_mark();
                    for &(key, thunk) in &sites {
                        self.heap.keep(key);
                        self.heap.keep(thunk);
                    }
                    let mapping = self.dict(Vec::new());
                    self.heap.keep(mapping);
                    let result = (|| {
                        for (key, thunk) in sites {
                            let value = self.call(thunk, Vec::new(), Vec::new())?;
                            self.put_item(mapping, key, value)?;
                        }
                        self.bind_local(scope, name, mapping);
                        let Value::Scope(frame) = self.heap.get_mut(scope) else {
                            unreachable!("class scope")
                        };
                        frame.class_namespace.as_mut().unwrap().annotation_sites = None;
                        Ok(mapping)
                    })();
                    self.heap.release_temporaries_since(mark);
                    return result;
                }
                if name == "__name__" {
                    let assigned = self
                        .scope_data(scope)
                        .class_namespace
                        .as_ref()
                        .and_then(|namespace| namespace.name_override);
                    if let Some(name) = assigned {
                        return Ok(name);
                    }
                    let class_name = class_name.clone();
                    return Ok(self.heap.code_name(&class_name));
                }
                if let Some(value) = self.class_attr(obj, name) {
                    return self.bind_attribute(value, NONE, obj);
                }
                if name == "_abc_caches_clear" && self.is_protocol_declaration(obj) {
                    let callable = self.builtin("abc._abc_caches_clear");
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: true,
                    }));
                }
                if name == "register"
                    && self.heap.get(obj).instance_class().is_none()
                    && (self.is_bundled_abc(obj) || super::mapping_abc::is_abc_class(self, obj))
                {
                    let callable = self.builtin("abc.register");
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: true,
                    }));
                }
                if name == "__new__" && self.has_base(obj, "str") {
                    return Ok(self.builtin("str.__new__"));
                }
                if name == "__new__" && self.has_base(obj, "float") {
                    return Ok(self.builtin("float.__new__"));
                }
                if let Some((meta, member)) = metaclass_member {
                    return self.bind_attribute(member, obj, meta);
                }
            }
            Value::NamedTuple {
                class,
                ref names,
                ref values,
                ref extras,
            } => {
                if let Some((_, value)) = extras.iter().find(|(key, _)| *key == name) {
                    return Ok(*value);
                }
                if let Some(i) = names.iter().position(|n| n == name) {
                    return Ok(values[i]);
                }
                if class == Some(stdlib::os::stat::CLASS) {
                    if name == "__class__" {
                        return Ok(self.builtin(stdlib::os::stat::CLASS));
                    }
                    if let Some(n) = stdlib::os::stat::count_attribute(name) {
                        return Ok(self.int(n));
                    }
                }
                if let Some(
                    owner @ ("time.struct_time"
                    | stdlib::fixed_structseq::UNRAISABLE_ARGS
                    | stdlib::fixed_structseq::HASH_INFO
                    | stdlib::fixed_structseq::TERMINAL_SIZE
                    | stdlib::flags::TYPE),
                ) = class
                    && name == "__class__"
                {
                    return Ok(self.builtin(owner));
                }
                if matches!(
                    class,
                    Some("SplitResult" | "SplitResultBytes" | "ParseResult" | "ParseResultBytes")
                ) && let Some(property) = stdlib::urllib::property(self, values[1], name)?
                {
                    return Ok(property);
                } else if class == Some(stdlib::flags::TYPE)
                    && let Some(value) = stdlib::flags::type_attribute(self, name, false)
                {
                    return Ok(value);
                } else if let Some(owner) = class.and_then(stdlib::fixed_structseq::fixed_type)
                    && let Some(value) =
                        stdlib::fixed_structseq::type_attribute(self, owner, name, false)
                {
                    return Ok(value);
                }
            }
            _ => {}
        }
        if let Value::Raised(error) = self.heap.get(obj)
            && !self.native_exception_descriptor(error, name)
        {
            let dictionary = error.trace.as_ref().and_then(|trace| trace.dictionary);
            if let Some(field) = self.dictionary_field(dictionary, name)? {
                return Ok(field);
            }
            if name == "add_note" {
                let class = self.exact_type(obj)?;
                return self.instance_attribute(obj, class, None, name);
            }
        }
        self.builtin_attribute(obj, name)
    }

    fn builtin_attribute(&mut self, obj: Id, name: &str) -> Result<Id> {
        if name == "__init_subclass__" {
            let class = if self.builtin_type_name(obj) == Some("type") {
                obj
            } else {
                self.exact_type(obj)?
            };
            let hook = self.builtin("object.__init_subclass__");
            return self.bind_attribute(hook, NONE, class);
        }
        if name == "__init__" {
            let tuple = self.builtin("tuple");
            if self.direct_subclass(obj, tuple) {
                return Ok(self.builtin("object.__init__"));
            }
            if self.instance_of(obj, tuple) {
                let init = self.builtin("object.__init__");
                return self.bind_attribute(init, obj, tuple);
            }
        }
        if name == "__getstate__" {
            let callable = self.builtin("object.__getstate__");
            if self.builtin_type_name(obj) == Some("type") {
                return Ok(callable);
            }
            return Ok(self.alloc(Value::Method {
                callable,
                receiver: obj,
                explicit: false,
            }));
        }
        let base = self.scalar_value(obj);
        if name == "__format__" {
            let owner = builtins::format_owner(self.type_name(base));
            let callable = self.builtin(&format!("{owner}.__format__"));
            return Ok(self.alloc(Value::Method {
                callable,
                receiver: obj,
                explicit: false,
            }));
        }
        if name == "maketrans" && matches!(self.heap.get(base), Value::Str(_)) {
            return Ok(self.builtin("str.maketrans"));
        }
        if name == "__new__" {
            match self.heap.get(base) {
                Value::Dict(_) | Value::DefaultDict { .. } => {
                    return Ok(self.builtin("dict.__new__"));
                }
                Value::Int(_) => return Ok(self.builtin("int.__new__")),
                Value::Str(_) | Value::SurrogateStr(_) => return Ok(self.builtin("str.__new__")),
                Value::Class { .. } | Value::Instance { .. } => {
                    let class = self.heap.get(base).instance_class().unwrap_or(base);
                    if self.allocation_base(class) == self.builtins.get("object").copied() {
                        return Ok(self.builtin("object.__new__"));
                    }
                }
                _ => {}
            }
        }
        if let Some(method) = builtins::method_name(self.heap.get(base), name) {
            let fun = self.alloc(Value::Native(method));
            return Ok(self.alloc(Value::Method {
                callable: fun,
                receiver: obj,
                explicit: false,
            }));
        }
        Err(self.missing_attribute(obj, name))
    }
    /// A missing T_OBJECT member reads as None, even after repeated deletions.
    fn update_key_wrapper_field(&mut self, obj: Id, name: &str, value: Option<Id>) -> Result<bool> {
        let Value::KeyWrapper { object, .. } = self.heap.get_mut(obj) else {
            return Ok(false);
        };
        match name {
            "obj" => {
                if let Some(old) = std::mem::replace(object, value) {
                    self.heap.weak_edge_removed(old);
                }
                Ok(true)
            }
            "__text_signature__" => Err(Error::new(
                "AttributeError",
                format!("attribute '{name}' of '{KEY_WRAPPER}' objects is not writable"),
            )),
            _ => Ok(false),
        }
    }

    pub(super) fn delete_attr(&mut self, obj: Id, name: &str) -> Result<()> {
        if let Value::Native(class) = self.heap.get(obj)
            && HashState::is_provider_class(class)
        {
            return Err(Error::new(
                "TypeError",
                format!("cannot set '{name}' attribute of immutable type '{class}'"),
            ));
        }

        self.delete_named_attr(obj, name, None)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn delete_named_attr(
        &mut self,
        obj: Id,
        name: &str,
        original: Option<Id>,
    ) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::DirEntry(_)) && matches!(name, "name" | "path") {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_local_attribute(
                obj,
                super::thread_local::Name::from_lookup(name, original),
                None,
            );
        }
        if matches!(self.heap.get(obj), Value::Range(_) | Value::Slice(_))
            && matches!(name, "start" | "stop" | "step")
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(hook) = self.bound_slot(obj, "__delattr__")? {
            let name = original.unwrap_or_else(|| self.text(name));
            self.call(hook, vec![name], Vec::new())?;
            return Ok(());
        }
        self.default_delete_attr(obj, name)
    }

    pub(super) fn delete_dict_attribute(
        &mut self,
        obj: Id,
        dictionary: Id,
        name: &str,
    ) -> Result<()> {
        let key = self.text(name);
        match self.delete_item(dictionary, key) {
            Err(error) if error.kind == "KeyError" => Err(self.missing_attribute(obj, name)),
            result => result,
        }
    }

    /// Bypass the object's deletion hook, while honoring data descriptors on its class.
    pub(super) fn default_delete_attr(&mut self, obj: Id, name: &str) -> Result<()> {
        self.check_template_write(obj, name)?;
        super::namespace::check_class_write(self, obj, name)?;
        stdlib::io::check_class_write(self, obj, name)?;
        self.check_thread_reporting_binding(obj, name)?;
        if name == "__doc__" && matches!(self.heap.get(obj), Value::TupleGetter { .. }) {
            return super::namedtuple::set_document(self, obj, None).map(|_| ());
        }
        if name == "cell_contents" && matches!(self.heap.get(obj), Value::Cell(_)) {
            self.heap.replace_cell(obj, None);
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_base_local_attribute(obj, name, None);
        }
        let exception_member = self.exception_member(obj, name);
        let member = self.writable_attribute_member(obj, name);
        if member.is_none()
            && self.exposes_defaultdict_factory(obj, name)
            && self.replace_defaultdict_factory(obj, NONE)
        {
            return Ok(());
        }
        if name == "__callback__"
            && member.is_none()
            && self.heap.get(obj).weak_reference().is_some()
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(descriptor) = member.or(exception_member)
            && self.data_descriptor(descriptor)
        {
            match *self.heap.get(descriptor) {
                Value::SlotMember { .. } => return self.write_slot_member(descriptor, obj, None),
                Value::TupleGetterDoc => {
                    return super::namedtuple::set_document(self, obj, None).map(|_| ());
                }
                Value::ReadOnlyMember(_, _) => {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                Value::TupleGetter { .. } => {
                    return Err(Error::new("AttributeError", "can't delete attribute"));
                }
                Value::Property(property) => return property.remove(self, obj),
                Value::WeakrefDescriptor(owner) => return Err(self.readonly_weakref(owner)),
                Value::GetSetDescriptor { set: Some(set), .. } => {
                    self.call(set, vec![obj], Vec::new())?;
                    return Ok(());
                }
                Value::GetSetDescriptor { owner, name, .. } => {
                    return Err(Error::new(
                        "AttributeError",
                        format!("attribute '{name}' of '{owner}' objects is not writable"),
                    ));
                }
                _ => {}
            }
            let delete = self
                .bound_slot(descriptor, "__delete__")?
                .ok_or_else(|| Error::new("AttributeError", "__delete__"))?;
            self.call(delete, vec![obj], Vec::new())?;
            return Ok(());
        }
        if self.update_syntax_member(obj, name, NONE) {
            return Ok(());
        }
        if exception_member.is_some() {
            return self.update_exception_field(obj, name, None);
        }
        if self.update_key_wrapper_field(obj, name, None)?
            || self.update_os_field(obj, name, None)?
        {
            return Ok(());
        }
        if name == "__dict__"
            && matches!(
                self.heap.get(obj),
                Value::Raised(_) | Value::Cached { .. } | Value::Partial { .. }
            )
        {
            return Err(Error::new("TypeError", "cannot delete __dict__"));
        }
        if name == "name"
            && let Some(error) = self.exception_storage(obj, "NameError")
        {
            if let Some(trace) = error.trace.as_mut() {
                trace.name_error_name = None;
            }
            return Ok(());
        }
        if name == "value"
            && let Some(error) = self.exception_storage(obj, "StopIteration")
        {
            error.set_stop_value(NONE);
            return Ok(());
        }
        if name == "msg" && self.import_error_message(obj, Some(NONE)).is_some() {
            return Ok(());
        }
        if name == "args" && matches!(self.heap.get(obj), Value::Raised(_)) {
            return Err(Error::new("TypeError", "args may not be deleted"));
        }
        if name == "__class__" && member.is_none() {
            return Err(Error::new("TypeError", "can't delete __class__ attribute"));
        }
        if member.is_none()
            && matches!(name, "__name__" | "fget" | "fset" | "fdel")
            && let Some(property) = self.heap.get_mut(obj).property_mut()
        {
            if name == "__name__" {
                property.name = None;
                return Ok(());
            }
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Value::Raised(error) = self.heap.get(obj)
            && !self.native_exception_descriptor(error, name)
        {
            return self.update_exception_field(obj, name, None);
        }
        self.check_instance_field(obj, name)?;
        match *self.heap.get(obj) {
            Value::Partial { dictionary, .. } => {
                if matches!(name, "func" | "args" | "keywords") {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                let dictionary = self.partial_dictionary(obj, dictionary);
                self.update_dictionary_field(obj, dictionary, name, None)
            }
            Value::Cached { dictionary, .. } => {
                if name == "__dict__" {
                    return Err(Error::new("TypeError", "cannot delete __dict__"));
                }
                self.update_dictionary_field(obj, dictionary, name, None)
            }
            Value::Function { fields, .. } => self.delete_function_attr(obj, fields, name),
            Value::Code(_) if name == "co_qualname" => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
            Value::Property(_) if name == "__doc__" => {
                self.heap.get_mut(obj).property_mut().unwrap().doc = NONE;
                Ok(())
            }
            Value::StaticMethod { dictionary, .. } => {
                super::static_method::update_attribute(self, obj, dictionary, name, None)
            }
            Value::Module { scope, .. } => {
                self.update_module_annotations(scope, name, None)?;
                if name == "__dict__" {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                let key = self.text(name);
                if self.remove_binding(scope, name, key)? {
                    Ok(())
                } else {
                    Err(self.missing_attribute(obj, name))
                }
            }
            Value::Class { scope, .. } => {
                if matches!(name, "__annotations__" | "__annotate__")
                    && self.has_class_annotation_provider(scope)
                {
                    return self.update_class_annotation_attr(scope, name, None);
                }
                let error = match name {
                    "__name__" | "__qualname__" | "__bases__" | "__type_params__"
                    | "__module__" | "__doc__" => Error::new(
                        "TypeError",
                        format!(
                            "cannot delete '{name}' attribute of immutable type '{}'",
                            self.class_name(obj)
                        ),
                    ),
                    "__dict__" | "__mro__" => Error::new(
                        "AttributeError",
                        format!("attribute '{name}' of 'type' objects is not writable"),
                    ),
                    "__base__" => Error::new("AttributeError", "readonly attribute"),
                    "__annotate__" => {
                        Error::new("TypeError", "cannot delete __annotate__ attribute")
                    }
                    "__annotations__"
                        if self
                            .scope_data(scope)
                            .class_namespace
                            .as_ref()
                            .is_some_and(|namespace| namespace.annotation_sites.is_some()) =>
                    {
                        Error::new("AttributeError", "__annotations__")
                    }
                    _ => {
                        let key = self.text(name);
                        if self.remove_binding(scope, name, key)? {
                            if name == "__abstractmethods__"
                                && let Value::Scope(data) = self.heap.get_mut(scope)
                            {
                                data.is_abstract = false;
                            }
                            return Ok(());
                        }
                        Error::new(
                            "AttributeError",
                            format!(
                                "type object '{}' has no attribute '{name}'",
                                self.class_name(obj)
                            ),
                        )
                    }
                };
                Err(error)
            }
            Value::Instance { class, .. } if self.builtins.get("object") != Some(&class) => {
                let layout = self.instance_layout(class, name);
                if name == "__dict__" && member.is_none() {
                    if !layout.dictionary {
                        return Err(self.missing_attribute(obj, name));
                    }
                    self.replace_instance_dictionary(obj, class, None);
                    return Ok(());
                }
                if !layout.active_slot
                    && let Value::Instance {
                        namespace: Some(storage),
                        ..
                    } = *self.heap.get(obj)
                {
                    return self.update_dictionary_field(obj, storage, name, None);
                }
                if (layout.declared_slot && !layout.active_slot)
                    || self.has_namedtuple_field(class, name)
                {
                    return Err(self.missing_attribute(obj, name));
                }
                let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                let value = fields
                    .shift_remove(name)
                    .ok_or_else(|| self.missing_attribute(obj, name))?;
                self.heap.weak_edge_removed(value);
                Ok(())
            }
            Value::Native(ref class) if self.builtin_type_name(obj) == Some("type") => {
                Err(Error::new(
                    "TypeError",
                    format!("cannot set '{name}' attribute of immutable type '{class}'"),
                ))
            }
            _ => Err(Error::new(
                "AttributeError",
                format!(
                    "'{}' object has no attribute '{name}' and no __dict__ for setting new attributes",
                    self.type_name(obj)
                ),
            )),
        }
    }

    pub(super) fn class_instances_have_dict(&self, class: Id) -> bool {
        self.instance_layout(class, "").dictionary
    }

    /// A source slot stored inline has no boxed descriptor to return to inspect.
    pub(super) fn is_inline_instance_slot(&self, class: Id, name: &str) -> bool {
        self.instance_layout(class, name).active_slot
    }

    /// Whether instances have a dictionary and whether this particular name addresses a slot.
    fn instance_layout(&self, class: Id, name: &str) -> InstanceLayout {
        let mut layout = InstanceLayout::default();
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return layout;
        };
        let mut shadowed = false;
        for &base in std::iter::once(&class).chain(ancestors) {
            let Value::Class {
                scope, name: owner, ..
            } = self.heap.get(base)
            else {
                continue;
            };
            let frame = self.scope_data(*scope);
            shadowed |= self.namespace_binding(frame, name).is_some();
            let Some(slots) = frame
                .class_namespace
                .as_deref()
                .and_then(|namespace| namespace.slot_names.as_deref())
            else {
                layout.dictionary = true;
                continue;
            };
            for slot in slots {
                match slot.as_str() {
                    "__dict__" => layout.dictionary = true,
                    "__weakref__" => {}
                    slot => {
                        let owner = owner.trim_start_matches('_');
                        let matches = if slot.starts_with("__")
                            && !slot.ends_with("__")
                            && !owner.is_empty()
                        {
                            name.strip_prefix('_').and_then(|n| n.strip_prefix(owner)) == Some(slot)
                        } else {
                            name == slot
                        };
                        if matches {
                            layout.declared_slot = true;
                            layout.active_slot |= !shadowed
                                && !frame
                                    .class_namespace
                                    .as_ref()
                                    .is_some_and(|ns| ns.member_slots);
                        }
                    }
                }
            }
        }
        layout
    }

    fn writable_attribute_member(&mut self, obj: Id, name: &str) -> Option<Id> {
        if let Value::NamedTuple {
            class: Some(owner), ..
        } = *self.heap.get(obj)
        {
            let member = if owner == stdlib::os::stat::CLASS {
                stdlib::os::stat::member(self, name)
            } else if let Some(class) = stdlib::fixed_structseq::fixed_type(owner) {
                stdlib::fixed_structseq::member(self, class, name)
            } else {
                None
            };
            if member.is_some() {
                return member;
            }
        }
        if matches!(
            self.heap.get(obj),
            Value::NamedTuple {
                class: Some(stdlib::flags::TYPE),
                ..
            }
        ) && let Some(member) = stdlib::flags::member(self, name)
        {
            return Some(member);
        }
        let member = self.slot(obj, name)?;
        if let Value::Instance { class, .. } = *self.heap.get(obj)
            && self.instance_layout(class, name).active_slot
        {
            return None;
        }
        Some(member)
    }

    /// A declared instance slot shadows the mapping's inherited factory member.
    fn exposes_defaultdict_factory(&self, obj: Id, name: &str) -> bool {
        name == "default_factory"
            && !matches!(*self.heap.get(obj), Value::Instance { class, .. } if self.instance_layout(class, name).active_slot)
    }

    /// Move ordinary fields out of the inline storage while keeping declared slots in place.
    fn take_instance_fields(&mut self, obj: Id, class: Id) -> Vec<(String, Id)> {
        let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        let original = std::mem::take(fields);
        let mut ordinary = Vec::new();
        let mut retained = indexmap::IndexMap::new();
        for (name, value) in original {
            if self.instance_layout(class, &name).declared_slot
                || self
                    .namedtuple_fields(class)
                    .is_some_and(|names| names.contains(&name))
            {
                retained.insert(name, value);
            } else {
                ordinary.push((name, value));
            }
        }
        let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        *fields = retained;
        ordinary
    }

    /// Look up native backing fields on either side of ordinary dictionary materialization.
    pub(super) fn raw_instance_field(&self, obj: Id, name: &str) -> Option<Id> {
        let Value::Instance {
            fields, namespace, ..
        } = self.heap.get(obj)
        else {
            return None;
        };
        fields
            .get(name)
            .copied()
            .or_else(|| self.dict_named_item(self.dict_storage((*namespace)?)?, name))
    }

    /// Return the live instance dictionary for default object state, bypassing attribute hooks.
    pub(super) fn pickle_instance_state_dict(&mut self, obj: Id) -> Result<Id> {
        let dictionary = match *self.heap.get(obj) {
            Value::Instance {
                class,
                ref fields,
                namespace,
                ..
            } if (!fields.is_empty() || namespace.is_some())
                && self.class_instances_have_dict(class) =>
            {
                Some(self.instance_dictionary(obj, class))
            }
            Value::Module { scope, .. } => Some(self.module_globals(scope)?),
            Value::Function { fields, .. } => self.function_attr(obj, fields, "__dict__")?,
            Value::Raised(ref error) => error.trace.as_ref().and_then(|trace| trace.dictionary),
            Value::Partial { dictionary, .. } => dictionary,
            Value::StaticMethod { dictionary, .. } | Value::Cached { dictionary, .. } => {
                Some(dictionary)
            }
            Value::Class { scope, .. }
                if self.classes.get(super::namespace::CLASS) != Some(&obj) =>
            {
                let proxy = self.class_dictionary(scope);
                Some(self.mapping_proxy_target(proxy))
            }
            _ => None,
        };
        Ok(dictionary
            .filter(|&dictionary| {
                !self
                    .dict_entries(dictionary)
                    .expect("instance dictionary")
                    .is_empty()
            })
            .unwrap_or(NONE))
    }

    /// Lazily expose the dictionary that subsequent ordinary attribute operations use.
    pub(super) fn instance_dictionary(&mut self, obj: Id, class: Id) -> Id {
        if let Value::Instance {
            namespace: Some(dictionary),
            ..
        } = *self.heap.get(obj)
        {
            return dictionary;
        }
        let entries = self
            .take_instance_fields(obj, class)
            .into_iter()
            .map(|(name, value)| (self.text(name), value))
            .collect();
        let dictionary = self.dict(entries);
        let Value::Instance { namespace, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        *namespace = Some(dictionary);
        dictionary
    }

    fn replace_instance_dictionary(&mut self, obj: Id, class: Id, new: Option<Id>) {
        let removed = self.take_instance_fields(obj, class);
        let Value::Instance { namespace, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        let previous = std::mem::replace(namespace, new);
        if previous != new
            && let Some(old) = previous
        {
            self.heap.weak_edge_removed(old);
        }
        for (_, value) in removed {
            self.heap.weak_edge_removed(value);
        }
    }

    /// A slot-only instance can retain fields declared by any of its Python bases.
    fn check_instance_field(&mut self, obj: Id, name: &str) -> Result<()> {
        let Value::Instance { class, .. } = *self.heap.get(obj) else {
            return Ok(());
        };
        if !matches!(self.heap.get(class), Value::Class { .. }) {
            return Ok(());
        }
        let layout = self.instance_layout(class, name);
        if layout.dictionary || layout.active_slot {
            return Ok(());
        }
        let attribute = self.text(name);
        let quoted = self.repr(attribute);
        let kind = self.type_name(obj);
        let message = if self.class_attr(class, name).is_some() {
            format!("'{kind}' object attribute {quoted} is read-only")
        } else {
            format!(
                "'{kind}' object has no attribute {quoted} and no __dict__ for setting new attributes"
            )
        };
        Err(Error::new("AttributeError", message))
    }

    pub(super) fn set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<()> {
        self.set_named_attr(obj, name, None, value)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn set_named_attr(
        &mut self,
        obj: Id,
        name: &str,
        original: Option<Id>,
        value: Id,
    ) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::DirEntry(_)) && matches!(name, "name" | "path") {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_local_attribute(
                obj,
                super::thread_local::Name::from_lookup(name, original),
                Some(value),
            );
        }
        if matches!(
            self.heap.get(obj),
            Value::Raised(Error {
                decode: Some(_),
                ..
            })
        ) && matches!(name, "encoding" | "object" | "start" | "end" | "reason")
        {
            let offset = if matches!(name, "start" | "end") {
                let integer = self
                    .as_int(value)
                    .map_err(|_| Error::new("TypeError", "an integer is required"))?;
                Some(
                    integer
                        .to_i64()
                        .and_then(|n| isize::try_from(n).ok())
                        .ok_or_else(|| {
                            Error::new(
                                "OverflowError",
                                "Python int too large to convert to C ssize_t",
                            )
                        })? as i64,
                )
            } else {
                None
            };
            let Value::Raised(Error {
                decode: Some(state),
                ..
            }) = self.heap.get_mut(obj)
            else {
                unreachable!();
            };
            match name {
                "encoding" => state.encoding = value,
                "object" => state.object = value,
                "start" => state.start = offset.unwrap(),
                "end" => state.end = offset.unwrap(),
                "reason" => state.reason = value,
                _ => unreachable!(),
            }
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::Range(_) | Value::Slice(_))
            && matches!(name, "start" | "stop" | "step")
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(hook) = self.bound_slot(obj, "__setattr__")? {
            let name = original.unwrap_or_else(|| self.text(name));
            self.call(hook, vec![name, value], Vec::new())?;
            return Ok(());
        }
        self.default_set_attr(obj, name, value)
    }

    pub(super) fn default_set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<()> {
        if self.builtins.get("abc.ABCMeta") == Some(&obj)
            && matches!(
                name,
                "__new__"
                    | "__init__"
                    | "__prepare__"
                    | "mro"
                    | "__bases__"
                    | "__instancecheck__"
                    | "__subclasscheck__"
            )
        {
            return Err(Error::new(
                "NotImplementedError",
                "custom ABCMeta construction and subclass checks are unsupported",
            ));
        }
        self.check_template_write(obj, name)?;
        super::namespace::check_class_write(self, obj, name)?;
        stdlib::io::check_class_write(self, obj, name)?;
        if let Value::Native(class) = self.heap.get(obj)
            && (builtins::is_static_gc_type(class)
                || HashState::is_provider_class(class)
                || matches!(
                    class.as_str(),
                    HashState::CLASS
                        | HashState::XOF_CLASS
                        | KEY_WRAPPER
                        | stdlib::flags::TYPE
                        | value::REGEX_PATTERN_TYPE
                        | value::REGEX_MATCH_TYPE
                ))
        {
            return Err(Error::new(
                "TypeError",
                format!("cannot set '{name}' attribute of immutable type '{class}'"),
            ));
        }
        self.check_thread_reporting_binding(obj, name)?;
        if name == "__doc__" && matches!(self.heap.get(obj), Value::TupleGetter { .. }) {
            return super::namedtuple::set_document(self, obj, Some(value)).map(|_| ());
        }
        if name == "__callback__"
            && self.slot(obj, name).is_none()
            && self.heap.get(obj).weak_reference().is_some()
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_base_local_attribute(obj, name, Some(value));
        }
        if name == "__abstractmethods__"
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            return self.set_abstract_methods(scope, value);
        }
        if name == "__name__"
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            let text = self.str_value(value).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "can only assign string to {}.__name__, not '{}'",
                        self.class_name(obj),
                        self.type_name(value)
                    ),
                )
            })?;
            if text.contains('\0') {
                return Err(Error::new(
                    "ValueError",
                    "type name must not contain null characters",
                ));
            }
            let Value::Scope(frame) = self.heap.get_mut(scope) else {
                unreachable!()
            };
            let old = frame
                .class_namespace
                .get_or_insert_with(Default::default)
                .name_override
                .replace(value);
            if let Some(old) = old
                && old != value
            {
                self.heap.weak_edge_removed(old);
            }
            return Ok(());
        }
        if name == "__traceback__" && matches!(self.heap.get(obj), Value::Raised(_)) {
            return self.set_traceback(obj, value);
        }
        if name == "__context__" && matches!(self.heap.get(obj), Value::Raised(_)) {
            if value != NONE && !matches!(self.heap.get(value), Value::Raised(_)) {
                return Err(Error::new(
                    "TypeError",
                    "exception context must be None or derive from BaseException",
                ));
            }
            let Value::Raised(error) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            error.context = value;
            return Ok(());
        }
        if let Value::StaticMethod { dictionary, .. } = *self.heap.get(obj) {
            return super::static_method::update_attribute(
                self,
                obj,
                dictionary,
                name,
                Some(value),
            );
        }
        if name == "cell_contents" && matches!(self.heap.get(obj), Value::Cell(_)) {
            self.heap.replace_cell(obj, Some(value));
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::Function { .. }) {
            return self.set_function_attr(obj, name, value);
        }
        let exception_member = self.exception_member(obj, name);
        let member = self.writable_attribute_member(obj, name);
        if let Some(descriptor) = member.or(exception_member)
            && self.data_descriptor(descriptor)
        {
            if let Value::Property(property) = *self.heap.get(descriptor) {
                return property.write(self, obj, value);
            }
            if let Value::WeakrefDescriptor(owner) = *self.heap.get(descriptor) {
                return Err(self.readonly_weakref(owner));
            }
            if let Value::GetSetDescriptor {
                owner, name, set, ..
            } = *self.heap.get(descriptor)
            {
                if let Some(set) = set {
                    self.call(set, vec![obj, value], Vec::new())?;
                    return Ok(());
                }
                return Err(Error::new(
                    "AttributeError",
                    format!("attribute '{name}' of '{owner}' objects is not writable"),
                ));
            }
            if matches!(self.heap.get(descriptor), Value::SlotMember { .. }) {
                return self.write_slot_member(descriptor, obj, Some(value));
            }
            if matches!(self.heap.get(descriptor), Value::ReadOnlyMember(_, _)) {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            if matches!(self.heap.get(descriptor), Value::TupleGetterDoc) {
                return super::namedtuple::set_document(self, obj, Some(value)).map(|_| ());
            }
            if matches!(self.heap.get(descriptor), Value::TupleGetter { .. }) {
                return Err(Error::new("AttributeError", "can't set attribute"));
            }
            if let Some(setter) = self.slot(descriptor, "__set__") {
                self.call(setter, vec![descriptor, obj, value], Vec::new())?;
                return Ok(());
            }
            return Err(Error::new(
                "AttributeError",
                format!("attribute {name:?} cannot be set"),
            ));
        }
        if self.update_syntax_member(obj, name, value) {
            return Ok(());
        }
        if exception_member.is_some() {
            return self.update_exception_field(obj, name, Some(value));
        }
        if self.update_key_wrapper_field(obj, name, Some(value))?
            || self.update_os_field(obj, name, Some(value))?
        {
            return Ok(());
        }
        if name == "__dict__"
            && matches!(
                self.heap.get(obj),
                Value::Raised(_) | Value::Cached { .. } | Value::Partial { .. }
            )
        {
            if self.dict_storage(value).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__dict__ must be set to a dictionary, not a '{}'",
                        self.type_name(value)
                    ),
                ));
            }
            match self.heap.get_mut(obj) {
                Value::Raised(error) => error.tracing().dictionary = Some(value),
                Value::Partial { dictionary, .. } => {
                    if let Some(old) = dictionary.replace(value)
                        && old != value
                    {
                        self.heap.weak_edge_removed(old);
                    }
                }
                Value::Cached { dictionary, .. } => {
                    let old = std::mem::replace(dictionary, value);
                    if old != value {
                        self.heap.weak_edge_removed(old);
                    }
                }
                _ => unreachable!(),
            }
            return Ok(());
        }
        if name == "name"
            && let Some(error) = self.exception_storage(obj, "NameError")
        {
            error.tracing().name_error_name = Some(value);
            return Ok(());
        }
        if name == "value"
            && let Some(error) = self.exception_storage(obj, "StopIteration")
        {
            error.set_stop_value(value);
            return Ok(());
        }
        if member.is_none()
            && matches!(name, "__name__" | "fget" | "fset" | "fdel")
            && let Some(property) = self.heap.get_mut(obj).property_mut()
        {
            match name {
                "__name__" => {
                    property.name = Some(value);
                    return Ok(());
                }
                "fget" | "fset" | "fdel" => {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                _ => {}
            }
        }
        if name == "msg" && self.import_error_message(obj, Some(value)).is_some() {
            return Ok(());
        }
        if name == "args" && matches!(self.heap.get(obj), Value::Raised(_)) {
            let tuple = if matches!(self.heap.get(value), Value::Tuple(_)) {
                value
            } else {
                let iter = self.iterator(value)?;
                self.heap.keep(iter);
                let mut items = Vec::new();
                while let Some(item) = self.next_item(iter)? {
                    self.heap.keep(item);
                    items.push(item);
                }
                self.alloc(Value::Tuple(items))
            };
            let _ = self.import_error_message(obj, None);
            let Value::Raised(error) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            error.args = Some(tuple);
            return Ok(());
        }
        if let Value::Raised(error) = self.heap.get(obj)
            && !self.native_exception_descriptor(error, name)
        {
            return self.update_exception_field(obj, name, Some(value));
        }
        if member.is_none()
            && self.exposes_defaultdict_factory(obj, name)
            && self.replace_defaultdict_factory(obj, value)
        {
            return Ok(());
        }
        self.check_instance_field(obj, name)?;
        if let Value::Partial { dictionary, .. } = *self.heap.get(obj)
            && name != "__class__"
        {
            if matches!(name, "func" | "args" | "keywords") {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            let dictionary = self.partial_dictionary(obj, dictionary);
            return self.update_dictionary_field(obj, dictionary, name, Some(value));
        }
        if let Value::Cached { dictionary, .. } = *self.heap.get(obj)
            && name != "__class__"
        {
            return self.update_dictionary_field(obj, dictionary, name, Some(value));
        }
        if let Value::Instance {
            class, namespace, ..
        } = *self.heap.get(obj)
        {
            let layout = self.instance_layout(class, name);
            if layout.dictionary && name == "__dict__" && member.is_none() {
                if self.dict_storage(value).is_none() {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            self.type_name(value)
                        ),
                    ));
                }
                self.replace_instance_dictionary(obj, class, Some(value));
                return Ok(());
            }
            if !layout.active_slot {
                let storage = namespace.or_else(|| {
                    (layout.declared_slot || self.has_namedtuple_field(class, name))
                        .then(|| self.instance_dictionary(obj, class))
                });
                if let Some(storage) = storage {
                    return self.update_dictionary_field(obj, storage, name, Some(value));
                }
            }
        }
        let object_class = self.builtins.get("object").copied();
        match self.heap.get_mut(obj) {
            Value::Code(_)
                if matches!(
                    name,
                    "co_name" | "co_qualname" | "co_filename" | "co_firstlineno" | "co_flags"
                ) =>
            {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Property(property) if name == "__doc__" => property.doc = value,
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Module {
                name: module,
                scope,
                constructor,
                ..
            } => {
                if name == "__dict__" {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                if constructor.is_none() && module == "sys" && name == "path" {
                    self.sys_path = value;
                }
                let scope = *scope;
                self.update_module_annotations(scope, name, Some(value))?;
                self.bind(scope, name, value);
            }
            Value::Instance { class, fields, .. } if Some(*class) != object_class => {
                if let Some(previous) = fields.insert(name.to_owned(), value)
                    && previous != value
                {
                    self.heap.weak_edge_removed(previous);
                }
            }
            Value::Class { scope, .. } => {
                if name == "__dict__" {
                    return Err(Error::new(
                        "AttributeError",
                        "attribute '__dict__' of 'type' objects is not writable",
                    ));
                }
                let scope = *scope;
                if matches!(name, "__annotations__" | "__annotate__")
                    && self.has_class_annotation_provider(scope)
                {
                    return self.update_class_annotation_attr(scope, name, Some(value));
                }
                if name == "__annotations__"
                    && let Value::Scope(data) = self.heap.get_mut(scope)
                    && let Some(namespace) = &mut data.class_namespace
                {
                    namespace.annotation_sites = None;
                }
                self.bind(scope, name, value);
                if name == "__del__" {
                    self.register_class_finalizers();
                }
            }
            _ => {
                let attr = self.text(name);
                return Err(Error::new(
                    "AttributeError",
                    format!(
                        "'{}' object has no attribute {} and no __dict__ for setting new attributes",
                        self.type_name(obj),
                        self.repr(attr)
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn instance(&mut self, class: Id, dictionary: Option<Id>) -> Id {
        let property = self.has_base(class, "property").then(Box::default);
        self.alloc(Value::Instance {
            namespace: None,
            class,
            dictionary,
            property,
            weak_reference: None,
            cycle: None,
            member_values: None,
            memory_stream: None,
            fields: Default::default(),
            scalar: None,
        })
    }

    /// The first non-Python base owns the instance layout. Native classes
    /// represented with a class namespace (e.g. BytesIO) have no Python class cell.
    fn allocation_base(&self, class: Id) -> Option<Id> {
        self.find_in_mro(class, |base| self.owns_allocation(base).then_some(base))
    }

    fn owns_allocation(&self, base: Id) -> bool {
        match self.heap.get(base) {
            Value::Native(_) => true,
            // Generic and Protocol inherit object.__new__ without native storage.
            Value::Class { scope, .. } => {
                self.scope_data(*scope).class_cell.is_none()
                    && self.classes.get("typing.Generic") != Some(&base)
                    && self.classes.get("typing.Protocol") != Some(&base)
            }
            _ => false,
        }
    }

    /// Find the native layout owner for the generic allocator exposed by tzinfo.
    /// A Python class follows its most-derived solid base, with ties going to
    /// its first direct base. A tzinfo instance adds no storage over object.
    pub(super) fn tzinfo_allocation_base(
        &self,
        mut class: Id,
        tzinfo: Id,
        object: Id,
    ) -> Option<Id> {
        while !self.owns_allocation(class) {
            let Value::Class { scope, .. } = self.heap.get(class) else {
                return None;
            };
            let namespace = self.scope_data(*scope).class_namespace.as_ref()?;
            let Value::Tuple(bases) = self.heap.get(namespace.bases) else {
                return None;
            };
            let mut primary = *bases.first().unwrap_or(&object);
            let mut solid = object;
            for &base in bases {
                let candidate = self.find_in_mro(base, |owner| {
                    if owner == tzinfo || owner == object {
                        return None;
                    }
                    let slots = matches!(self.heap.get(owner), Value::Class { scope, .. }
                        if self.scope_data(*scope).class_namespace.as_ref()
                            .and_then(|ns| ns.slot_names.as_deref())
                            .is_some_and(|names| names.iter().any(|n| n != "__dict__" && n != "__weakref__")));
                    (self.owns_allocation(owner) || slots).then_some(owner)
                }).unwrap_or(object);
                if candidate != solid && self.direct_subclass(candidate, solid) {
                    primary = base;
                    solid = candidate;
                } else if !self.direct_subclass(solid, candidate) {
                    return None;
                }
            }
            class = primary;
        }
        Some(class)
    }

    /// Allocate a plain instance without invoking its constructor or initializer.
    fn new_object(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&class, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "object.__new__(): not enough arguments",
            ));
        };
        if !matches!(
            self.builtin_type_name(class),
            Some("type" | "typing._AnyMeta")
        ) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "object.__new__(X): X is not a type object ({})",
                    self.type_name(class)
                ),
            ));
        }
        let object = self.builtin("object");
        if let Value::Native(name) = self.heap.get(class)
            && matches!(
                name.as_str(),
                KEY_WRAPPER
                    | value::REGEX_PATTERN_TYPE
                    | value::REGEX_MATCH_TYPE
                    | stdlib::os::scandir::ENTRY
            )
        {
            return Err(Error::new(
                "TypeError",
                format!("cannot create '{name}' instances"),
            ));
        }
        let base = self.allocation_base(class).unwrap_or(class);
        let base = if Some(&base) == self.builtins.get(stdlib::datetime::TZINFO) {
            self.tzinfo_allocation_base(class, base, object)
                .unwrap_or(base)
        } else {
            base
        };
        let new = self.class_attr(class, "__new__");
        if base != object {
            let name = self.class_name(class);
            let owner = if new.is_some_and(|new| !matches!(self.heap.get(new), Value::Native(_))) {
                self.class_name(base)
            } else {
                name
            };
            return Err(Error::new(
                "TypeError",
                format!("object.__new__({name}) is not safe, use {owner}.__new__()"),
            ));
        }
        if !rest.is_empty() || !kwargs.is_empty() {
            if new.is_some_and(|new| Some(&new) != self.builtins.get("object.__new__")) {
                return Err(Error::new(
                    "TypeError",
                    "object.__new__() takes exactly one argument (the type to instantiate)",
                ));
            }
            let init = self.class_attr(class, "__init__");
            if init.is_none_or(|init| Some(&init) == self.builtins.get("object.__init__")) {
                return Err(Error::new(
                    "TypeError",
                    format!("{}() takes no arguments", self.class_name(class)),
                ));
            }
        }
        if let Value::Class { scope, .. } = *self.heap.get(class)
            && self.scope_data(scope).is_abstract
        {
            let name = self.class_name(class).to_owned();
            return Err(self.abstract_class_error(scope, &name)?);
        }
        Ok(self.instance(class, None))
    }

    /// Allocate a binary64 payload while retaining the requested float subclass.
    ///
    /// ```python
    /// class Weight(float):
    ///     pass
    /// assert type(Weight("1.5")) is Weight
    /// ```
    pub(super) fn new_float(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&class, values)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "float.__new__(): not enough arguments",
            ));
        };
        let base = self.builtin("float");
        if !self.subclass(class, base) {
            return Err(Error::new(
                "TypeError",
                "float.__new__ requires a float subtype",
            ));
        }
        let value = builtins::call(self, "float", values, kwargs)?;
        if class == base {
            return Ok(value);
        }
        Ok(self.alloc(Value::Instance {
            namespace: None,
            class,
            fields: Default::default(),
            scalar: Some(value),
            dictionary: None,
            property: None,
            weak_reference: None,
            cycle: None,
            member_values: None,
            memory_stream: None,
        }))
    }

    /// Create empty mapping storage for the requested type without running its initializer.
    fn new_dict(&mut self, args: &[Id]) -> Result<Id> {
        let Some(&class) = args.first() else {
            return Err(Error::new(
                "TypeError",
                "dict.__new__(): not enough arguments",
            ));
        };
        let dict = self.builtin("dict");
        if !self.direct_subclass(class, dict) {
            return Err(self.invalid_new_subtype("dict", class));
        }
        let storage = if self.has_base(class, "collections.defaultdict") {
            self.alloc(Value::DefaultDict {
                factory: NONE,
                entries: Default::default(),
            })
        } else {
            self.dict(Vec::new())
        };
        if matches!(self.heap.get(class), Value::Class { .. }) {
            Ok(self.instance(class, Some(storage)))
        } else {
            Ok(storage)
        }
    }

    /// Describe a receiver rejected by a builtin allocator.
    pub(super) fn invalid_new_subtype(&self, kind: &str, class: Id) -> Error {
        let message = match self.heap.get(class) {
            Value::Class { name, .. } => {
                format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
            }
            Value::Native(name) if builtins::is_builtin_type(name) => {
                format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
            }
            _ => format!(
                "{kind}.__new__(X): X is not a type object ({})",
                self.type_name(class)
            ),
        };
        Error::new("TypeError", message)
    }

    /// Allocate the immutable string or bytes payload while retaining the Python class.
    pub(super) fn new_string(
        &mut self,
        kind: &str,
        class: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let base = self.builtin(kind);
        if !self.subclass(class, base) {
            return Err(self.invalid_new_subtype(kind, class));
        }
        let text = builtins::call(self, kind, args, kwargs)?;
        if class == base {
            return Ok(text);
        }
        let text = self.scalar_value(text);
        Ok(self.alloc(Value::Instance {
            namespace: None,
            class,
            fields: Default::default(),
            dictionary: None,
            scalar: Some(text),
            property: None,
            weak_reference: None,
            cycle: None,
            member_values: None,
            memory_stream: None,
        }))
    }

    /// Count a generated NamedTuple repr frame like a runtime-loaded Python call.
    pub(super) fn check_call_depth(&self, additional: usize) -> Result<()> {
        if self.call_depth + additional >= 1000 {
            return Err(Error::new(
                "RecursionError",
                "maximum recursion depth exceeded",
            ));
        }
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        callable: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        // Binding a descriptor can call Python before it returns its target.
        if matches!(self.heap.get(callable), Value::Instance { .. }) {
            return self.with_slot_depth(
                "maximum recursion depth exceeded while calling a Python object",
                |ctx| ctx.call_instance(callable, args, kwargs),
            );
        }
        if !self.callable(callable) {
            let name = match self.heap.get(callable) {
                Value::Weak(wr) if wr.kind == weakref::Kind::Proxy => "weakref.ProxyType",
                _ => self.type_name(callable),
            };
            return Err(non_callable_error(name));
        }
        let target = match self.heap.get(callable) {
            Value::Native(name) => Value::Native(copy_label(name)?),
            Value::Class {
                name,
                scope,
                ancestors,
                namedtuple_fields,
                metaclass,
                io_kind,
            } => {
                let mut copied_name = String::new();
                copied_name
                    .try_reserve_exact(name.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                copied_name.push_str(name);
                let mut copied_ancestors = Vec::new();
                copied_ancestors
                    .try_reserve_exact(ancestors.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                copied_ancestors.extend_from_slice(ancestors);
                Value::Class {
                    metaclass: *metaclass,
                    io_kind: *io_kind,
                    name: copied_name,
                    scope: *scope,
                    ancestors: copied_ancestors,
                    namedtuple_fields: namedtuple_fields.clone(),
                }
            }
            value => value.clone(),
        };
        match target {
            Value::Weak(wr) => match wr.kind {
                weakref::Kind::Reference => wr.call(&args, &kwargs),
                weakref::Kind::CallableProxy => self.call(wr.upgrade()?, args, kwargs),
                weakref::Kind::Proxy => Err(non_callable_error("weakref.ProxyType")),
            },
            Value::GenericAlias(alias) => {
                if !alias.instantiable {
                    let origin_name = self.lookup_attr(alias.origin, "__name__")?;
                    let origin_name = self.str_value(origin_name)?;
                    let alias_name = alias
                        .name
                        .map(|name| self.str_value(name))
                        .transpose()?
                        .unwrap_or("None");
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "Type {alias_name} cannot be instantiated; use {origin_name}() instead"
                        ),
                    ));
                }
                let result = self.call(alias.origin, args, kwargs)?;
                if matches!(self.heap.get(result), Value::Instance { .. })
                    && let Err(error) = self.set_attr(result, "__orig_class__", callable)
                    && !alias.kind.is_typing_alias()
                    && !matches!(error.kind.as_ref(), "AttributeError" | "TypeError")
                {
                    return Err(error);
                }
                Ok(result)
            }
            Value::Native(name) if name == "function.__call__" => {
                let mut arguments = args.into_iter();
                let function = arguments.next().ok_or_else(|| {
                    Error::new(
                        "TypeError",
                        "descriptor '__call__' of 'function' object needs an argument",
                    )
                })?;
                if !matches!(self.heap.get(function), Value::Function { .. }) {
                    let actual = self
                        .builtin_type_name(function)
                        .unwrap_or_else(|| self.type_name(function));
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "descriptor '__call__' requires a 'function' object but received a '{actual}'"
                        ),
                    ));
                }
                self.call(function, arguments.collect(), kwargs)
            }
            Value::Native(name) if name == "classmethod.__get__" => {
                wrapper_descriptor::descriptor_get(self, "classmethod", &args, &kwargs)
            }
            Value::Native(name) if name == "wrapper_descriptor.__get__" => {
                wrapper_descriptor::descriptor_get(self, "wrapper_descriptor", &args, &kwargs)
            }
            Value::Native(name) if name == "method_descriptor.__get__" => {
                wrapper_descriptor::descriptor_get(self, "method_descriptor", &args, &kwargs)
            }
            Value::Native(name) if name == stdlib::flags::TYPE => Err(Error::new(
                "TypeError",
                "cannot create 'sys.flags' instances",
            )),
            Value::Native(name) if let Some(class) = stdlib::fixed_structseq::fixed_type(&name) => {
                stdlib::fixed_structseq::construct(self, class, &args, &kwargs)
            }
            Value::Native(ref name)
                if let Some((owner, slot)) = name.split_once('.')
                    && DictIterator::methods(owner).contains(&slot)
                    && DictIterator::class(owner).is_some() =>
            {
                self.iterator_method(owner, slot, &args, &kwargs)
            }
            Value::Native(name) if name == "object.__new__" => self.new_object(&args, &kwargs),
            Value::Native(name) if name == "dict.__new__" => self.new_dict(&args),
            Value::Native(name) if name == "int.__new__" => self.new_int(&args, &kwargs),
            Value::Native(name) if name == "tuple.__new__" => {
                if args.first().is_some_and(|&class| matches!(self.heap.get(class), Value::Native(name) if name == stdlib::flags::TYPE)) {
                    return Err(Error::new("TypeError", "cannot create 'sys.flags' instances"));
                }
                super::namedtuple::allocate(self, &args, &kwargs)
            }
            Value::Native(name) if name == "object.__getstate__" => {
                self.object_getstate(&args, &kwargs)
            }
            Value::Native(name)
                if matches!(name.as_str(), "object.__reduce__" | "object.__reduce_ex__") =>
            {
                stdlib::pickle::object_reduce(self, &name["object.".len()..], &args, &kwargs)
            }
            Value::Native(name) if name == "float.__new__" => self.new_float(&args, &kwargs),
            Value::Native(name)
                if builtins::is_builtin(&name)
                    || builtins::constructor_owner(&name).is_some()
                    || matches!(
                        name.as_str(),
                        "BaseException.__init__"
                            | "BaseException.add_note"
                            | "ImportError.__init__"
                            | "OSError.__init__"
                            | "StopIteration.__init__"
                            | "float.__format__"
                            | "float.__getformat__"
                            | "bool.__repr__"
                            | "float.__repr__"
                            | "float.as_integer_ratio"
                    )
                    || !name.contains('.')
                    || matches!(
                        name.split('.').next(),
                        Some(
                            "str"
                                | "enumerate"
                                | "generator"
                                | "coroutine"
                                | "coroutine_wrapper"
                                | "dict"
                                | "list"
                                | "map"
                                | "filter"
                                | "mappingproxy"
                                | "set"
                                | "int"
                                | "frozenset"
                                | "tuple"
                                | "bytes"
                                | "bytearray"
                                | "memoryview"
                                | "range"
                                | "range_iterator"
                                | "longrange_iterator"
                                | "module"
                                | "slice"
                                | "function"
                                | "method"
                                | "object"
                                | "property"
                                | "staticmethod"
                        )
                    ) =>
            {
                builtins::call(self, &name, &args, &kwargs)
            }
            Value::Native(name)
                if matches!(
                    name.as_str(),
                    HashState::CLASS
                        | HashState::XOF_CLASS
                        | KEY_WRAPPER
                        | value::REGEX_PATTERN_TYPE
                        | value::REGEX_MATCH_TYPE
                ) =>
            {
                Err(Error::new(
                    "TypeError",
                    format!("cannot create '{name}' instances"),
                ))
            }
            Value::Native(name) => stdlib::call(self, &name, &args, &kwargs),
            Value::StaticMethod { callable, .. } => self.call(callable, args, kwargs),
            Value::KeyWrapper { comparator, .. } => {
                let object = stdlib::functools::key_argument("K", "obj", &args, &kwargs)?;
                Ok(self.alloc(Value::KeyWrapper {
                    comparator,
                    object: Some(object),
                }))
            }
            Value::CodecDecoder(codec) => stdlib::codecs::decode(self, codec, &args, &kwargs),
            Value::IoMethod { owner, name } => stdlib::io::call(self, owner, name, &args, &kwargs),
            Value::Partial {
                function,
                args: bound,
                keywords,
                ..
            } => {
                let bound = self.items(bound)?;
                let required = bound
                    .iter()
                    .filter(|&&id| matches!(self.heap.get(id), Value::PartialPlaceholder))
                    .count();
                if args.len() < required {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "missing positional arguments in 'partial' call; expected at least {required}, got {}",
                            args.len()
                        ),
                    ));
                }
                let mut supplied = args.into_iter();
                let mut positional = Vec::with_capacity(bound.len() + supplied.len());
                for argument in bound {
                    if matches!(self.heap.get(argument), Value::PartialPlaceholder) {
                        positional.push(supplied.next().unwrap());
                    } else {
                        positional.push(argument);
                    }
                }
                positional.extend(supplied);
                let mut merged = self.keyword_args(keywords)?;
                for (key, value) in kwargs {
                    if let Some((_, previous)) = merged.iter_mut().find(|(k, _)| k == &key) {
                        *previous = value;
                    } else {
                        merged.push((key, value));
                    }
                }
                self.call(function, positional, merged)
            }
            Value::Method {
                callable, receiver, ..
            } => {
                let mut all = Vec::new();
                all.try_reserve_exact(args.len() + 1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                all.push(receiver);
                all.extend(args);
                self.call(callable, all, kwargs)
            }
            Value::AttrGetter(paths) => self.apply_attrgetter(&paths, &args, &kwargs),
            Value::ItemGetter(keys) => self.apply_itemgetter(keys, &args, &kwargs),
            Value::CacheDecorator { maxsize, typed } => {
                let function = stdlib::single_argument(
                    "lru_cache.<locals>.decorating_function",
                    "user_function",
                    &args,
                    &kwargs,
                )?;
                stdlib::functools::wrap_cache(self, function, maxsize, typed)
            }
            Value::CacheParameters { maxsize, typed } => {
                if !args.is_empty() || !kwargs.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        "cache_parameters() takes no arguments",
                    ));
                }
                let size_key = self.text("maxsize");
                let typed_key = self.text("typed");
                Ok(self.dict(vec![(size_key, maxsize), (typed_key, typed)]))
            }
            Value::Cached {
                function,
                table,
                marker,
                capacity,
                typed,
                ..
            } => {
                let mark = self.heap.temporary_mark();
                let result = (|| -> Result<Id> {
                    self.heap.keep(callable);
                    self.heap.keep(table);
                    if capacity == Some(0) {
                        let Value::Cached { misses, .. } = self.heap.get_mut(callable) else {
                            unreachable!()
                        };
                        *misses += 1;
                        return self.call(function, args, kwargs);
                    }
                    let fast = !typed
                        && args.len() == 1
                        && kwargs.is_empty()
                        && matches!(
                            self.heap.get(args[0]),
                            Value::Int(_) | Value::Str(_) | Value::SurrogateStr(_)
                        );
                    let mut parts = args.clone();
                    if !kwargs.is_empty() {
                        parts.push(marker);
                        for (name, value) in &kwargs {
                            let name = self.text(name.clone());
                            self.heap.keep(name);
                            parts.push(name);
                            parts.push(*value);
                        }
                    }
                    for &part in &parts {
                        self.heap.keep(part);
                    }
                    if typed {
                        for &value in args.iter().chain(kwargs.iter().map(|(_, value)| value)) {
                            let class = self.exact_type(value)?;
                            self.heap.keep(class);
                            parts.push(class);
                        }
                    }
                    let key = if fast {
                        args[0]
                    } else {
                        self.alloc(Value::Tuple(parts))
                    };
                    self.heap.keep(key);
                    let hash = self.dictionary_hash(key)?;
                    if let Some(index) = self.find_dictionary_key(table, key, Some(hash))?.1 {
                        let (resident, value) = self.dict_entries(table).unwrap()[index];
                        let Value::Cached { entries, hits, .. } = self.heap.get_mut(callable)
                        else {
                            unreachable!()
                        };
                        *hits += 1;
                        if capacity.is_some()
                            && let Some(i) = entries.iter().position(|call| call.key == resident)
                        {
                            let hit = entries.remove(i);
                            entries.push(hit);
                        }
                        return Ok(value);
                    }
                    let Value::Cached { misses, .. } = self.heap.get_mut(callable) else {
                        unreachable!()
                    };
                    *misses += 1;
                    let result = self.call(function, args, kwargs)?;
                    self.heap.keep(result);
                    let Some(capacity) = capacity else {
                        self.set_cache_key(table, key, result, hash)?;
                        return Ok(result);
                    };
                    if self
                        .find_dictionary_key(table, key, Some(hash))?
                        .1
                        .is_some()
                    {
                        return Ok(result);
                    }
                    let evicted = self.evict_cached_call(callable, table, capacity)?;
                    if let Some(old) = evicted {
                        self.heap.keep(old.key);
                        if let Some(i) = self.find_dictionary_key(table, old.key, Some(old.hash))?.1
                        {
                            let Value::Dict(entries) = self.heap.get_mut(table) else {
                                unreachable!()
                            };
                            let (key, value) = entries.remove(i)?;
                            self.heap.weak_edge_removed(key);
                            self.heap.weak_edge_removed(value);
                        }
                    }
                    let resident = self.set_cache_key(table, key, result, hash)?;
                    {
                        let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                            unreachable!()
                        };
                        if let Some(i) = entries.iter().position(|entry| entry.key == resident) {
                            entries.remove(i);
                        }
                    }
                    loop {
                        let old = self.evict_cached_call(callable, table, capacity)?;
                        let Some(old) = old else { break };
                        self.heap.keep(old.key);
                        if let Some(i) = self.find_dictionary_key(table, old.key, Some(old.hash))?.1
                        {
                            let Value::Dict(entries) = self.heap.get_mut(table) else {
                                unreachable!()
                            };
                            let (key, value) = entries.remove(i)?;
                            self.heap.weak_edge_removed(key);
                            self.heap.weak_edge_removed(value);
                        }
                    }
                    let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                        unreachable!()
                    };
                    entries.push(value::CachedCall {
                        key: resident,
                        hash,
                    });
                    Ok(result)
                })();
                self.heap.release_temporaries_since(mark);
                result
            }
            Value::Class {
                name,
                scope,
                ancestors,
                metaclass,
                ..
            } => {
                if self.builtins.get("abc.ABCMeta") == Some(&callable) {
                    return Err(Error::new(
                        "NotImplementedError",
                        "direct ABCMeta construction is unsupported",
                    ));
                }
                if let Some(meta) = metaclass
                    && let Some(slot) = self.class_attr(meta, "__call__")
                {
                    return self.with_slot_depth(
                        "maximum recursion depth exceeded while calling a Python object",
                        |ctx| {
                            let call = ctx.bind_attribute(slot, callable, meta)?;
                            ctx.call(call, args, kwargs)
                        },
                    );
                }
                let (mut args, mut kwargs) = (args, kwargs);
                if let Some(&decimal) = self.classes.get("decimal.Decimal")
                    && (callable == decimal
                        || (ancestors.contains(&decimal)
                            && self.class_attr(callable, "__new__").is_none()
                            && self.class_attr(callable, "__init__")
                                == self.class_attr(decimal, "__init__")))
                {
                    let value = match (&mut args[..], &mut kwargs[..]) {
                        ([value], []) => Some(value),
                        ([], [(key, value)]) if key == "value" => Some(value),
                        _ => None,
                    };
                    if let Some(value) = value {
                        if let Value::Instance { class, .. } = self.heap.get(*value)
                            && self.direct_subclass(*class, decimal)
                        {
                            if *class == decimal && callable == decimal {
                                return Ok(*value);
                            }
                            if let Some(text) = self.raw_instance_field(*value, "_text") {
                                *value = text;
                            }
                        }
                        if let Some(text) = self.string_storage(*value) {
                            *value = text;
                        } else if matches!(
                            self.heap.get(self.scalar_value(*value)),
                            Value::Int(_) | Value::Bool(_)
                        ) {
                            let text =
                                crate::integer_text::try_decimal_coefficient(&self.as_int(*value)?)
                                    .ok_or_else(crate::Error::memory_error)?;
                            *value = self.text(text.into_string());
                        }
                    }
                }
                if let Some(values) = self.enum_values(callable) {
                    return self.enum_lookup(callable, values, &args, &kwargs);
                }
                if self.classes.get("typing.Union") == Some(&callable) {
                    return Err(Error::new(
                        "TypeError",
                        "cannot create 'typing.Union' instances",
                    ));
                }
                if self.scope_data(scope).is_abstract {
                    return Err(self.abstract_class_error(scope, &name)?);
                }
                if self.is_protocol_declaration(callable)
                    && self.class_attr(callable, "__init__").is_none()
                {
                    return Err(Error::new("TypeError", "Protocols cannot be instantiated"));
                }
                if self.classes.get("collections.abc.Callable") == Some(&callable) {
                    return Err(Error::new(
                        "TypeError",
                        "Can't instantiate abstract class Callable without an implementation for abstract method '__call__'",
                    ));
                }
                if self.classes.get("typing.Generic") == Some(&callable) {
                    return Err(Error::new(
                        "TypeError",
                        "Can't instantiate typing.Generic directly",
                    ));
                }
                let constructor = self.class_attr(callable, "__new__");
                let generic_tzinfo = self.builtins.get("datetime.tzinfo.__new__").copied();
                let native_tzinfo_new = constructor.is_some()
                    && constructor == generic_tzinfo
                    && self.find_in_mro(callable, |base| {
                        self.own_class_attr(base, "__new__").map(|_| base)
                    }) == self.builtins.get(stdlib::datetime::TZINFO).copied();
                // A subclass calling through the inherited generic constructor
                // uses the primary base's allocator. Later Python mixins do
                // not get a second chance to supply __new__.
                let new = if native_tzinfo_new {
                    let tzinfo = stdlib::datetime::tzinfo(self);
                    let object = self.builtin("object");
                    let owner = self
                        .tzinfo_allocation_base(callable, tzinfo, object)
                        .ok_or_else(|| {
                            Error::new("TypeError", "incompatible tzinfo instance layout")
                        })?;
                    if owner == tzinfo {
                        None
                    } else {
                        self.class_attr(owner, "__new__")
                    }
                } else {
                    constructor
                }
                .or_else(|| {
                    [("int", "int.__new__"), ("tuple", "tuple.__new__")]
                        .into_iter()
                        .find(|(base, _)| self.has_base(callable, base))
                        .map(|(_, new)| self.builtin(new))
                });
                let string_base = ["str", "bytes"]
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
                let float_subclass = self.has_base(callable, "float");
                let rlock_subclass = self.has_base(callable, super::thread_rlock::CLASS);
                let collection_kind = ["set", "frozenset", "bytearray"]
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
                let instance = if let Some(new) = new {
                    let allocate = self.bind_attribute(new, NONE, callable)?;
                    let positional = std::iter::once(callable)
                        .chain(args.iter().copied())
                        .collect();
                    let instance = self.call(allocate, positional, kwargs.clone())?;
                    if !self.instance_of(instance, callable) {
                        return Ok(instance);
                    }
                    instance
                } else if self.has_base(callable, "type") {
                    return Err(Error::new(
                        "NotImplementedError",
                        "direct metaclass construction is unsupported",
                    ));
                } else if self.has_base(callable, "module") {
                    self.allocate_module(callable)
                } else if let Some(kind) = string_base {
                    self.new_string(kind, callable, &args, &kwargs)?
                } else if float_subclass {
                    let positional = std::iter::once(callable)
                        .chain(args.iter().copied())
                        .collect::<Vec<_>>();
                    self.new_float(&positional, &kwargs)?
                } else {
                    let storage = if rlock_subclass {
                        Some(self.alloc(Value::HandlerLock(Default::default())))
                    } else if self.has_base(callable, "list") {
                        Some(self.alloc(Value::List(Vec::new())))
                    } else if let Some(kind) = collection_kind {
                        let custom_init = self.class_attr(callable, "__init__").is_some();
                        let items = if kind != "frozenset" && custom_init {
                            &[][..]
                        } else {
                            &args
                        };
                        let keywords = if custom_init { &[][..] } else { &kwargs };
                        Some(builtins::call(self, kind, items, keywords)?)
                    } else {
                        None
                    };
                    let object = self.instance(callable, None);
                    if let Value::Instance { scalar, .. } = self.heap.get_mut(object) {
                        *scalar = storage;
                    }
                    object
                };
                let actual = self.exact_type(instance)?;
                if let Some(init) = self.class_attr(actual, "__init__") {
                    let init = self.bind_attribute(init, instance, actual)?;
                    let result = self.call(init, args, kwargs)?;
                    if result != NONE {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "__init__() should return None, not '{}'",
                                self.type_name(result)
                            ),
                        ));
                    }
                } else if new.is_none()
                    && !native_tzinfo_new
                    && string_base.is_none()
                    && !rlock_subclass
                    && !float_subclass
                    && collection_kind.is_none()
                    && (!args.is_empty() || !kwargs.is_empty())
                {
                    return Err(Error::new(
                        "TypeError",
                        format!("{name} takes no arguments"),
                    ));
                }
                Ok(instance)
            }
            Value::Function {
                fields,
                scope,
                class_cell,
                code,
                defaults,
                kwdefaults,
                ..
            } => {
                let parameters = self.code_object(code).parameters;
                let name = copy_label(self.str_value(fields.qualname)?)?;
                if parameters == NONE {
                    if let Some((key, _)) = kwargs.first() {
                        return Err(Error::new(
                            "TypeError",
                            format!("{name}() got an unexpected keyword argument '{key}'"),
                        ));
                    }
                    if !args.is_empty() {
                        let verb = if args.len() == 1 { "was" } else { "were" };
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "{name}() takes 0 positional arguments but {} {verb} given",
                                args.len()
                            ),
                        ));
                    }
                    let globals = self.module_globals(scope)?;
                    return self.exec_source(code, globals, None, Some(callable));
                }
                // Root the scope, packed arguments, keyword names, and returning value.
                self.heap
                    .reserve_temporaries(kwargs.len().saturating_add(4))?;
                let module = copy_label(&self.scope_data(scope).module)?;
                let closure = allocator_api2::boxed::Box::try_new(value::ClosureScope::default())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                let local = self.scope_with_closure(module, Some(scope), closure);
                let sig: serde_json::Value = serde_json::from_str(self.str_value(parameters)?)
                    .map_err(|e| Error::new("SystemError", e.to_string()))?;
                let names = |key: &str| -> Result<Vec<String>> {
                    let mut names = Vec::new();
                    for name in sig[key].as_array().into_iter().flatten() {
                        if let Some(name) = name.as_str() {
                            names
                                .try_reserve(1)
                                .map_err(|_| Error::new("MemoryError", ""))?;
                            names.push(copy_label(name)?);
                        }
                    }
                    Ok(names)
                };
                let locals = names("scope_locals")?;
                let nonlocals = names("scope_nonlocals")?;
                let Value::Scope(frame) = self.heap.get_mut(local) else {
                    unreachable!()
                };
                frame
                    .closure
                    .declared
                    .try_reserve(locals.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                frame
                    .closure
                    .nonlocals
                    .try_reserve(nonlocals.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                frame.closure.declared.extend(locals);
                frame.closure.declared_complete =
                    sig["scope_locals_complete"].as_bool().unwrap_or(false);
                frame.closure.nonlocals.extend(nonlocals);
                let posonly = names("posonly")?;
                let positional = names("positional")?;
                let mut pos = Vec::new();
                let count = posonly
                    .len()
                    .checked_add(positional.len())
                    .ok_or_else(|| Error::new("MemoryError", ""))?;
                pos.try_reserve_exact(count)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                for name in &posonly {
                    pos.push(copy_label(name)?);
                }
                pos.extend(positional);
                let kwonly = names("kwonly")?;
                for (n, &v) in pos.iter().zip(&args) {
                    self.bind_argument(local, n, v)?;
                }
                let given = args.len();
                if let Some(n) = sig["varargs"].as_str() {
                    let extra = if pos.is_empty() {
                        args
                    } else {
                        copy_ids(args.get(pos.len()..).unwrap_or_default())?
                    };
                    let extra = self.alloc(Value::Tuple(extra));
                    self.bind_argument(local, n, extra)?;
                }
                let mut extra = Vec::new();
                for (n, v) in kwargs {
                    if !posonly.contains(&n) && (pos.contains(&n) || kwonly.contains(&n)) {
                        if self.scope_data(local).names.contains_key(&n) {
                            return Err(Error::new(
                                "TypeError",
                                format!("{name}() got multiple values for argument '{n}'"),
                            ));
                        }
                        self.bind_argument(local, &n, v)?;
                    } else if sig["kwargs"].is_string() {
                        let k = self.text(n);
                        extra.push((k, v));
                    } else {
                        let candidates: Vec<_> = pos[posonly.len()..]
                            .iter()
                            .chain(&kwonly)
                            .map(String::as_str)
                            .collect();
                        let mut message =
                            format!("{name}() got an unexpected keyword argument '{n}'");
                        if let Some(hint) =
                            crate::objects::exceptions::keyword_suggestion(&n, &candidates)
                        {
                            message.push_str(&format!(". Did you mean '{hint}'?"));
                        }
                        return Err(Error::new("TypeError", message));
                    }
                }
                if let Some(n) = sig["kwargs"].as_str() {
                    let v = self.dict(extra);
                    self.bind_argument(local, n, v)?;
                }
                let default_values = if defaults == NONE {
                    Vec::new()
                } else {
                    self.items(defaults)?
                };
                if given > pos.len() && !sig["varargs"].is_string() {
                    let bound = &self.scope_data(local).names;
                    let keywords = kwonly.iter().filter(|n| bound.contains_key(*n)).count();
                    return Err(excess_positional_error(
                        &name,
                        pos.len(),
                        default_values.len(),
                        given,
                        keywords,
                    ));
                }
                let mut missing = Vec::new();
                for (index, n) in pos.iter().enumerate() {
                    if !self.scope_data(local).names.contains_key(n) {
                        let offset = index + default_values.len();
                        if let Some(&val) = offset
                            .checked_sub(pos.len())
                            .and_then(|i| default_values.get(i))
                        {
                            self.bind_argument(local, n, val)?;
                        } else {
                            missing
                                .try_reserve(1)
                                .map_err(|_| Error::new("MemoryError", ""))?;
                            missing.push(n.as_str());
                        }
                    }
                }
                if !missing.is_empty() {
                    return Err(missing_arguments_error(&name, "positional", &missing));
                }
                for n in &kwonly {
                    if !self.scope_data(local).names.contains_key(n) {
                        let val = self.dict_entries(kwdefaults).and_then(|entries| {
                            entries.iter().find_map(|&(key, value)| {
                                matches!(self.heap.get(key), Value::Str(text) if text == n).then_some(value)
                            })
                        });
                        if let Some(val) = val {
                            self.bind_argument(local, n, val)?;
                        } else {
                            missing
                                .try_reserve(1)
                                .map_err(|_| Error::new("MemoryError", ""))?;
                            missing.push(n.as_str());
                        }
                    }
                }
                if !missing.is_empty() {
                    return Err(missing_arguments_error(&name, "keyword-only", &missing));
                }
                if let Some(types) = sig["types"].as_object() {
                    for (param, expected) in types {
                        if let Some(&id) = self.scope_data(local).names.get(param) {
                            self.require_scalar(
                                id,
                                expected.as_str().unwrap_or_default(),
                                &format!("{name} argument {param:?}"),
                            )?;
                        }
                    }
                }
                let Value::Scope(frame) = self.heap.get_mut(local) else {
                    unreachable!()
                };
                frame.first_argument = pos.into_iter().next();
                if sig["class_cell"] == true {
                    frame.class_cell = class_cell;
                }
                self.check_call_depth(0)?;
                self.call_depth += 1;
                let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    self.execute(code, local, FrameOwner::Function(callable))
                });
                self.call_depth -= 1;
                let value = result?;
                if let Some(expected) = sig["return_type"].as_str() {
                    self.require_scalar(value, expected, &format!("{name} return value"))?;
                }
                Ok(value)
            }
            _ => Err(non_callable_error(self.type_name(callable))),
        }
    }

    /// Guard nested native slots that can recurse without entering a Python frame.
    pub(super) fn with_slot_depth<T>(
        &mut self,
        message: &'static str,
        operation: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if stacker::remaining_stack()
            .map_or(self.slot_depth >= 128, |remaining| remaining < 64 * 1024)
        {
            return Err(Error::new("RecursionError", message));
        }
        self.slot_depth += 1;
        let result = operation(self);
        self.slot_depth -= 1;
        result
    }

    fn call_instance(
        &mut self,
        mut callable: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        // A finite delegation chain only needs one Rust frame per batch. The
        // next call checks the available stack again, including on cyclic chains.
        for _ in 0..32 {
            let Some(next) = self.bound_slot(callable, "__call__")? else {
                return Err(non_callable_error(self.type_name(callable)));
            };
            callable = next;
            if !matches!(self.heap.get(callable), Value::Instance { .. }) {
                break;
            }
        }
        self.call(callable, args, kwargs)
    }

    fn require_scalar(&self, value: Id, expected: &str, context: &str) -> Result<()> {
        if matches!(
            (expected, self.heap.get(value)),
            ("None", Value::None)
                | ("int", Value::Int(_))
                | ("float", Value::Float(_))
                | ("bytes", Value::Bytes(_))
                | ("str", Value::Str(_) | Value::SurrogateStr(_))
                | ("bool", Value::Bool(_))
        ) {
            Ok(())
        } else {
            Err(Error::new(
                "TypeError",
                format!("{context} requires {expected}"),
            ))
        }
    }

    /// Initialize descriptor names in class body insertion order, before subclass hooks run.
    pub(super) fn initialize_class_members(&mut self, class: Id, scope: Id) -> Result<()> {
        let Value::Scope(class_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        let members = class_scope
            .class_body
            .take()
            .unwrap()
            .into_iter()
            .map(|name| (class_scope.names[&name], name))
            .collect::<Vec<_>>();
        if let Some(dict) = class_scope.class_namespace.as_ref().and_then(|namespace| {
            namespace.annotation_dict_cell.and(namespace.dictionary)
        }) {
            class_scope.namespace = Some(dict);
            class_scope.names.retain(|name, _| name == "__qualname__");
        }
        // A hook may replace an attribute and execute code that triggers collection.
        for &(value, _) in &members {
            self.heap.keep(value);
        }
        for (value, name) in members {
            if matches!(self.heap.get(value), Value::Property(_)) {
                let name = self.text(name);
                let Value::Property(property) = self.heap.get_mut(value) else {
                    unreachable!()
                };
                property.name = Some(name);
            } else if let Some(set_name) = self.slot(value, "__set_name__") {
                let name = self.text(name);
                self.call(set_name, vec![value, class, name], Vec::new())?;
            }
        }
        Ok(())
    }

    /// Select the most derived compatible metaclass before executing a boxed class body.
    fn select_class_metaclass(
        &mut self,
        bases: &[Id],
        explicit: Option<Id>,
        typed_dict_meta: Option<Id>,
        enum_base: bool,
    ) -> Result<Option<Id>> {
        if explicit.is_none()
            && !bases.iter().any(|&base| {
                self.is_bundled_abc(base)
                    || matches!(self.heap.get(base), Value::Class { metaclass: Some(meta), .. }
                        if Some(*meta) != typed_dict_meta)
            })
        {
            return Ok(typed_dict_meta);
        }
        let type_class = self.builtin("type");
        let mut winner = explicit.unwrap_or(type_class);
        if !self.direct_subclass(winner, type_class) {
            return Err(Error::new(
                "NotImplementedError",
                "non-type metaclasses are unsupported",
            ));
        }
        let conflict = || {
            Error::new(
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            )
        };
        for &base in bases {
            let meta = if self.is_bundled_abc(base) {
                self.abc_meta()
            } else {
                self.exact_type(base)?
            };
            if !self.direct_subclass(winner, meta) {
                if !self.direct_subclass(meta, winner) {
                    return Err(conflict());
                }
                winner = meta;
            }
        }
        if winner == type_class {
            return Ok(None);
        }
        if enum_base {
            return Err(conflict());
        }
        let is_abc = self.builtins.get("abc.ABCMeta") == Some(&winner);
        if self
            .classes
            .get("typing.Protocol")
            .is_some_and(|&protocol| {
                bases
                    .iter()
                    .any(|&base| self.direct_subclass(base, protocol))
            })
        {
            if !is_abc {
                return Err(conflict());
            }
            if explicit == Some(winner) {
                return Err(Error::new(
                    "NotImplementedError",
                    "the Protocol metaclass is unsupported with ABCMeta",
                ));
            }
            // Protocol uses a synthetic layout and wins over an inherited ABCMeta.
            return Ok(None);
        }
        if let Some(meta) = self.module_value("typing", "_TypedDictMeta")
            && self.direct_subclass(winner, meta)
            && Some(winner) != typed_dict_meta
        {
            return Err(Error::new(
                "NotImplementedError",
                "custom TypedDict metaclass construction is unsupported",
            ));
        }
        // User metaclasses can customize their instances' calls and attributes while
        // inheriting type's normal namespace preparation, allocation, and MRO.
        if !is_abc
            && Some(winner) != typed_dict_meta
            && (["__prepare__", "__new__", "__init__", "mro"]
                .iter()
                .any(|name| self.class_attr(winner, name).is_some())
                || ["__call__", "__getattr__", "__getattribute__"]
                    .iter()
                    .any(|name| self.slot(winner, name).is_some()))
        {
            return Err(Error::new(
                "NotImplementedError",
                "custom metaclass construction is unsupported",
            ));
        }
        Ok(Some(winner))
    }

    pub(super) fn class(
        &mut self,
        bases: Id,
        body: Id,
        mut keywords: Vec<(String, Id)>,
        parameterized: bool,
    ) -> Result<Id> {
        let (original, mut bases) = super::typing::resolve_bases(self, bases)?;
        if self
            .builtins
            .get("abc.ABCMeta")
            .is_some_and(|meta| bases.contains(meta))
        {
            return Err(Error::new(
                "NotImplementedError",
                "subclassing ABCMeta is unsupported",
            ));
        }
        let typed_dict_meta = self
            .module_value("typing", "_TypedDictMeta")
            .filter(|meta| {
                bases
                    .iter()
                    .any(|&base| self.heap.get(base).instance_class() == Some(*meta))
            });
        let enum_namespace = if typed_dict_meta.is_some() {
            None
        } else {
            self.enum_namespace(&bases)
        };
        let explicit_metaclass = keywords
            .iter()
            .position(|(name, _)| name == "metaclass")
            .map(|index| keywords.remove(index).1);
        if enum_namespace.is_some()
            && keywords
                .iter()
                .any(|(name, _)| matches!(name.as_str(), "boundary" | "_simple"))
        {
            return Err(Error::new(
                "NotImplementedError",
                "boxed enum metaclass options are unsupported",
            ));
        }
        let class_meta = self.select_class_metaclass(
            &bases,
            explicit_metaclass,
            typed_dict_meta,
            enum_namespace.is_some(),
        )?;
        let Value::Function {
            scope: parent,
            code,
            ..
        } = self.heap.get(body).clone()
        else {
            return Err(Error::new("TypeError", "invalid class body"));
        };
        let name = self.str_value(self.code_object(code).name)?.to_owned();
        #[derive(serde::Deserialize)]
        struct ClassBindings {
            #[serde(default)]
            scope_nonlocals: std::collections::HashSet<String>,
        }
        let parameters = self.code_object(code).parameters;
        let bindings: ClassBindings = serde_json::from_str(self.str_value(parameters)?)
            .map_err(|e| Error::new("SystemError", e.to_string()))?;
        let module = self.scope_data(parent).module.clone();
        let scope = self.scope(&module, Some(parent));
        let enum_kind = enum_namespace.as_ref().map(|state| state.kind);
        let cell = self.alloc(Value::Cell(None));
        let Value::Scope(body_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        body_scope.class_body = Some(Vec::new());
        body_scope.closure.nonlocals = bindings.scope_nonlocals;
        body_scope.enum_namespace = enum_namespace;
        body_scope.class_cell = Some(cell);
        let mut globals = parent;
        while let Some(outer) = self.scope_data(globals).parent {
            globals = outer;
        }
        let module_name = if let Some(exec_globals) = self.scope_data(globals).exec_globals {
            self.source_global(exec_globals, "__name__")?
        } else {
            self.scope_name(globals, "__name__")
                .unwrap_or_else(|| self.text("builtins"))
        };
        self.bind(scope, "__module__", module_name);
        if parameterized {
            let params = self
                .scope_name(parent, ".type_params")
                .expect("class parameter scope");
            self.bind(scope, "__type_params__", params);
        }
        if let Ok(qualname) = self.lookup_attr(body, "__qualname__") {
            self.bind(scope, "__qualname__", qualname);
        }
        let annotations = self.dict(Vec::new());
        self.bind_local(scope, "__annotations__", annotations);
        if let Value::Scope(frame) = self.heap.get_mut(scope) {
            frame.class_namespace.get_or_insert_with(Default::default).annotation_scratch = Some(annotations);
        }
        self.execute(code, scope, FrameOwner::Function(body))?;
        let explicit_annotations = self.scope_name(scope, "__annotations__")
            .is_some_and(|value| value != annotations);
        let annotation_body = self.finish_annotation_body(scope)?;
        if let Value::Scope(frame) = self.heap.get_mut(scope) {
            if let Some(namespace) = &mut frame.class_namespace {
                namespace.annotation_scratch = None;
                if explicit_annotations {
                    namespace.annotation_sites = None;
                }
            }
            frame.globals.clear();
            frame.closure.nonlocals.clear();
        }
        if self.scope_name(scope, "__qualname__").is_none() {
            let qualname = self.text(&name);
            self.bind_local(scope, "__qualname__", qualname);
        }
        if self.scope_name(scope, "__module__").is_none() {
            let namespace = self.module_globals(parent)?;
            if let Some(name) = self.dict_named_item(namespace, "__name__") {
                self.bind_local(scope, "__module__", name);
            }
        }
        // Inheriting a protocol creates a concrete class unless Protocol is a direct base again.
        if let Some(&protocol) = self.classes.get("typing.Protocol")
            && bases.iter().any(|&base| self.subclass(base, protocol))
        {
            if self.scope_name(scope, "__subclasshook__").is_none()
                && let Some(hook) = self.module_value("typing", "_proto_hook")
            {
                self.bind_local(scope, "__subclasshook__", hook);
            }
            let declaration = bases.contains(&protocol);
            if declaration {
                self.validate_protocol_bases(&bases)?;
            }
            self.bind(
                scope,
                "_is_protocol",
                if declaration { TRUE } else { FALSE },
            );
        }
        let typed_dict_fields = if let Some(meta) = typed_dict_meta {
            let mut total = TRUE;
            for (name, value) in keywords.drain(..) {
                if name == "total" {
                    total = value;
                } else {
                    let reason = if matches!(name.as_str(), "cls" | "name" | "bases" | "ns") {
                        "got multiple values for argument"
                    } else {
                        "got an unexpected keyword argument"
                    };
                    return Err(Error::new(
                        "TypeError",
                        format!("_TypedDictMeta.__new__() {reason} '{name}'"),
                    ));
                }
            }
            let generic = self.classes.get("typing.Generic").copied();
            if bases.iter().any(|&base| {
                self.heap.get(base).instance_class() != Some(meta) && generic != Some(base)
            }) {
                return Err(Error::new(
                    "TypeError",
                    "cannot inherit from both a TypedDict type and a non-TypedDict base class",
                ));
            }
            let dict = self.builtin("dict");
            let generic = generic.filter(|&g| bases.iter().any(|&base| self.subclass(base, g)));
            let parents = std::mem::take(&mut bases);
            bases.extend(generic);
            bases.push(dict);
            let parents = self.alloc(Value::Tuple(parents));
            let (fields, annotate) = self.prepare_typeddict_namespace(scope, annotations)?;
            Some((parents, fields, annotate, generic, total))
        } else {
            None
        };
        if typed_dict_meta.is_none() && self.scope_name(scope, "__annotate_func__").is_some() {
            self.enable_class_annotation_provider(scope);
            if self.scope_name(scope, "__annotations__") == Some(annotations) {
                let key = self.heap.code_name("__annotations__");
                self.remove_binding(scope, "__annotations__", key)?;
            }
        }
        if let Some(original) = original {
            self.bind(scope, "__orig_bases__", original);
        }
        if typed_dict_meta.is_none() {
            super::typing::class_parameters(self, scope, original, &bases)?;
        }
        let class = self.allocate_class(name.clone(), scope, bases)?;
        let Value::Class { metaclass, .. } = self.heap.get_mut(class) else {
            unreachable!()
        };
        *metaclass = class_meta;
        self.install_weakref_descriptor(class, scope);
        self.heap.replace_cell(cell, Some(class));
        let Value::Scope(class_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        // The class body can read the enclosing module's docstring before the
        // type supplies a default for an undocumented class.
        let has_doc_slot = class_scope
            .class_namespace
            .as_deref()
            .and_then(|namespace| namespace.slot_names.as_deref())
            .is_some_and(|slots| slots.iter().any(|slot| slot == "__doc__"));
        if !has_doc_slot {
            class_scope
                .names
                .entry("__doc__".to_owned())
                .or_insert(NONE);
        }
        if let Some((cell, body)) = annotation_body {
            // type.__new__ replaces the body namespace with the finished type's dictionary.
            let dict = self.class_namespace_dict(scope, false);
            let extras = self
                .dict_entries(body)
                .expect("class body dictionary")
                .iter()
                .copied()
                .filter(|&(key, _)| self.string_storage(key).is_none())
                .collect::<Vec<_>>();
            for (key, value) in extras {
                self.put_item(dict, key, value)?;
            }
            self.heap.replace_cell(cell, Some(dict));
        }
        self.initialize_class_members(class, scope)?;
        if let Some(kind) = enum_kind {
            let scalar = kind.scalar_base().map(|base| self.builtin(base));
            self.populate_enum(class, scalar)?;
        }
        // Look up the inherited hook after __set_name__, before publishing the class.
        let hook = self.super_attribute(class, class, class, "__init_subclass__")?;
        self.call(hook, Vec::new(), keywords)?;
        if module == "_collections_abc"
            && matches!(
                name.as_str(),
                "Hashable" | "Awaitable" | "AsyncIterable" | "Buffer"
            )
        {
            self.classes
                .insert(format!("collections.abc.{name}"), class);
        }
        self.init_abstract_class(class, scope)?;
        if self.is_protocol_declaration(class) {
            self.call_typing_helper("_freeze_protocol_members", vec![class, annotations])?;
        }
        if let Some((parents, fields, annotate, generic, total)) = typed_dict_fields {
            if let Some(generic) = generic {
                // Generic.__init_subclass__ reads the declared bases after __set_name__.
                let raw = self
                    .scope_name(scope, "__orig_bases__")
                    .map(|tuple| self.items(tuple))
                    .transpose()?
                    .unwrap_or_default();
                super::typing::bind_generic_parameters(self, scope, generic, &raw)?;
            }
            let configure = self
                .module_value("typing", "_configure_typeddict")
                .ok_or_else(|| Error::new("RuntimeError", "missing TypedDict initializer"))?;
            self.call(
                configure,
                vec![class, fields, total, annotate, parents],
                Vec::new(),
            )?;
        }
        self.classes.insert(name, class);
        Ok(class)
    }
}
