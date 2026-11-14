use super::attribute_name::AttributeName;
use crate::objects::HashState;

use super::stdlib::functools::LRU_CACHE_CLASS;
use super::wrapper_descriptor::BuiltinDescriptor;
use super::*;

/// Written operands still need Python argument binding. Selected slots are in formal order.
pub(super) enum FunctionArguments {
    Written(Vec<Id>, Vec<(String, Id)>, Option<Id>),
    Bound(Vec<Id>),
}

const KEY_WRAPPER: &str = "functools.KeyWrapper";

/// These native result types inherit object.__new__ but reject direct allocation.
fn nonconstructible_result_type(name: &str) -> bool {
    matches!(
        name,
        value::ZLIB_COMPRESS_TYPE
            | HashState::CLASS
            | HashState::XOF_CLASS
            | HashState::OPENSSL_CLASS
            | KEY_WRAPPER
            | "getset_descriptor"
            | "set_iterator"
            | value::ZLIB_DECOMPRESS_TYPE
            | "anext_awaitable"
            | "_buffer_wrapper"
            | value::REGEX_PATTERN_TYPE
            | value::REGEX_MATCH_TYPE
            | value::REGEX_SCANNER_TYPE
            | value::STRUCT_UNPACK_ITERATOR
    )
}

#[derive(Clone, Copy)]
pub(super) enum MappingOperation {
    Copy,
    Update,
    Unpack,
    Keywords(Option<Id>),
}

#[derive(Clone, Copy)]
pub(super) enum ObjectAttribute {
    Get,
    Edit(Option<Id>),
}

enum ScalarAnnotation {
    Value(Id),
    Missing,
    Unresolved,
}

#[derive(PartialEq, Eq)]
enum AttributeLookup<'a> {
    Value,
    Method(Option<&'a mut Id>),
    ListedClassMember,
}

#[derive(Default)]
struct InstanceLayout {
    dictionary: bool,
    materialize_dictionary_on_write: bool,
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
        "operator" if stdlib::operator::Operation::named(function).is_some() => {
            Some(("_operator", function))
        }
        "unicodedata" if matches!(function, "category" | "bidirectional") => {
            Some((module, function))
        }
        "time" if stdlib::time::is_function(function) => Some((module, function)),
        "sys"
            if matches!(
                function,
                "_get_cpu_count_config"
                    | "get_coroutine_origin_tracking_depth"
                    | "set_coroutine_origin_tracking_depth"
            ) =>
        {
            Some((module, function))
        }
        "_socket" if stdlib::SOCKET_FUNCTIONS.contains(&function) => Some((module, function)),
        "_typing" if function == "_idfunc" => Some((module, function)),
        "encodings" if function == "search_function" => Some((module, function)),
        "posix"
            if !function.contains('.')
                && !matches!(
                    symbol,
                    stdlib::os::scandir::ENTRY | stdlib::os::scandir::ITER
                )
                && stdlib::fixed_structseq::fixed_type(symbol).is_none() =>
        {
            Some((module, function))
        }
        "codecs"
            if matches!(
                function,
                "encode" | "decode" | "register" | "unregister" | "lookup" | "escape_decode"
            ) || stdlib::codecs::WideFunction::named(function).is_some() =>
        {
            Some(("_codecs", function))
        }
        "_thread" if matches!(function, "allocate" | "allocate_lock") => Some((module, function)),
        #[cfg(unix)]
        "_signal" if matches!(function, "sigpending" | "valid_signals") => Some((module, function)),
        #[cfg(unix)]
        "_signal"
            if matches!(
                function,
                "default_int_handler"
                    | "getsignal"
                    | "pause"
                    | "set_wakeup_fd"
                    | "siginterrupt"
                    | "signal"
            ) =>
        {
            Some((module, function))
        }
        "_contextvars" if function == "copy_context" => Some((module, function)),
        "_io" if function == "text_encoding" => Some((module, function)),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "grp" if matches!(function, "getgrall" | "getgrgid" | "getgrnam") => {
            Some((module, function))
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "pwd" if matches!(function, "getpwall" | "getpwnam" | "getpwuid") => {
            Some((module, function))
        }
        #[cfg(target_os = "linux")]
        "resource" if matches!(function, "getrlimit" | "setrlimit") => Some((module, function)),
        "gc" if matches!(
            function,
            "enable" | "disable" | "isenabled" | "get_threshold" | "set_threshold" | "is_tracked"
        ) =>
        {
            Some((module, function))
        }
        "binascii"
            if matches!(
                function,
                "a2b_qp" | "b2a_qp" | "crc32" | "crc_hqx" | "b2a_hex" | "hexlify"
            ) =>
        {
            Some((module, function))
        }
        "zlib" if function == "compressobj" => Some((module, function)),
        "_lzma"
            if matches!(
                function,
                "is_check_supported" | "_encode_filter_properties" | "_decode_filter_properties"
            ) =>
        {
            Some((module, function))
        }
        "lzma" if function == "decompress" => Some((module, function)),
        "zlib"
            if matches!(
                function,
                "adler32" | "crc32" | "compress" | "decompress" | "decompressobj"
            ) =>
        {
            Some((module, function))
        }
        #[cfg(unix)]
        "_imre_ssl" if !function.contains('.') => Some((module, function)),
        "_hashlib" if !function.contains('.') => Some((module, function)),
        "hashlib"
            if function == "new"
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

pub(super) fn unexpected_keyword_error(name: &str, key: &str, params: &[&str]) -> Error {
    Error::new(
        "TypeError",
        crate::source_arguments::unexpected_keyword_message(name, key, params),
    )
}
pub(super) fn excess_positional_error(
    name: &str,
    allowed: usize,
    defaults: usize,
    given: usize,
    keywords: usize,
) -> Error {
    Error::new(
        "TypeError",
        crate::source_arguments::excess_positional_message(
            name, allowed, defaults, given, keywords,
        ),
    )
}
pub(super) fn missing_arguments_error(name: &str, kind: &str, missing: &[&str]) -> Error {
    Error::new(
        "TypeError",
        crate::source_arguments::missing_arguments_message(name, kind, missing),
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

fn method_arguments(receiver: Id, mut args: Vec<Id>) -> Result<Vec<Id>> {
    args.try_reserve_exact(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    args.insert(0, receiver);
    Ok(args)
}

pub(super) enum RegisteredCall {
    Returned(Id),
    Native(RegisteredFrame),
    /// The entered frame roots bare code and its namespace through the callback.
    Code {
        entry: Code,
        module: bool,
        discard: bool,
    },
    Start(Box<thread_worker::Start>),
    Join(thread_worker::Join),
    Acquire(thread_lock::Acquire),
    /// Condition.notify has already selected the waiter's bound release method.
    Release(Arc<crate::objects::locks::PrimitiveLock>),
    Restore(thread_rlock::Restore),
    Recv(stdlib::socket::Recv),
}

impl RegisteredCall {
    /// # Safety
    /// A prepared frame belongs to the exclusively executing context that pushed it.
    #[inline]
    pub(super) unsafe fn run(self, context: *mut Context) -> Result<Id> {
        match self {
            Self::Returned(value) => Ok(value),
            // SAFETY: the owner keeps the context and its linked code live through the call.
            Self::Native(frame) => unsafe { frame.run(context) },
            Self::Code {
                entry,
                module,
                discard,
            } => {
                // SAFETY: the prepared code frame is active in its owning context.
                let result = unsafe { Context::run_code_frame(context, entry, module) }?;
                Ok(if discard { NONE } else { result })
            }
            // SAFETY: the interrupted caller remains rooted in this exclusively executing context.
            Self::Join(join) => unsafe { join.run(context) },
            // SAFETY: the native worker takes no reference into its creator's heap.
            Self::Start(start) => unsafe { start.run(context) },
            // SAFETY: the caller owns the context and its interrupted frame on this thread.
            Self::Acquire(wait) => unsafe { wait.run_registered(context) },
            Self::Release(lock) => {
                lock.release()?;
                Ok(NONE)
            }
            Self::Restore(wait) => Ok(wait.run()),
            // SAFETY: the caller roots the frame and owns the context on this thread.
            Self::Recv(recv) => unsafe { recv.run(context) },
        }
    }
}

struct FunctionReturn {
    callable: Id,
    declared: ScalarHint,
    name: crate::Str,
}

impl Context {
    fn finish_function(&mut self, check: Option<FunctionReturn>, result: Result<Id>) -> Result<Id> {
        self.call_depth -= 1;
        let value = result?;
        if let Some(check) = check {
            self.require_scalar(
                check.callable,
                "return",
                value,
                (check.declared.0, None),
                false,
                format_args!("{} return value", check.name),
            )?;
        }
        Ok(value)
    }
}

/// The pushed frame owns the function and local scope until completion.
pub(super) struct RegisteredFrame {
    entry: Code,
    scope: Id,
    returning: Option<FunctionReturn>,
}

impl RegisteredFrame {
    /// # Safety
    /// `context` must point to the exclusively executing Context which prepared this frame.
    /// It must stay in place on this thread until the linked entry and finalization return.
    pub(super) unsafe fn run(self, context: *mut Context) -> Result<Id> {
        stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
            // SAFETY: the caller owns this execution and keeps the linked entry loaded.
            let value = unsafe { (self.entry)(context, self.scope) };
            // SAFETY: the entry has returned, leaving the live Context exclusively with the caller.
            let ctx = unsafe { &mut *context };
            let result = ctx.finish_native_frame(value);
            ctx.finish_function(self.returning, result)
        })
    }
}

impl Context {
    pub(super) fn class_attr(&self, class: Id, name: &str) -> Option<Id> {
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
            && matches!(name, "__reduce__" | "__reduce_ex__" | "__getstate__")
            && let Value::Native(kind) = self.heap.get(class)
            && matches!(
                kind.as_str(),
                stdlib::datetime::TZINFO
                    | super::thread_handle::CLASS
                    | "datetime.timezone"
                    | value::ZSTD_COMPRESS_TYPE
                    | value::ZSTD_DECOMPRESS_TYPE
                    | value::BZ2_COMPRESS_TYPE
                    | value::BZ2_DECOMPRESS_TYPE
                    | value::LZMA_COMPRESS_TYPE
                    | value::LZMA_DECOMPRESS_TYPE
            )
        {
            let parent = if kind == "datetime.timezone" {
                stdlib::datetime::TZINFO
            } else {
                "object"
            };
            return self
                .builtins
                .get(parent)
                .and_then(|&base| self.class_attr(base, name));
        }
        if attr.is_none() && name == "__sizeof__" {
            let owner = match self.heap.get(class) {
                Value::Native(kind) if kind == "bool" => "int",
                Value::Native(kind) if kind == defaultdict::CLASS => "dict",
                _ => "object",
            };
            return self.builtins.get(object_size::descriptor(owner)?).copied();
        }
        if attr.is_none()
            && name == "__reduce_ex__"
            && self
                .find_in_mro(class, |base| self.own_class_attr(base, "__reduce__"))
                .is_some()
        {
            // Exact native classes have no stored MRO but inherit object's delegating reducer.
            return self.builtins.get("object.__reduce_ex__").copied();
        }
        attr
    }

    /// Look up an attribute in the actual type dictionaries. Python comparisons may replace
    /// a class's MRO or its members, so keep the original bases live for this lookup.
    fn class_attr_with_name(&mut self, class: Id, name: AttributeName<'_>) -> Option<Id> {
        let Some(key) = name.nonexact_key(self) else {
            return self.class_attr(class, &name.text(self).ok()?);
        };
        self.class_dictionary_member(class, key)
            .map(|(_, _, value)| value)
    }

    /// Return the declaring class, stored key and member selected by one type lookup.
    fn class_dictionary_member(&mut self, class: Id, key: Id) -> Option<(Id, Id, Id)> {
        // CPython clears hash and equality exceptions from type lookup. Instance dictionary
        // operations hash the key independently and propagate their own failures.
        let hash = self.dictionary_hash(key).ok()?;
        let mut bases = Vec::new();
        self.find_in_mro(class, |base| {
            bases.push(base);
            None::<()>
        });
        let object = self.builtin("object");
        if !bases.contains(&object) {
            bases.push(object);
        }
        for &base in &bases {
            self.heap.keep(base);
        }
        for base in bases {
            let dictionary = if let Value::Class { scope, .. } = *self.heap.get(base) {
                self.class_namespace_dict(scope, false)
            } else if let Some(&dictionary) = self.native_type_dicts.get(&base) {
                dictionary
            } else if let Some(&scope) = self.enum_scopes.get(&base) {
                self.class_namespace_dict(scope, false)
            } else {
                // Some native classes construct their mapping proxy on first access.
                let Ok(proxy) = self.default_get_attr(base, "__dict__") else {
                    continue;
                };
                let Value::MappingProxy(dictionary) = *self.heap.get(proxy) else {
                    continue;
                };
                dictionary
            };
            if let (_, Some(index)) = self.find_dictionary_key(dictionary, key, Some(hash)).ok()? {
                let (stored, member) = self.stored_entries(dictionary)[index];
                return Some((base, stored, member));
            }
        }
        None
    }

    /// Visit the class and its bases in Python's method resolution order.
    pub(super) fn find_in_mro<T>(
        &self,
        class: Id,
        mut lookup: impl FnMut(Id) -> Option<T>,
    ) -> Option<T> {
        if let Some(mro) = self.published_mro(class) {
            return mro.iter().copied().find_map(lookup);
        }
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return lookup(class).or_else(|| {
                if let Value::Native(kind) = self.heap.get(class)
                    && builtins::is_enum_base(kind)
                {
                    let scalar = match kind.as_str() {
                        "enum.IntEnum" | "enum.IntFlag" => Some("int"),
                        "enum.StrEnum" => Some("str"),
                        _ => None,
                    };
                    return scalar
                        .into_iter()
                        .chain((kind == "enum.IntFlag").then_some("enum.Flag"))
                        .chain((kind != "enum.Enum").then_some("enum.Enum"))
                        .chain(["object"])
                        .filter_map(|name| self.builtins.get(name))
                        .find_map(|&base| lookup(base));
                }
                if let Value::Native(name) = self.heap.get(class)
                    && let Some(kind) = builtins::exception_kind(name)
                {
                    return builtins::exception_ancestors(kind)
                        .filter_map(|name| self.builtins.get(name))
                        .find_map(|&base| lookup(base));
                }
                let base = match self.heap.get(class) {
                    Value::Native(name) if name == "datetime.datetime" => "datetime.date",
                    _ => return None,
                };
                self.builtins.get(base).and_then(|&base| lookup(base))
            });
        };
        std::iter::once(class)
            .chain(ancestors.iter().copied())
            .find_map(lookup)
    }

    pub(super) fn own_class_attr(&self, class: Id, name: &str) -> Option<Id> {
        if let Value::Class { scope, .. } = self.heap.get(class) {
            return self.namespace_binding(self.scope_data(*scope), name);
        }
        if let Some(&dictionary) = self.native_type_dicts.get(&class)
            && let Some(value) = self.dict_named_item(dictionary, name)
        {
            return Some(value);
        }
        match self.heap.get(class) {
            Value::Native(kind) if name == "__sizeof__" => {
                self.builtins.get(object_size::descriptor(kind)?).copied()
            }
            // The native generic alias inherits the same subclass hook as source special forms.
            Value::Native(kind)
                if kind == "typing._GenericAlias" && name == "__init_subclass__" =>
            {
                self.classes
                    .get("typing._Final")
                    .and_then(|&base| self.own_class_attr(base, name))
            }
            Value::Native(kind) if kind == "type" && name == "__text_signature__" => {
                self.builtins.get("type.__text_signature__").copied()
            }
            Value::Native(kind) if kind == "type" && name == "__call__" => {
                self.builtins.get("type.__call__").copied()
            }
            Value::Native(kind) if kind == "type" && name == "__prepare__" => {
                self.builtins.get("type.__prepare__").copied()
            }
            Value::Native(kind) if kind == "type" && name == "__subclasses__" => {
                self.builtins.get("type.__subclasses__").copied()
            }
            Value::Native(kind) if kind == "type" && name == "mro" => {
                self.builtins.get("type.mro").copied()
            }
            Value::Native(kind) if kind == "object" => match name {
                "__hash__" => self.builtins.get("object.__hash__").copied(),
                "__ne__" => self.builtins.get("object.__ne__").copied(),
                "__format__" => self.builtins.get("object.__format__").copied(),
                "__getstate__" => self.builtins.get("object.__getstate__").copied(),
                "__reduce__" => self.builtins.get("object.__reduce__").copied(),
                "__reduce_ex__" => self.builtins.get("object.__reduce_ex__").copied(),
                "__init_subclass__" => self.builtins.get("object.__init_subclass__").copied(),
                _ => None,
            },
            Value::Native(owner)
                if builtins::set_methods(owner).contains(&name)
                    || builtins::set_operators(owner).contains(&name) =>
            {
                self.builtins.get(&format!("{owner}.{name}")).copied()
            }
            Value::Native(kind) if kind == "collections.defaultdict" => {
                let owner = if super::defaultdict::METHODS.contains(&name) {
                    kind.as_str()
                } else {
                    "dict"
                };
                self.builtins.get(&format!("{owner}.{name}")).copied()
            }
            Value::Native(_) if let Some(&scope) = self.enum_scopes.get(&class) => {
                self.namespace_binding(self.scope_data(scope), name)
            }
            Value::Native(kind)
                if name == "__hash__"
                    && matches!(
                        kind.as_str(),
                        "list" | "dict" | "set" | "bytearray" | "array.array" | KEY_WRAPPER
                    ) =>
            {
                Some(NONE)
            }
            Value::Native(kind)
                if matches!(
                    kind.as_str(),
                    "dict"
                        | "list"
                        | "tuple"
                        | "bool"
                        | "int"
                        | "float"
                        | "complex"
                        | "bytes"
                        | "bytearray"
                        | "memoryview"
                        | "str"
                        | "property"
                        | "module"
                        | super::thread_local::CLASS
                        | weakref::PROXY_TYPE
                        | weakref::CALLABLE_PROXY_TYPE
                        | weakref::REFERENCE_TYPE
                ) || (kind == super::deque::CLASS && super::deque::owns(name))
                    || (kind == stdlib::fixed_structseq::SCHED_PARAM
                        && matches!(name, "__reduce__" | "__replace__"))
                    || (kind == "function" && name == "__call__")
                    || (matches!(
                        kind.as_str(),
                        "builtin_function_or_method" | "method" | "method-wrapper"
                    ) && name == "__reduce__") =>
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

    /// Replace the SystemExit member and schedule collection of a released weak target.
    pub(super) fn replace_exit_code(&mut self, object: Id, value: Id) -> bool {
        let Some(error) = self.exception_storage(object, "SystemExit") else {
            return false;
        };
        let old = std::mem::replace(&mut error.exit_code, value);
        if old != value {
            self.heap.weak_edge_removed(old);
        }
        true
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

    /// Read constructor arguments without consulting a subclass's Python attributes.
    pub(super) fn exception_reduction_arguments(&mut self, object: Id) -> Result<Id> {
        if let Value::Raised(error) = self.heap.get(object)
            && let Some(args) = error.args
        {
            return Ok(args);
        }
        if let Some(args) = self.os_attribute(object, "args")? {
            return Ok(args);
        }
        let Value::Raised(error) = self.heap.get(object) else {
            unreachable!("validated BaseException receiver");
        };
        Ok(self.exception_argument_tuple(object, error.clone()))
    }

    /// The reducer includes an allocated dictionary even when it is empty.
    pub(super) fn exception_reduction_state(&self, object: Id) -> Result<Option<Id>> {
        let Value::Raised(error) = self.heap.get(object) else {
            unreachable!("validated BaseException receiver");
        };
        // Raised instances currently store Python slot values in the same dictionary.
        if error.class().is_some_and(|class| {
            self.find_in_mro(class, |base| self.declares_named_slots(base).then_some(()))
                .is_some()
        }) {
            return Err(Error::new(
                "NotImplementedError",
                "native pickle cannot separate exception slots from instance state",
            ));
        }
        Ok(error.trace.as_ref().and_then(|trace| trace.dictionary))
    }

    fn reduce_exception(&mut self, object: Id, owner: &str) -> Result<Id> {
        let mut state = self.exception_reduction_state(object)?;
        if owner == "ImportError"
            && super::exception_members::import_names(self, object)
                .iter()
                .any(Option::is_some)
        {
            let dict = if let Some(dict) = state {
                let storage = self.dict_storage(dict).expect("exception dictionary");
                builtins::copy_dict(self, dict, storage)?
            } else {
                self.dict(Vec::new())
            };
            self.heap.keep(dict);
            // Inserting a key can run Python that mutates the remaining slots.
            for (index, key) in builtins::IMPORT_KEYWORDS.iter().enumerate() {
                if let Some(value) = super::exception_members::import_names(self, object)[index] {
                    let key = self.text(*key);
                    self.set_dictionary_key(dict, key, value, None, true)?;
                }
            }
            state = Some(dict);
        }
        let class = self.exact_type(object)?;
        let args = self.exception_reduction_arguments(object)?;
        let mut fields = vec![class, args];
        fields.extend(state);
        Ok(self.alloc(Value::Tuple(fields)))
    }

    /// Restore state in insertion order, allowing each attribute setter to mutate the dictionary.
    fn set_exception_state(&mut self, object: Id, state: Id) -> Result<()> {
        if state == NONE {
            return Ok(());
        }
        let storage = self
            .dict_storage(state)
            .ok_or_else(|| Error::new("TypeError", "state is not a dictionary"))?;
        let mut cursor = 0;
        while let Some((pos, key, value)) = self.stored_entries(storage).at(cursor) {
            cursor = pos + 1;
            let name = builtins::attribute_name(self, key)?;
            let mark = self.heap.temporary_mark();
            self.heap.keep(key);
            self.heap.keep(value);
            let result =
                self.set_named_attr(object, AttributeName::from_lookup(&name, Some(key)), value);
            self.heap.release_temporaries_since(mark);
            result?;
        }
        Ok(())
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

    fn replace_partial_dictionary(&mut self, obj: Id, value: Id) {
        let Value::Partial { dictionary, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        if let Some(old) = dictionary.replace(value)
            && old != value
        {
            self.heap.weak_edge_removed(old);
        }
    }

    /// Instance attributes use raw dict storage even when __dict__ was set to a subclass.
    fn dictionary_field(&mut self, dictionary: Option<Id>, name: &str) -> Result<Option<Id>> {
        let Some(dictionary) = dictionary else {
            return Ok(None);
        };
        let key = self.text(name);
        self.dictionary_field_with_key(dictionary, key)
    }

    fn dictionary_field_with_key(&mut self, dictionary: Id, key: Id) -> Result<Option<Id>> {
        let storage = self.dict_storage(dictionary).expect("instance dictionary");
        let (_, index) = self.find_dictionary_key(storage, key, None)?;
        Ok(index.map(|i| self.stored_entries(storage)[i].1))
    }

    fn update_exception_field(&mut self, obj: Id, name: &str, value: Option<Id>) -> Result<()> {
        // Even a failed instance-dictionary deletion creates the dictionary seen by __reduce__.
        let dictionary = self.exception_dictionary(obj);
        self.update_dictionary_field(obj, dictionary, name, value)
    }

    fn instance_namespace_for_name(&mut self, obj: Id, class: Id, writing: bool) -> Option<Id> {
        match self.heap.get(obj) {
            Value::Raised(error) if !writing => error.trace.as_ref().and_then(|t| t.dictionary),
            Value::Raised(_) => Some(self.exception_dictionary(obj)),
            Value::Instance {
                namespace: Some(dict),
                ..
            } => Some(*dict),
            Value::Instance { .. } => {
                let layout = self.instance_layout(class, "");
                (layout.dictionary && (writing || !layout.materialize_dictionary_on_write))
                    .then(|| self.instance_dictionary(obj, class))
            }
            _ => None,
        }
    }

    /// Resolve the descriptor before consulting instance storage. Both steps use the
    /// same Python name; the MRO and dictionary probes can run different callbacks.
    pub(super) fn default_object_attribute(
        &mut self,
        obj: Id,
        name: AttributeName<'_>,
        op: ObjectAttribute,
    ) -> Result<Id> {
        let text = name.text(self)?;
        let original = name.original();
        let ordinary = matches!(
            self.heap.get(obj),
            Value::Instance { .. } | Value::Raised(_)
        );
        // Compiled literal names use the existing inline-field and native-type fast paths.
        // A Python name must run the same descriptor operation even when its spelling was supplied.
        if !ordinary && matches!(op, ObjectAttribute::Get) {
            return self.default_attr(obj, name, AttributeLookup::Value);
        }
        if !ordinary || original.is_none() {
            return self.default_text_attribute(obj, &text, op);
        }
        let class = self.exact_type(obj)?;
        let member = self.class_attr_with_name(class, name);
        // Exact strings with no published member retain the existing specialized builtins
        // and the inline namespace used by ordinary source assignments.
        if name.nonexact_key(self).is_none() && (member.is_none() || text == "__qualname__") {
            return self.default_text_attribute(obj, &text, op);
        }
        let key = original.expect("original Python name");
        if let Some(descriptor) = member {
            if matches!(op, ObjectAttribute::Get) && self.data_descriptor(descriptor) {
                return self.read_data_descriptor(descriptor, obj, class, |ctx| {
                    match ctx.instance_namespace_for_name(obj, class, false) {
                        Some(dict) => ctx.dictionary_field_with_key(dict, key),
                        None => Ok(None),
                    }
                });
            }
            self.heap.keep(descriptor);
            if let ObjectAttribute::Edit(value) = op
                && self.data_descriptor(descriptor)
            {
                self.edit_data_descriptor(descriptor, obj, value)?;
                return Ok(NONE);
            }
        }
        let dictionary =
            self.instance_namespace_for_name(obj, class, matches!(op, ObjectAttribute::Edit(_)));
        if let Some(dictionary) = dictionary {
            match op {
                ObjectAttribute::Get => {
                    if let Some(field) = self.dictionary_field_with_key(dictionary, key)? {
                        return Ok(field);
                    }
                }
                ObjectAttribute::Edit(value) => {
                    let storage = self.dict_storage(dictionary).expect("instance dictionary");
                    let result = if let Some(value) = value {
                        self.put_item(storage, key, value)
                    } else {
                        self.delete_item(storage, key)
                    };
                    if let Err(error) = result {
                        return Err(if self.is_builtin_error(&error, "KeyError") {
                            self.missing_attribute(obj, &text)
                        } else {
                            error
                        });
                    }
                    if value.is_none() {
                        self.finish_binding_release();
                    }
                    return Ok(NONE);
                }
            }
        }
        if matches!(op, ObjectAttribute::Get)
            && let Some(member) = member
        {
            return self.bind_attribute(member, obj, class);
        }
        Err(self.missing_attribute(obj, &text))
    }

    fn default_text_attribute(&mut self, obj: Id, text: &str, op: ObjectAttribute) -> Result<Id> {
        match op {
            ObjectAttribute::Get => self.default_get_attr(obj, text),
            ObjectAttribute::Edit(Some(value)) => {
                self.default_set_attr(obj, text, value).map(|_| NONE)
            }
            ObjectAttribute::Edit(None) => self.default_delete_attr(obj, text).map(|_| NONE),
        }
    }

    /// Assign or delete a local naming attribute without changing the generic alias's origin.
    fn update_alias_name(&mut self, obj: Id, name: &str, value: Option<Id>) -> Option<Result<()>> {
        let Value::GenericAlias(alias) = self.heap.get(obj) else {
            return None;
        };
        let fields = alias.naming_fields?;
        if !matches!(name, "__name__" | "__qualname__" | "__module__") {
            return None;
        }
        Some(self.update_dictionary_field(obj, fields, name, value))
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

    /// Bind declared native data descriptors as well as members supplied by a Python subclass.
    fn exception_member(&mut self, object: Id, name: &str) -> Option<Id> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        let user_class_or_group = error.class().is_some() || error.group().is_some();
        let class = self.exact_type(object).ok()?;
        self.instance_class_member(class, name)
            .filter(|&member| user_class_or_group || self.native_data_descriptor(member))
    }

    /// Bind an override only if it precedes the built-in numeric method in the MRO.
    pub(super) fn bound_numeric_slot(&mut self, value: Id, slot: &str) -> Result<Option<Id>> {
        let class = match *self.heap.get(value) {
            Value::Instance { class, .. } => class,
            Value::Raised(_) => self.exact_type(value)?,
            _ => return Ok(None),
        };
        self.numeric_slot_override(class, slot)
            .map(|method| self.bind_attribute(method, value, class))
            .transpose()
    }

    /// Look up a numeric override up to the first built-in implementation.
    pub(super) fn numeric_slot_override(&self, class: Id, slot: &str) -> Option<Id> {
        self.find_in_mro(class, |base| match self.heap.get(base) {
            Value::Native(name) if matches!(name.as_str(), "int" | "float") => Some(None),
            _ => self.own_class_attr(base, slot).map(Some),
        })
        .flatten()
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

    /// Mixed positional stars expand immediately; a sole star is consumed by the call.
    pub(super) fn expand_positional_items(
        &mut self,
        args: &mut Vec<Id>,
        iterable: Id,
        sole_star_callable: Option<Id>,
    ) -> Result<()> {
        let iter = match self.iterator(iterable) {
            Err(error)
                if error.kind == "TypeError"
                    && self.slot(iterable, "__iter__").is_none()
                    && self.slot(iterable, "__getitem__").is_none() =>
            {
                let prefix = if let Some(callable) = sole_star_callable {
                    format!(
                        "{} argument after *",
                        self.keyword_callable(Some(callable))?
                    )
                } else {
                    "Value after *".into()
                };
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{prefix} must be an iterable, not {}",
                        self.type_name(iterable)
                    ),
                ));
            }
            other => other?,
        };
        self.heap.keep(iter);
        if sole_star_callable.is_none() {
            let default = self.int(8);
            let hint = self.length_hint(iterable, Some(default))?;
            let hint = self.as_integer(hint)? as usize;
            if hint <= isize::MAX as usize - args.len() {
                args.try_reserve(hint)
                    .map_err(|_| Error::new("MemoryError", ""))?;
            }
        }
        while let Some(item) = self.next_item(iter)? {
            args.try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            // Further arguments may clear the iterable and invoke Python callbacks.
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(item);
            args.push(item);
        }
        Ok(())
    }

    /// CPython reports the qualified callable in errors while unpacking its keywords.
    pub(super) fn keyword_callable(&mut self, callable: Option<Id>) -> Result<String> {
        let Some(callable) = callable else {
            return Ok("__build_class__()".into());
        };
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

    /// Merge mapping entries, rejecting duplicate keys when constructing call arguments.
    pub(super) fn merge_mapping(
        &mut self,
        target: Id,
        input: Id,
        operation: MappingOperation,
    ) -> Result<Id> {
        // A fresh keyword dictionary can copy a dense table of exact names in one pass.
        if matches!(operation, MappingOperation::Keywords(_))
            && matches!(self.heap.get(target), Value::Dict(entries) if entries.is_empty())
            && let Value::Dict(entries) = self.heap.get(input)
            && entries
                .iter()
                .all(|&(key, _)| matches!(self.heap.get(key), Value::Str(_)))
            && let Some(copy) = entries.clone_dense()?
        {
            *self.heap.get_mut(target) = Value::Dict(copy);
            return Ok(target);
        }
        let mapping =
            self.for_each_mapping_item(input, target, operation, |ctx, key, value, hash| {
                ctx.set_dictionary_key(target, key, value, hash, true)
                    .map(|_| ())
            })?;
        if !mapping {
            let message = match operation {
                MappingOperation::Keywords(callable) => format!(
                    "{} argument after ** must be a mapping, not {}",
                    self.keyword_callable(callable)?,
                    self.type_name(input)
                ),
                _ => "expected a mapping".into(),
            };
            return Err(Error::new("TypeError", message));
        }
        Ok(target)
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
                | Value::SetIterator(_)
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
        if matches!(self.heap.get(object), Value::AsyncGenerator { .. })
            && matches!(name, "__aiter__" | "__anext__")
        {
            return self.lookup_attr(object, name).map(Some);
        }
        if name == "__repr__"
            && matches!(self.heap.get(object), Value::Weak(wr) if wr.kind == weakref::Kind::Reference)
        {
            return self.lookup_attr(object, name).map(Some);
        }
        let class = match self.heap.get(object) {
            Value::SetIterator(_) if name == "__length_hint__" => self.builtin("set_iterator"),
            Value::Weak(wr)
                if name == "__repr__"
                    || (wr.kind == weakref::Kind::Proxy
                        && weakref::PROXY_METHODS.contains(&name)) =>
            {
                self.builtin(wr.kind.class())
            }
            value => match value.instance_class() {
                Some(class) => class,
                None => return Ok(None),
            },
        };
        self.class_attr(class, name)
            .map(|slot| self.bind_attribute(slot, object, class))
            .transpose()
    }

    pub(super) fn callable(&self, value: Id) -> bool {
        if matches!(self.heap.get(value), Value::ConstEvaluator(_)) {
            return true;
        }
        matches!(
            self.heap.get(value),
            Value::Class { .. }
                | Value::GenericAlias(_)
                | Value::Function { .. }
                | Value::FractionHash(_)
                | Value::AttrGetter(_)
                | Value::ItemGetter(_)
                | Value::MethodCaller { .. }
                | Value::StaticMethod { .. }
                | Value::KeyWrapper { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::Native(_)
                | Value::CodecEncoder(_)
                | Value::CodecDecoder(_)
                | Value::IoMethod { .. }
                | Value::IoOpen { .. }
                | Value::HeapTypeMethod { .. }
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
                | Value::LayoutDescriptor(..)
        )
    }

    fn data_descriptor(&self, descriptor: Id) -> bool {
        self.native_data_descriptor(descriptor)
            || self.slot(descriptor, "__set__").is_some()
            || self.slot(descriptor, "__delete__").is_some()
    }

    fn edit_data_descriptor(&mut self, descriptor: Id, obj: Id, value: Option<Id>) -> Result<()> {
        match *self.heap.get(descriptor) {
            Value::SlotMember { .. } => self.write_slot_member(descriptor, obj, value),
            Value::LayoutDescriptor(owner, field) => {
                self.write_layout_descriptor(owner, field, obj, value)
            }
            Value::ReadOnlyMember(owner, field) => self.reject_member_write(owner, field, obj),
            Value::Property(property) => match value {
                Some(value) => property.write(self, obj, value),
                None => property.remove(self, obj),
            },
            Value::GetSetDescriptor { set: Some(set), .. } => {
                let mut args = vec![obj];
                args.extend(value);
                self.call(set, args, Vec::new()).map(|_| ())
            }
            Value::GetSetDescriptor { owner, name, .. } => {
                Err(self.readonly_getset_error(owner, name, obj))
            }
            Value::TupleGetterDoc => super::namedtuple::set_document(self, obj, value).map(|_| ()),
            Value::TupleGetter { .. } => Err(Error::new(
                "AttributeError",
                if value.is_some() {
                    "can't set attribute"
                } else {
                    "can't delete attribute"
                },
            )),
            _ => {
                let slot = if value.is_some() {
                    "__set__"
                } else {
                    "__delete__"
                };
                let method = self
                    .bound_slot(descriptor, slot)?
                    .ok_or_else(|| Error::new("AttributeError", slot))?;
                let mut args = vec![obj];
                args.extend(value);
                self.call(method, args, Vec::new()).map(|_| ())
            }
        }
    }

    fn readable_data_descriptor(&self, member: Id) -> bool {
        self.native_data_descriptor(member)
            || (self.slot(member, "__get__").is_some() && self.data_descriptor(member))
    }

    /// A descriptor with only a setter or deleter yields to an instance field. Keep the
    /// selected descriptor on a miss even if comparing dictionary keys replaces its class entry.
    fn read_data_descriptor(
        &mut self,
        descriptor: Id,
        object: Id,
        class: Id,
        field: impl FnOnce(&mut Self) -> Result<Option<Id>>,
    ) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(descriptor);
        let result = if self.readable_data_descriptor(descriptor) {
            self.bind_attribute(descriptor, object, class)
        } else {
            field(self).map(|value| value.unwrap_or(descriptor))
        };
        self.release_with_result(mark, result)
    }

    /// Bind a Python method to the receiver selected by descriptor lookup.
    pub(super) fn bind_python_method(&mut self, callable: Id, receiver: Id) -> Id {
        self.alloc(Value::Method {
            callable,
            receiver,
            explicit: true,
        })
    }

    /// Bind a native class method to the class through which it was retrieved.
    fn bind_native_classmethod(&mut self, callable: Id, owner: Id) -> Id {
        self.alloc(Value::Method {
            callable,
            receiver: owner,
            explicit: false,
        })
    }

    /// Validate a descriptor against its own generation of a native heap type.
    pub(super) fn check_heap_method_receiver(
        &self,
        owner: Id,
        name: &str,
        receiver: Id,
    ) -> Result<()> {
        let declaring = self
            .heap
            .get(owner)
            .heap_type()
            .expect("native heap type method");
        if self
            .heap
            .get(receiver)
            .instance_class()
            .is_some_and(|actual| self.direct_subclass(actual, owner))
        {
            return Ok(());
        }
        let actual = self.type_name(receiver);
        Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '{declaring}' objects doesn't apply to a '{actual}' object"
            ),
        ))
    }

    pub(super) fn bind_attribute(&mut self, value: Id, receiver: Id, owner: Id) -> Result<Id> {
        if matches!(self.heap.get(value), Value::Native(name) if name == "dict.fromkeys") {
            stdlib::datetime::validate_classmethod_receiver(self, "dict", "fromkeys", owner)?;
            return Ok(self.bind_native_classmethod(value, owner));
        }
        if let Value::Native(symbol) = self.heap.get(value)
            && let Some(class) = typing::generic_alias_method_owner(symbol)
        {
            stdlib::datetime::validate_classmethod_receiver(
                self,
                class,
                "__class_getitem__",
                owner,
            )?;
            return Ok(self.bind_native_classmethod(value, owner));
        }
        if matches!(self.heap.get(value), Value::Native(name) if name == "float.fromhex") {
            stdlib::datetime::validate_classmethod_receiver(self, "float", "fromhex", owner)?;
        }
        if matches!(self.heap.get(value), Value::Native(name) if matches!(name.as_str(), "type.__prepare__" | "mappingproxy.__class_getitem__" | "set.__class_getitem__" | "frozenset.__class_getitem__" | "int.from_bytes" | "float.fromhex" | "float.__getformat__" | "bytes.fromhex" | "bytearray.fromhex") || stdlib::datetime::clock_classmethod(name))
        {
            return Ok(self.bind_native_classmethod(value, owner));
        }
        if matches!(self.heap.get(value), Value::Native(name) if matches!(name.as_str(), "object.__init_subclass__" | "object.__subclasshook__" | "typing.Generic.__init_subclass__"))
        {
            return Ok(self.alloc(Value::Method {
                callable: value,
                receiver: owner,
                explicit: false,
            }));
        }
        if let Value::ClassMethod { callable, .. } = *self.heap.get(value) {
            return Ok(self.bind_python_method(callable, owner));
        }
        if let Value::Super {
            start,
            receiver: bound,
            ..
        } = *self.heap.get(value)
        {
            return self.bind_super(value, receiver, start, bound);
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
                Value::LayoutDescriptor(owner, name) => {
                    return self.read_layout_descriptor(*owner, name, receiver);
                }
                Value::GetSetDescriptor { get, .. } => {
                    return self.call(*get, vec![receiver], Vec::new());
                }
                Value::Function { .. } | Value::Cached { .. } | Value::Partial { .. } => {
                    return Ok(self.bind_python_method(value, receiver));
                }
                Value::IoMethod { .. } => true,
                Value::HeapTypeMethod { owner, name, .. } => {
                    self.check_heap_method_receiver(*owner, name, receiver)?;
                    true
                }
                Value::Native(name) => {
                    matches!(
                        name.as_str(),
                        "ast.AST.__init__"
                            | "bytes.startswith"
                            | "object.__getattribute__"
                            | "enum.auto.__init__"
                            | "enum.auto.__repr__"
                            | "enum.Flag.__repr__"
                            | "enum.Enum.__repr__"
                            | "enum.Enum.__str__"
                            | "enum.Enum.__format__"
                            | "enum.Enum.__copy__"
                            | "enum.Enum.__deepcopy__"
                            | "enum.Enum.__reduce_ex__"
                            | "object.__setattr__"
                            | "object.__delattr__"
                            | "object.__init__"
                            | "object.__str__"
                            | "bytes.count"
                            | "bytes.decode"
                            | "bytes.find"
                            | "bytes.index"
                            | "bytes.rfind"
                            | "bytes.rsplit"
                            | "bytes.rstrip"
                            | "int.bit_count"
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
                        || name.starts_with("FrameLocalsProxy.")
                        || name.starts_with("abc.")
                        || name.starts_with("selectors._SelectorMapping.")
                }
                _ => false,
            };
            if method {
                // A saved writelines method uses its receiver type in argument errors.
                let callable = match self.heap.get(value) {
                    Value::IoMethod {
                        owner: defining,
                        name: "writelines",
                        ..
                    } if self.direct_subclass(owner, *defining) => self.builtin("file.writelines"),
                    _ => value,
                };
                return Ok(self.alloc(Value::Method {
                    callable,
                    receiver,
                    explicit: false,
                }));
            }
        }
        Ok(value)
    }

    fn instance_class_member(&mut self, class: Id, name: &str) -> Option<Id> {
        // Class scopes also store type.__qualname__. Only a member of a published
        // type dictionary can supply that name to an instance through its MRO.
        if name == "__qualname__" {
            let key = self.heap.code_name(name);
            return self
                .class_dictionary_member(class, key)
                .map(|(_, _, member)| member);
        }
        self.class_attr(class, name)
    }

    pub(super) fn instance_attribute(
        &mut self,
        object: Id,
        class: Id,
        field: Option<Id>,
        name: &str,
    ) -> Result<Id> {
        let member = self.instance_class_member(class, name);
        self.resolved_instance_attribute(object, class, field, name, member)
    }

    fn resolved_instance_attribute(
        &mut self,
        object: Id,
        class: Id,
        field: Option<Id>,
        name: &str,
        member: Option<Id>,
    ) -> Result<Id> {
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
            (None, None)
                if (matches!(name, "__getattribute__" | "__delattr__")
                    || self.allocation_base(class) == Some(self.builtin("object")))
                    && let Some(method) = self.raw_object_member(name) =>
            {
                self.bind_attribute(method, object, class)
            }
            (None, None) => self.builtin_attribute(object, name, AttributeLookup::Value, None),
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
        if matches!(self.heap.get(parent), Value::Native(name) if name == "typing.Callable") {
            let origin = super::mapping_abc::callable(self)?;
            // Unsubscripted typing aliases use their origins as the candidate class.
            let typing_instance = |name| {
                self.module_value("typing", name)
                    .is_some_and(|class| self.instance_of(child, class))
            };
            let child = match self.heap.get(child) {
                _ if child == parent => origin,
                Value::GenericAlias(alias) if !alias.is_builtin_alias() => {
                    return Err(alias.kind.class_check_error("issubclass"));
                }
                Value::Instance { .. } if typing_instance("_SpecialGenericAlias") => {
                    self.lookup_attr(child, "__origin__")?
                }
                Value::Instance { .. } if typing_instance("_AnnotatedAlias") => {
                    return Err(super::typing::AliasKind::Generic.class_check_error("issubclass"));
                }
                _ => child,
            };
            return self.python_subclass_check(child, origin);
        }
        if let Value::GenericAlias(alias) = self.heap.get(parent) {
            return Err(alias.kind.class_check_error("issubclass"));
        }
        if let Some(check) = self.bound_slot(parent, "__subclasscheck__")?
            && self.slot(parent, "__subclasscheck__")
                != self.builtins.get("abc.__subclasscheck__").copied()
        {
            let result = self.call(check, vec![child], Vec::new())?;
            return self.truth(result);
        }
        if typing::alias_payload(self, parent).is_some() {
            return Err(typing::AliasKind::Builtin.class_check_error("issubclass"));
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
        let abc = self.is_bundled_abc(parent);
        let key = [parent, child];
        if abc && let Some(&result) = self.heap.abc_subclass_cache.get(&key) {
            return Ok(result);
        }
        let result = if self.has_abc_subclass_hook(parent)
            && let Some(matched) = self.abc_hook_result(child, parent)?
        {
            matched
        } else {
            // A direct registry hit bypasses the positive cache in _abc.
            if abc && self.heap.abc_registrations.contains(&[parent, child]) {
                return Ok(true);
            }
            self.subclass(child, parent)
        };
        if abc {
            self.heap
                .abc_subclass_cache
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            self.heap.abc_subclass_cache.insert(key, result);
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
            if Self::native_base(child) == Some(parent) {
                return true;
            }
            if parent == "object" {
                return self.is_type_object(class);
            }
            if let Some(result) = builtins::exception_subclass(child, parent) {
                return result;
            }
            return matches!(
                (child.as_str(), parent.as_str()),
                ("enum.Flag", "enum.Enum")
                    | ("enum.IntFlag", "enum.Flag")
                    | ("enum.IntEnum" | "enum.IntFlag", "int" | "enum.Enum")
                    | ("enum.StrEnum", "str" | "enum.Enum")
            );
        }
        false
    }
    /// Immediate native parent shared by subclass checks and the class lineage.
    pub(super) fn native_base(class: &str) -> Option<&'static str> {
        Some(match class {
            "bool" => "int",
            "typing._AnyMeta" => "type",
            "collections.abc._CallableGenericAlias" => "types.GenericAlias",
            "datetime.datetime" => "datetime.date",
            "datetime.timezone" => stdlib::datetime::TZINFO,
            stdlib::flags::TYPE => "tuple",
            class if stdlib::structseq::layout(class).is_some() => "tuple",
            class if stdlib::fixed_structseq::fixed_type(class).is_some() => "tuple",
            "typing._LiteralGenericAlias"
            | "typing._UnpackGenericAlias"
            | "typing._ConcatenateGenericAlias" => "typing._GenericAlias",
            HashState::XOF_CLASS => HashState::CLASS,
            "collections.defaultdict" => "dict",
            "odict_keys" => "dict_keys",
            "odict_items" => "dict_items",
            "odict_values" => "dict_values",
            _ => return None,
        })
    }

    /// The getset belongs to the dict view base even when an OrderedDict made the view.
    pub(super) fn dict_view_owner(name: &str) -> Option<&'static str> {
        match Self::native_base(name).unwrap_or(name) {
            "dict_keys" => Some("dict_keys"),
            "dict_items" => Some("dict_items"),
            "dict_values" => Some("dict_values"),
            _ => None,
        }
    }

    fn dict_view_mapping_member(&mut self, obj: Id) -> Option<Id> {
        let class = match self.heap.get(obj) {
            Value::Native(class) => class.as_str(),
            Value::DictView { .. } => self.type_name(obj),
            _ => return None,
        };
        let owner = self.builtin(Self::dict_view_owner(class)?);
        self.own_class_attr(owner, "mapping")
    }

    /// Resolve numeric fields on exact scalar values and their builtin classes.
    /// Subclasses find these descriptors through their normal method resolution order.
    fn builtin_numeric_member(&mut self, obj: Id, name: &str) -> Option<Id> {
        let kind = match self.heap.get(obj) {
            Value::Native(kind) => kind.as_str(),
            Value::Int(_) => "int",
            Value::Bool(_) => "bool",
            Value::Float(_) => "float",
            _ => return None,
        };
        let owner = match (kind, name) {
            ("int", "__doc__")
            | ("int" | "bool", "numerator" | "denominator" | "real" | "imag") => {
                self.builtin("int")
            }
            ("float", "real" | "imag") => self.builtin("float"),
            _ => return None,
        };
        self.own_class_attr(owner, name)
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
        if owner == "module"
            && field == "__dict__"
            && let Value::Module { scope, .. } = *self.heap.get(receiver)
        {
            return self.module_globals(scope);
        }
        if owner == "BaseExceptionGroup"
            && let Value::Raised(error) = self.heap.get(receiver)
            && let Some([message, exceptions]) = error.group()
        {
            return Ok(match field {
                "message" => message,
                "exceptions" => exceptions,
                _ => unreachable!("unknown exception group member"),
            });
        }
        if owner == value::ZSTD_COMPRESS_TYPE
            && let Value::ZstdCompressor(state) = self.heap.get(receiver)
        {
            let mode = state.lock().unwrap().last_mode;
            return Ok(self.int(i64::from(mode)));
        }
        if owner == value::ZSTD_DECOMPRESS_TYPE
            && let Value::ZstdDecompressor(state) = self.heap.get(receiver)
        {
            return Ok(state.field(field));
        }
        if owner == value::BZ2_DECOMPRESS_TYPE
            && let Value::Bz2Decompressor(state) = self.heap.get(receiver)
        {
            return stdlib::bz2::field(&state.borrow(), field);
        }
        if owner == value::LZMA_DECOMPRESS_TYPE
            && matches!(self.heap.get(receiver), Value::LzmaDecompressor(_))
        {
            return stdlib::lzma::decompressor_attr(self, receiver, field);
        }
        if owner == "super" {
            return self.super_field(receiver, field);
        }
        if owner == "datetime.timedelta" {
            return stdlib::datetime::read_field(self, owner, field, receiver);
        }
        if owner == stdlib::functools::PARTIAL_CLASS {
            let storage = self.scalar_value(receiver);
            if matches!(self.heap.get(storage), Value::Partial { .. }) {
                return self.lookup_attr(storage, field);
            }
        }
        let ty = self.exact_type(receiver)?;
        if ty == self.builtin(owner) {
            self.lookup_attr(receiver, field)
        } else {
            Err(self.member_receiver_error(owner, field, ty))
        }
    }

    fn member_receiver_error(&self, owner: &str, field: &str, ty: Id) -> Error {
        let name = self.class_name(ty);
        let name = &name[..name.floor_char_boundary(100)];
        Error::new(
            "TypeError",
            format!(
                "descriptor '{field}' for '{owner}' objects doesn't apply to a '{name}' object"
            ),
        )
    }

    /// A read-only member rejects an unrelated receiver before reporting a read-only write.
    pub(super) fn reject_member_write(
        &mut self,
        owner: &str,
        field: &str,
        receiver: Id,
    ) -> Result<()> {
        let ty = self.exact_type(receiver)?;
        let class = self.builtin(owner);
        if !self.direct_subclass(ty, class) {
            return Err(self.member_receiver_error(owner, field, ty));
        }
        Err(Error::new("AttributeError", "readonly attribute"))
    }

    pub(super) fn is_type_object(&self, obj: Id) -> bool {
        match self.heap.get(obj) {
            Value::Class { .. } => true,
            Value::Native(_) => matches!(
                self.builtin_type_name(obj),
                Some("type" | "typing._AnyMeta")
            ),
            _ => false,
        }
    }

    /// Read native getset documentation without rebinding the descriptor's own __doc__.
    pub(super) fn getset_documentation(&mut self, descriptor: Id) -> Result<Id> {
        let doc = match *self.heap.get(descriptor) {
            Value::LayoutDescriptor(owner, field) => {
                if matches!(self.heap.get(owner), Value::Native(name) if name == "BaseException") {
                    None
                } else {
                    match field {
                        "__dict__" => Some("dictionary for instance variables"),
                        "__class__" => Some("the object's class"),
                        "__weakref__" => Some("list of weak references to the object"),
                        _ => None,
                    }
                }
            }
            Value::GetSetDescriptor {
                owner, name, get, ..
            } => match (owner, name) {
                ("code", _) => None,
                ("int", "numerator") => Some("the numerator of a rational number in lowest terms"),
                ("int", "denominator") => {
                    Some("the denominator of a rational number in lowest terms")
                }
                ("int" | "float", "real") => Some("the real part of a complex number"),
                ("int" | "float", "imag") => Some("the imaginary part of a complex number"),
                ("struct.Struct", "format") => Some("struct format string"),
                ("struct.Struct", "size") => Some("struct size in bytes"),
                (super::deque::CLASS, "maxlen") => {
                    Some("maximum size of a deque or None if unbounded")
                }
                (_, "mapping") if Self::dict_view_owner(owner).is_some() => {
                    Some("dictionary that this view refers to")
                }
                ("wrapper_descriptor" | "getset_descriptor", "__doc__")
                | (stdlib::datetime::isocalendar::CLASS | super::thread_handle::CLASS, _)
                | ("datetime.date" | "datetime.datetime" | "datetime.time", _) => None,
                (_, "__dict__") if matches!(self.heap.get(get), Value::IoMethod { .. }) => None,
                _ => return Err(self.missing_attribute(descriptor, "__doc__")),
            },
            _ => unreachable!("validated getset descriptor"),
        };
        Ok(doc.map_or(NONE, |text| self.text(text)))
    }

    /// A read-only getset can be installed on another class without changing its receiver.
    pub(super) fn readonly_getset_error(&self, owner: &str, name: &str, receiver: Id) -> Error {
        if matches!(owner, "wrapper_descriptor" | "getset_descriptor")
            && let Err(error) =
                builtins::noargs_method_receiver(self, owner, name, &[receiver], &[])
        {
            return error;
        }
        Error::new(
            "AttributeError",
            format!("attribute '{name}' of '{owner}' objects is not writable"),
        )
    }

    /// Return the stored name of a slot descriptor that binds as a method wrapper.
    pub(super) fn wrapper_name(&self, descriptor: Id) -> Option<&'static str> {
        match self.heap.get(descriptor) {
            Value::Native(symbol) => BuiltinDescriptor::lookup(symbol)
                .filter(|slot| slot.class() == "wrapper_descriptor")
                .map(|slot| slot.name),
            Value::IoMethod {
                name: name @ ("__init__" | "__repr__" | "__iter__" | "__next__" | "__del__"),
                ..
            }
            | Value::HeapTypeMethod {
                name,
                slot_wrapper: true,
                ..
            } => Some(*name),
            _ => None,
        }
    }

    pub(super) fn builtin_type_name(&self, obj: Id) -> Option<&str> {
        Some(match self.heap.get(obj) {
            Value::Module { .. } => self.type_name(obj),
            Value::DefaultDict { .. } => "collections.defaultdict",
            Value::Partial { .. } => "functools.partial",
            Value::Cached { .. } => LRU_CACHE_CLASS,
            Value::KeyWrapper { .. } => KEY_WRAPPER,
            Value::BytesIoBuffer { .. } => "_io._BytesIOBuffer",
            Value::PartialPlaceholder => "functools._PlaceholderType",
            Value::Count { .. } => "itertools.count",
            Value::Repeat(_) => "itertools.repeat",
            Value::TupleGetter { .. } => super::namedtuple::GETTER,
            Value::TypeVar(_) => "typing.TypeVar",
            Value::ParamSpec(_) => "typing.ParamSpec",
            Value::ParamSpecMarker(kind, _) => kind.class(),
            Value::ConstEvaluator(_) => "constevaluator",
            Value::TypeVarTuple { .. } => "typing.TypeVarTuple",
            Value::GenericAlias(alias) => alias.kind.class(),
            Value::Socket { .. } => "_socket.socket",
            Value::Union(_) => "typing.Union",
            Value::Method { explicit: true, .. } => "method",
            Value::Method { callable, .. } => match self.heap.get(*callable) {
                Value::Function { .. } => "method",
                _ if self.wrapper_name(*callable).is_some() => "method-wrapper",
                Value::Native(_)
                | Value::IoOpen { .. }
                | Value::IoMethod { .. }
                | Value::HeapTypeMethod { .. } => "builtin_function_or_method",
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
                    || (cfg!(target_os = "linux")
                        && matches!(name.as_str(), "select.epoll" | "mmap.mmap"))
                    || stdlib::structseq::layout(name).is_some()
                    || builtins::is_builtin_type(name)
                    || builtins::is_iterator_type(name)
                    || builtins::CORE_RUNTIME_TYPES.contains(&name.as_str())
                    || matches!(
                        name.as_str(),
                        "odict_keys"
                            | "odict_items"
                            | "odict_values"
                            | KEY_WRAPPER
                            | "functools._PlaceholderType"
                            | LRU_CACHE_CLASS
                            | "functools.partial"
                            | HashState::CLASS
                            | HashState::XOF_CLASS
                            | "operator.attrgetter"
                            | super::namedtuple::GETTER
                            | "operator.itemgetter"
                            | "operator.methodcaller"
                            | "datetime.date"
                            | "datetime.datetime"
                            | "datetime.time"
                            | "datetime.timedelta"
                            | "datetime.timezone"
                            | stdlib::datetime::TZINFO
                            | value::ZSTD_COMPRESS_TYPE
                            | value::ZSTD_DECOMPRESS_TYPE
                            | value::BZ2_COMPRESS_TYPE
                            | value::BZ2_DECOMPRESS_TYPE
                            | value::LZMA_COMPRESS_TYPE
                            | value::LZMA_DECOMPRESS_TYPE
                            | value::ZLIB_COMPRESS_TYPE
                            | "struct.Struct"
                            | value::ZLIB_DECOMPRESS_TYPE
                            | "hmac.HMAC"
                            | "collections.defaultdict"
                            | "_contextvars.Context"
                            | "_contextvars.ContextVar"
                            | "_contextvars.Token"
                            | "array.array"
                            | "selectors.PollSelector"
                            | "selectors.SelectSelector"
                            | super::deque::CLASS
                            | "weakref.ReferenceType"
                            | "weakref.ProxyType"
                            | "weakref.CallableProxyType"
                            | super::thread_local::CLASS
                            | "_io._BytesIOBuffer"
                            | "_buffer_wrapper"
                            | stdlib::os::scandir::ENTRY
                            | stdlib::os::scandir::ITER
                            | super::thread_rlock::CLASS
                            | super::thread_handle::CLASS
                            | super::thread_lock::CLASS
                            | value::REGEX_PATTERN_TYPE
                            | value::REGEX_MATCH_TYPE
                            | value::REGEX_SCANNER_TYPE
                            | value::TEMPLATE_TYPE
                            | value::TEMPLATE_ITER_TYPE
                            | value::INTERPOLATION_TYPE
                            | "typing.TypeVar"
                            | "typing.ParamSpec"
                            | "typing.ParamSpecArgs"
                            | "typing.ParamSpecKwargs"
                            | "constevaluator"
                            | "typing.TypeVarTuple"
                            | "typing.TypeAliasType"
                            | "typing._ConcatenateGenericAlias"
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
                    || builtins::native_method_owner(name).is_some()
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
            Value::FrameLocalsProxy { class, .. } => Ok(*class),
            Value::Socket { class, .. }
            | Value::Instance { class, .. }
            | Value::Class {
                metaclass: Some(class),
                ..
            }
            | Value::Module {
                constructor: Some(class),
                ..
            } => Ok(*class),
            Value::NamedTuple {
                class: Some(class), ..
            } => Ok(self.builtin(class)),
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
            ("type", _) if self.builtins.get("type") == Some(&class) => self.is_type_object(value),
            (HashState::CLASS, Value::Hash(state)) => {
                !HashState::is_provider_class(state.lock().class_name())
            }
            (class, Value::Hash(state)) if HashState::is_provider_class(class) => {
                state.lock().class_name() == class
            }
            (HashState::XOF_CLASS, Value::Hash(state)) => state.lock().class_name() == name,
            (_, Value::Raised(error)) => {
                matches!(self.heap.get(class), Value::Native(name) if name == "object")
                    || self.match_raised_class(error, class, Self::subclass)
            }
            #[cfg(target_os = "linux")]
            ("select.epoll", Value::Epoll(_)) => true,
            #[cfg(target_os = "linux")]
            ("mmap.mmap", Value::Mmap { .. }) => true,
            (name, Value::Selector(handle)) if name == handle.borrow().class_name() => true,
            ("array.array", Value::Array(_))
            | ("bytearray", Value::ByteArray(_))
            | ("memoryview", Value::MemoryView { .. })
            | ("object", _)
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("float", Value::Float(_))
            | (value::ZSTD_COMPRESS_TYPE, Value::ZstdCompressor(_))
            | (value::ZSTD_DECOMPRESS_TYPE, Value::ZstdDecompressor(_))
            | (value::BZ2_COMPRESS_TYPE, Value::Bz2Compressor(_))
            | (value::BZ2_DECOMPRESS_TYPE, Value::Bz2Decompressor(_))
            | (value::LZMA_COMPRESS_TYPE, Value::LzmaCompressor(_))
            | (value::LZMA_DECOMPRESS_TYPE, Value::LzmaDecompressor(_))
            | (value::ZLIB_COMPRESS_TYPE, Value::ZlibCompressor(_))
            | ("complex", Value::Complex(_))
            | ("str", Value::Str(_) | Value::SurrogateStr(_))
            | ("bytes", Value::Bytes(_))
            | (value::ZLIB_DECOMPRESS_TYPE, Value::ZlibDecompressor { .. })
            | ("struct.Struct", Value::Struct { .. })
            | ("hmac.HMAC", Value::Hmac(_))
            | ("list", Value::List(_))
            | ("dict", Value::Dict(_) | Value::DefaultDict { .. })
            | ("collections.defaultdict", Value::DefaultDict { .. })
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | ("slice", Value::Slice(_))
            | ("range", Value::Range(_))
            | ("map", Value::Map { .. })
            | ("zip", Value::Zip { .. })
            | ("filter", Value::Filter { .. })
            | ("_contextvars.ContextVar", Value::ContextVar { .. })
            | ("_contextvars.Context", Value::Context(_))
            | ("functools.partial", Value::Partial { .. })
            | (LRU_CACHE_CLASS, Value::Cached { .. })
            | ("_contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | (super::namedtuple::GETTER, Value::TupleGetter { .. })
            | (
                "member_descriptor",
                Value::TupleGetterDoc | Value::ReadOnlyMember(_, _) | Value::SlotMember { .. },
            )
            | ("operator.itemgetter", Value::ItemGetter(_))
            | ("operator.methodcaller", Value::MethodCaller { .. })
            | ("itertools.count", Value::Count { .. })
            | (super::deque::CLASS, Value::Deque(_))
            | (super::deque::ITERATOR, Value::DequeIterator { reverse: false, .. })
            | (super::deque::REVERSE_ITERATOR, Value::DequeIterator { reverse: true, .. })
            | ("itertools.repeat", Value::Repeat(_))
            | (super::thread_local::CLASS, Value::ThreadLocal(_))
            | ("_io._BytesIOBuffer", Value::BytesIoBuffer { .. })
            | ("_buffer_wrapper", Value::BufferWrapper)
            | (stdlib::os::scandir::ENTRY, Value::DirEntry(_))
            | (stdlib::os::scandir::ITER, Value::Scandir(_))
            | (super::thread_rlock::CLASS, Value::HandlerLock(_))
            | (super::thread_handle::CLASS, Value::ThreadHandle(_))
            | (super::thread_lock::CLASS, Value::PrimitiveLock(_))
            | (value::TEMPLATE_TYPE, Value::Template { .. })
            | (value::TEMPLATE_ITER_TYPE, Value::TemplateIter { .. })
            | (value::INTERPOLATION_TYPE, Value::Interpolation { .. })
            | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => true,
            ("types.GenericAlias", Value::GenericAlias(alias)) => alias.is_builtin_alias(),
            ("typing._GenericAlias", Value::GenericAlias(alias))
                if matches!(
                    alias.kind,
                    super::typing::AliasKind::Unpack | super::typing::AliasKind::Concatenate
                ) =>
            {
                true
            }
            (
                name,
                Value::NamedTuple {
                    class: Some(class), ..
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
    /// Every tuple member must be a real exception class, even after an earlier match.
    pub(super) fn check_exception_classes(&self, classes: &[Id], base: Id) -> Result<()> {
        if classes
            .iter()
            .all(|&class| self.is_type_object(class) && self.direct_subclass(class, base))
        {
            Ok(())
        } else {
            Err(Error::new(
                "TypeError",
                "catching classes that do not inherit from BaseException is not allowed",
            ))
        }
    }

    pub(super) fn raised_error_matches(&self, error: &Error, class: Id) -> bool {
        self.match_raised_class(error, class, Self::direct_subclass)
    }

    fn match_raised_class(
        &self,
        error: &Error,
        class: Id,
        subtype: fn(&Self, Id, Id) -> bool,
    ) -> bool {
        match self.heap.get(class) {
            Value::Tuple(classes) => classes
                .iter()
                .any(|&c| self.match_raised_class(error, c, subtype)),
            _ if let Some(actual) = error.class() => subtype(self, actual, class),
            Value::Native(_) => {
                let kind = error.kind.as_ref();
                self.error_kind_matches(kind, class, self.builtins.get(kind).copied(), subtype)
            }
            // Native operations on intrinsic file objects belong to the first I/O module.
            Value::Class { .. } if error.kind == "UnsupportedOperation" => self
                .builtins
                .get("UnsupportedOperation")
                .is_some_and(|&initial| subtype(self, initial, class)),
            _ => false,
        }
    }

    fn error_kind_matches(
        &self,
        kind: &str,
        class: Id,
        registered: Option<Id>,
        subtype: fn(&Self, Id, Id) -> bool,
    ) -> bool {
        if let Value::Native(parent) = self.heap.get(class)
            && let Some(result) = builtins::exception_subclass(kind, parent)
        {
            return result;
        }
        match self.heap.get(class) {
            Value::Tuple(classes) => classes
                .iter()
                .any(|&c| self.error_kind_matches(kind, c, registered, subtype)),
            Value::Class { name, .. } | Value::Native(name) => {
                kind == name
                    || name == "BaseException"
                    || (name == "Exception" && kind != "SystemExit")
                    || (name == "ValueError"
                        && matches!(kind, "JSONDecodeError" | "binascii.Error"))
                    || (name == "OSError" && kind == "socket.gaierror")
                    || registered.is_some_and(|actual| subtype(self, actual, class))
            }
            _ => false,
        }
    }
    pub(super) fn missing_attribute(&self, obj: Id, name: &str) -> Error {
        let message = if self.is_type_object(obj) {
            let class = self.class_name(obj);
            let class = &class[..class.floor_char_boundary(100)];
            format!("type object '{class}' has no attribute '{name}'")
        } else {
            let class = match self.heap.get(obj) {
                Value::Partial { .. } => stdlib::functools::PARTIAL_CLASS,
                Value::Native(symbol) if let Some(slot) = BuiltinDescriptor::lookup(symbol) => {
                    slot.class()
                }
                Value::Method { .. } => self.builtin_type_name(obj).unwrap_or("method"),
                _ => self.type_name(obj),
            };
            format!("'{class}' object has no attribute '{name}'")
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
    pub(super) fn exception_argument_tuple(&mut self, obj: Id, error: Error) -> Id {
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

    fn read_exception_getset(&mut self, obj: Id, name: &str) -> Result<Id> {
        let Value::Raised(error) = self.heap.get(obj) else {
            return Err(self.missing_attribute(obj, name));
        };
        match name {
            "args" => {
                let error = error.clone();
                match self.os_attribute(obj, "args")? {
                    Some(args) => Ok(args),
                    None => Ok(self.exception_argument_tuple(obj, error)),
                }
            }
            "__traceback__" => self.exception_traceback(obj),
            "__context__" => Ok(error.context),
            "__cause__" => Ok(error
                .trace
                .as_ref()
                .and_then(|trace| trace.cause)
                .unwrap_or(NONE)),
            _ => unreachable!(),
        }
    }

    fn write_exception_getset(&mut self, obj: Id, name: &str, value: Option<Id>) -> Result<()> {
        if !matches!(self.heap.get(obj), Value::Raised(_)) {
            return Err(self.missing_attribute(obj, name));
        }
        let value =
            value.ok_or_else(|| Error::new("TypeError", format!("{name} may not be deleted")))?;
        match name {
            "__traceback__" => self.set_traceback(obj, value),
            "__cause__" => self.set_exception_cause(obj, value),
            "__context__" => {
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
                Ok(())
            }
            "args" => {
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
                Ok(())
            }
            _ => unreachable!(),
        }
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
        attr: AttributeName<'_>,
        original: Option<Id>,
    ) -> Result<Id> {
        let callable = self.bind_attribute(hook, obj, class)?;
        let key = original.unwrap_or_else(|| match attr {
            AttributeName::Linked(name) => self.heap.code_name(name),
            _ => attr.key(self),
        });
        self.call(callable, vec![key], Vec::new())
    }

    /// Read an optional attribute, preserving exceptions raised by user lookup hooks.
    pub(super) fn optional_attr(&mut self, obj: Id, name: &str) -> Result<Option<Id>> {
        match self.lookup_attribute(obj, AttributeName::Text(name), false) {
            Ok(value) => Ok(Some(value)),
            Err(error) if self.is_attribute_error(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Normalize the globals' __builtins__ at function creation, keeping its mapping live.
    pub(super) fn definition_builtins(&mut self, value: Id) -> Result<Id> {
        match *self.heap.get(value) {
            Value::Module { scope, .. } => self.module_globals(scope),
            _ => Ok(value),
        }
    }

    pub(super) fn function_environment(&mut self, module: &str, globals: Id, builtins: Id) -> Id {
        let scope = self.scope(module, None);
        let Value::Scope(environment) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        environment.namespace = Some(globals);
        environment.exec_globals = Some(value::ExecGlobals {
            dict: globals,
            builtins,
        });
        scope
    }

    /// Read the function's module before its builtins, using the defining namespace's keys.
    pub(super) fn capture_function_namespace(
        &mut self,
        lookup: impl Fn(&mut Self, &str) -> Result<Option<Id>>,
        capture_builtins: bool,
    ) -> Result<(Id, Option<Id>)> {
        let module = lookup(self, "__name__")?.unwrap_or(NONE);
        // A key comparison can remove the module before the function takes ownership.
        self.heap.keep(module);
        let builtins = if capture_builtins {
            lookup(self, "__builtins__")?
                .map(|value| self.definition_builtins(value))
                .transpose()?
        } else {
            None
        };
        Ok((module, builtins))
    }

    /// Retain the lexical environment for code whose closure layout is unavailable.
    pub(super) fn new_function_globals(
        &mut self,
        scope: Id,
        known_closure: bool,
    ) -> Result<(Id, Id)> {
        let root = self.root_scope(scope);
        let environment = if known_closure { root } else { scope };
        let (module, builtins) = self.capture_function_namespace(
            |ctx, name| ctx.lookup_scope_name(root, name),
            environment == root,
        )?;
        let Some(chosen) = builtins else {
            return Ok((module, environment));
        };
        let previous = self
            .scope_data(root)
            .exec_globals
            .map(|globals| globals.builtins)
            .or(self.source_builtins);
        if previous == Some(chosen) {
            return Ok((module, root));
        }
        let globals = self.module_globals(root)?;
        let label = self.scope_data(root).module.clone();
        Ok((module, self.function_environment(&label, globals, chosen)))
    }

    /// Return the builtins captured by a function's defining environment.
    pub(super) fn function_builtins(&mut self, scope: Id) -> Result<Id> {
        match self.scope_data(self.root_scope(scope)).exec_globals {
            Some(globals) => Ok(globals.builtins),
            None => self.default_source_builtins(),
        }
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
        self.lookup_attribute(obj, AttributeName::from_lookup(name, original), true)
    }

    pub(super) fn lookup_attribute(
        &mut self,
        obj: Id,
        attr: AttributeName<'_>,
        report_missing: bool,
    ) -> Result<Id> {
        self.lookup_attribute_operand(obj, attr, report_missing, attr.original())
    }

    /// Preserve a linked operand for user hooks while an exact static name takes the text path.
    pub(super) fn lookup_attribute_operand(
        &mut self,
        obj: Id,
        attr: AttributeName<'_>,
        report_missing: bool,
        original: Option<Id>,
    ) -> Result<Id> {
        let spelling = attr.text(self)?;
        let name = spelling.as_ref();
        let obj = self.weak_proxy_target(obj)?;
        let obj = super::typing::generic_attr_target(self, obj, name, false)?;
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.local_attribute(obj, attr);
        }
        let class = match *self.heap.get(obj) {
            Value::Instance { class, .. }
            | Value::Class {
                metaclass: Some(class),
                ..
            } => class,
            Value::Raised(ref error) if let Some(class) = error.class() => class,
            Value::Module {
                constructor: Some(class),
                ..
            } if matches!(self.heap.get(class), Value::Class { .. }) => class,
            Value::Module { scope, .. } => {
                // Exact ModuleType has no data descriptors for ordinary names.
                if !name.starts_with("__")
                    && let Some(field) = self.scope_name(scope, name)
                {
                    return Ok(field);
                }
                return self.module_get_attribute(obj, scope, name, original, report_missing);
            }
            Value::Super { .. } => return self.super_getattribute(obj, name),
            _ => return self.default_object_attribute(obj, attr, ObjectAttribute::Get),
        };
        let attribute = if let Some(hook) = self.class_attr(class, "__getattribute__") {
            self.attribute_hook(hook, obj, class, attr, original)
        } else if name == "__class__"
            && attr.nonexact_key(self).is_none()
            && matches!(self.heap.get(obj), Value::Instance { .. })
            && self.class_attr(class, name).is_none()
        {
            // object.__class__ is a data descriptor and takes precedence over instance fields.
            Ok(class)
        } else if let Value::Module { scope, .. } = *self.heap.get(obj) {
            self.module_get_attribute(obj, scope, name, original, report_missing)
        } else {
            self.default_object_attribute(obj, attr, ObjectAttribute::Get)
        };
        match attribute {
            Err(error) if self.is_attribute_error(&error) => {
                let Some(hook) = self.class_attr(class, "__getattr__") else {
                    return Err(error);
                };
                self.attribute_hook(hook, obj, class, attr, original)
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
            return self.builtin_attribute(obj, name, AttributeLookup::Value, None);
        }
        let mut kind = self
            .builtin_type_name(obj)
            .unwrap_or_else(|| self.type_name(obj));
        let mut module = None;
        if let Some(class) = self.heap.get(obj).instance_class()
            && let Value::Class { scope, .. } = self.heap.get(class)
        {
            if let Some(qualified) = self.namespace_binding(self.scope_data(*scope), "__qualname__")
            {
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

    /// Invoke the exit callable saved on entry with the exact body exception and traceback.
    pub(super) fn call_context_exit(&mut self, exit: Id, exception: Id) -> Result<Id> {
        let (class, traceback) = if exception == NONE {
            (NONE, NONE)
        } else {
            (
                self.exact_type(exception)?,
                self.exception_traceback(exception)?,
            )
        };
        self.call(exit, vec![class, exception, traceback], Vec::new())
    }

    /// Resolve an ordinary string method before evaluating positional arguments.
    /// Keep an exact string receiver alive if an argument rebinds its last owner.
    pub(super) fn string_method_target(&mut self, receiver: Id, symbol: &str) -> Result<(Id, Id)> {
        if matches!(
            self.heap.get(receiver),
            Value::Str(_) | Value::SurrogateStr(_)
        ) {
            return self.retain_builtin_method_receiver(receiver, symbol);
        }
        let name = symbol.strip_prefix("str.").expect("compiled string method");
        self.builtin_method_target(receiver, name)
    }

    /// Keep exact container and regular-expression receivers without allocating a bound method.
    pub(super) fn builtin_method_target(&mut self, receiver: Id, name: &str) -> Result<(Id, Id)> {
        let value = self.heap.get(receiver);
        if matches!(
            value,
            Value::Array(_)
                | Value::List(_)
                | Value::Set { .. }
                | Value::Tuple(_)
                | Value::RegexPattern(_)
                | Value::RegexMatch(_)
                | Value::RegexScanner { .. }
        ) && let Some(symbol) = builtins::method_name(value, name)
        {
            return self.retain_builtin_method_receiver(receiver, &symbol);
        }
        let mut bound = NONE;
        let callable = self.lookup_method_with_receiver(receiver, name, Some(&mut bound))?;
        Ok((callable, bound))
    }

    /// Resolve an immediate get call while preserving custom attribute lookup for other receivers.
    pub(super) fn dict_get_target(&mut self, receiver: Id) -> Result<(Id, Id)> {
        if matches!(self.heap.get(receiver), Value::Dict(_)) {
            return self.retain_builtin_method_receiver(receiver, "dict.get");
        }
        self.builtin_method_target(receiver, "get")
    }

    /// Keep the receiver alive while the caller evaluates the positional arguments.
    fn retain_builtin_method_receiver(&mut self, receiver: Id, symbol: &str) -> Result<(Id, Id)> {
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(receiver);
        Ok((self.builtin(symbol), receiver))
    }

    /// Resolve an immediately called attribute before its arguments are evaluated.
    pub(super) fn lookup_method(&mut self, obj: Id, name: &str) -> Result<Id> {
        self.lookup_method_with_receiver(obj, name, None)
    }

    /// Let a positional call keep the receiver separately when looking up an instance method.
    fn lookup_method_with_receiver(
        &mut self,
        obj: Id,
        name: &str,
        receiver: Option<&mut Id>,
    ) -> Result<Id> {
        if name == "writelines"
            && matches!(
                self.heap.get(obj),
                Value::BufferedRandom(..) | Value::File(_)
            )
        {
            if let Some(value) =
                self.dictionary_field(self.heap.get(obj).native_file_dict(), name)?
            {
                return Ok(value);
            }
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
            self.default_attr(obj, AttributeName::Text(name), AttributeLookup::Method(receiver))
        } else {
            self.lookup_attribute(obj, AttributeName::Linked(name), true)
        }
    }

    /// Base lookup bypasses Python's __getattribute__ and __getattr__ hooks.
    pub(super) fn default_get_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        self.default_attr(obj, AttributeName::Text(name), AttributeLookup::Value)
    }

    /// Resolve a dir-listed member of a class with the default metaclass for no_type_check.
    pub(super) fn no_type_check_member(&mut self, class: Id, name: &str) -> Result<Id> {
        self.default_attr(
            class,
            AttributeName::Text(name),
            AttributeLookup::ListedClassMember,
        )
    }

    /// Resolve a Python name object against immutable native type dictionaries.
    fn native_attribute_with_key(
        &mut self,
        obj: Id,
        class: Id,
        receiver: Id,
        key: Id,
        spelling: &str,
    ) -> Result<Id> {
        let meta = self.builtin("type");
        let meta_member = if receiver == NONE {
            self.class_dictionary_member(meta, key)
                .map(|(_, _, member)| member)
        } else {
            None
        };
        if let Some(member) = meta_member
            && self.readable_data_descriptor(member)
        {
            return self.bind_attribute(member, obj, meta);
        }
        if let Some((base, stored, member)) = self.class_dictionary_member(class, key) {
            if base == class
                && matches!(self.heap.get(member), Value::Native(symbol)
                    if BuiltinDescriptor::lookup(symbol).is_some())
            {
                return self.bind_attribute(member, receiver, class);
            }
            // Some native implementations are intrinsic and absent from their type
            // dictionary. Use the spelling actually selected by equality to dispatch them.
            let name = self.str_value(stored)?.to_owned();
            return self.default_get_attr(obj, &name);
        }
        if let Some(member) = meta_member {
            return self.bind_attribute(member, obj, meta);
        }
        Err(self.missing_attribute(obj, spelling))
    }

    /// Keep non-data wrapper receiver checks at call time for direct method calls.
    fn default_attr(
        &mut self,
        obj: Id,
        attr: AttributeName<'_>,
        mut lookup: AttributeLookup<'_>,
    ) -> Result<Id> {
        let spelling = attr.text(self)?;
        let name = spelling.as_ref();
        if let Value::Socket { class, .. } = *self.heap.get(obj)
            && let Some(member) = self.class_attr(class, name)
        {
            return self.bind_attribute(member, obj, class);
        }
        let native_key = match (attr.nonexact_key(self), self.heap.get(obj)) {
            (Some(key), Value::Native(owner))
                if matches!(owner.as_str(), "str" | "tuple" | "list") =>
            {
                Some((obj, NONE, key))
            }
            (
                Some(key),
                Value::Str(_) | Value::SurrogateStr(_) | Value::Tuple(_) | Value::List(_),
            ) => Some((self.exact_type(obj)?, obj, key)),
            _ => None,
        };
        if let Some((class, receiver, key)) = native_key
            && (name == "__getattribute__" || self.class_attr(class, name).is_some_and(|member| {
                matches!(self.heap.get(member), Value::Native(symbol) if BuiltinDescriptor::lookup(symbol).is_some())
            }))
        {
            return self.native_attribute_with_key(obj, class, receiver, key, name);
        }
        if name == "__dict__"
            && matches!(self.heap.get(obj), Value::Native(symbol) if builtins::exception_kind(symbol).is_some())
        {
            // A mapping proxy exposes the identity of its type's underlying dictionary.
            self.native_type_dicts
                .entry(obj)
                .or_insert_with(|| self.heap.alloc(Value::Dict(Default::default())));
        }
        if name == "__dict__"
            && let Some(&dictionary) = self.native_type_dicts.get(&obj)
        {
            if matches!(self.heap.get(obj), Value::Native(symbol) if symbol == "typing._GenericAlias")
            {
                typing::include_generic_alias_methods(self, obj, dictionary)?;
            }
            return Ok(self.alloc(Value::MappingProxy(dictionary)));
        }
        if let Value::Native(owner) = self.heap.get(obj)
            && matches!(
                owner.as_str(),
                "frame"
                    | super::thread_handle::CLASS
                    | value::ZSTD_COMPRESS_TYPE
                    | value::ZSTD_DECOMPRESS_TYPE
                    | value::BZ2_COMPRESS_TYPE
                    | value::BZ2_DECOMPRESS_TYPE
                    | value::LZMA_COMPRESS_TYPE
                    | value::LZMA_DECOMPRESS_TYPE
            )
        {
            if let Some(member) = self.class_attr(obj, name) {
                return Ok(member);
            }
            match name {
                "__name__" | "__qualname__" => {
                    return Ok(self.text(owner.rsplit('.').next().unwrap().to_owned()));
                }
                "__base__" => return Ok(self.builtin("object")),
                "__bases__" | "__mro__" => return Ok(self.builtin_hierarchy_tuple(obj, name)),
                _ => {}
            }
        }
        if matches!(
            self.heap.get(obj),
            Value::PythonFrame { .. }
                | Value::ThreadHandle(_)
                | Value::ZstdCompressor(_)
                | Value::ZstdDecompressor(_)
                | Value::Bz2Compressor(_)
                | Value::Bz2Decompressor(_)
                | Value::LzmaCompressor(_)
                | Value::LzmaDecompressor(_)
        ) {
            let class = self.exact_type(obj)?;
            if let Some(member) = self.class_attr(class, name) {
                return self.bind_attribute(member, obj, class);
            }
        }
        if name == "__new__" {
            let class = self.heap.get(obj).instance_class().unwrap_or(obj);
            if self.nonconstructible_heap_type(class).is_some() {
                return Ok(self.builtin("object.__new__"));
            }
        }
        if let Some(&scope) = self.enum_scopes.get(&obj)
            && let Some(value) = self.enum_base_attribute(obj, scope, name)?
        {
            return Ok(value);
        }
        let builtin_member = match name {
            "mapping" => self.dict_view_mapping_member(obj),
            "numerator" | "denominator" | "real" | "imag" | "__doc__" => {
                self.builtin_numeric_member(obj, name)
            }
            _ => None,
        };
        if let Some(descriptor) = builtin_member {
            if matches!(self.heap.get(obj), Value::Native(_)) {
                return Ok(descriptor);
            }
            let class = self.exact_type(obj)?;
            return self.bind_attribute(descriptor, obj, class);
        }
        if self.heap.get(obj).native_io_class_name().is_some() {
            let class = self.exact_type(obj)?;
            let member = self.class_attr(class, name);
            if name != "__class__"
                && !member.is_some_and(|member| self.readable_data_descriptor(member))
                && let Some(value) =
                    self.dictionary_field(self.heap.get(obj).native_file_dict(), name)?
            {
                return Ok(value);
            }
            if let Some(member) = member
                && matches!(
                    self.heap.get(member),
                    Value::IoMethod { .. } | Value::GetSetDescriptor { .. }
                )
            {
                return self.bind_attribute(member, obj, class);
            }
        }
        // These native layouts inherit object's attribute reader. A Python mixin can
        // supply the member earlier in the published MRO.
        let inherits_reader = |kind: &str| {
            matches!(
                kind,
                "tuple"
                    | "list"
                    | "dict"
                    | "str"
                    | "bytes"
                    | "bytearray"
                    | "int"
                    | "bool"
                    | "float"
                    | "complex"
                    | "set"
                    | "frozenset"
            )
        };
        let reader = if name != "__getattribute__" {
            None
        } else if self.is_type_object(obj) {
            self.allocation_base(obj)
                .filter(|&base| matches!(self.heap.get(base), Value::Native(kind) if inherits_reader(kind)))
                .map(|_| (obj, NONE))
        } else if self
            .heap
            .get(obj)
            .intrinsic_type_name()
            .is_some_and(inherits_reader)
        {
            Some((self.exact_type(obj)?, obj))
        } else {
            None
        };
        let named_reader = reader.is_some_and(|(_, receiver)| receiver == NONE)
            && attr.nonexact_key(self).is_some();
        let metaclass_member = match *self.heap.get(obj) {
            _ if named_reader => {
                let meta = self.exact_type(obj)?;
                self.class_attr_with_name(meta, attr)
                    .map(|member| (meta, member))
            }
            Value::Class {
                metaclass: Some(meta),
                ..
            } => self.class_attr(meta, name).map(|member| (meta, member)),
            Value::Class { .. } if name == "__text_signature__" => {
                let meta = self.builtin("type");
                self.own_class_attr(meta, name).map(|member| (meta, member))
            }
            _ => None,
        };
        if let Some((meta, member)) = metaclass_member
            && self.readable_data_descriptor(member)
        {
            return self.bind_attribute(member, obj, meta);
        }
        if let Some((class, receiver)) = reader {
            let member = if attr.nonexact_key(self).is_some() {
                self.class_attr_with_name(class, attr)
            } else {
                let object = self.builtin("object");
                self.base_lineage(class, true).into_iter().find_map(|base| {
                    if base == object {
                        self.raw_object_member(name)
                    } else {
                        self.own_class_attr(base, name)
                    }
                })
            };
            if let Some(member) = member {
                return self.bind_attribute(member, receiver, class);
            }
            if let Some((meta, member)) = metaclass_member {
                return self.bind_attribute(member, obj, meta);
            }
            return Err(self.missing_attribute(obj, name));
        }
        if name == "__delattr__"
            && self.is_type_object(obj)
            && self.allocation_base(obj).is_some_and(
                |base| matches!(self.heap.get(base), Value::Native(owner) if owner == "dict"),
            )
        {
            let member = self
                .class_attr(obj, name)
                .unwrap_or_else(|| self.builtin("object.__delattr__"));
            return self.bind_attribute(member, NONE, obj);
        }
        if let Value::Native(kind) = self.heap.get(obj)
            && stdlib::datetime::members(kind).is_some()
            && let Some(value) = self.class_attr(obj, name)
        {
            if matches!(self.heap.get(value), Value::Native(symbol) if stdlib::datetime::clock_classmethod(symbol))
            {
                return Ok(self.bind_native_classmethod(value, obj));
            }
            return Ok(value);
        }
        if let Value::Union(union) = *self.heap.get(obj) {
            match name {
                "__new__" => return Ok(self.builtin("object.__new__")),
                "__args__" => return Ok(union.args),
                "__parameters__" => {
                    return super::typing::builtin_parameters(
                        self,
                        obj,
                        union.args,
                        union.parameters,
                    );
                }
                "__class__" | "__origin__" => return Ok(super::union::class(self)),
                "__module__" => return Ok(self.text("typing")),
                "__name__" | "__qualname__" => return Ok(self.text("Union")),
                _ => {
                    let class = super::union::class(self);
                    if let Some(member) = self.own_class_attr(class, name) {
                        return self.bind_attribute(member, obj, class);
                    }
                }
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
            if let Some(value) = typing::subclass_alias_attribute(self, obj, class, name) {
                return value;
            }
            if matches!(name, "name" | "value") && self.enum_values(class).is_some() {
                let attr = self.class_attr(class, name);
                if attr.is_none_or(|attr| matches!(self.heap.get(attr), Value::Instance { class: owner, .. } if *owner == class)) {
                    let field = if name == "name" { "_name_" } else { "_value_" };
                    return self.lookup_attr(obj, field);
                }
            }
            let layout = self.instance_layout(class, name);
            if layout.active_slot {
                return inline.ok_or_else(|| self.missing_attribute(obj, name));
            }
            let member = self.instance_class_member(class, name);
            if name == "__dict__"
                && member.is_none()
                && layout.dictionary
                && !self.layout_descriptors_cleared(class)
            {
                return Ok(self.instance_dictionary(obj, class));
            }
            let inline = inline.filter(|_| !layout.declared_slot);
            let field = if inline.is_none()
                && let Some(dictionary) = namespace
            {
                if let Some(member) = member
                    && self.data_descriptor(member)
                {
                    return self.read_data_descriptor(member, obj, class, |ctx| {
                        ctx.dictionary_field(Some(dictionary), name)
                    });
                }
                self.dictionary_field(Some(dictionary), name)?
            } else {
                inline
            };
            if field.is_none()
                && let Some(member) = member
                && let AttributeLookup::Method(Some(receiver)) = &mut lookup
                && match self.heap.get(member) {
                    Value::Function { .. } => true,
                    // Direct writelines calls use the defining descriptor in argument errors.
                    Value::IoMethod { name, .. } => *name != "writelines",
                    _ => false,
                }
            {
                // The call target roots the callable. Its arguments can release the receiver.
                self.heap.reserve_temporaries(1)?;
                self.heap.keep(obj);
                **receiver = obj;
                return Ok(member);
            }
            if matches!(lookup, AttributeLookup::Method(_))
                && field.is_none()
                && let Some(member) = member
                && match self.heap.get(member) {
                    Value::Native(symbol) => {
                        !stdlib::datetime::clock_classmethod(symbol)
                            && BuiltinDescriptor::lookup(symbol)
                                .is_some_and(|slot| slot.class() != "classmethod_descriptor")
                    }
                    Value::IoMethod {
                        name: "writelines", ..
                    } => true,
                    _ => false,
                }
            {
                return Ok(self.alloc(Value::Method {
                    callable: member,
                    receiver: obj,
                    explicit: false,
                }));
            }
            return self.resolved_instance_attribute(obj, class, field, name, member);
        }
        if let Value::Raised(error) = self.heap.get(obj) {
            let dictionary = error.trace.as_ref().and_then(|trace| trace.dictionary);
            if let Some(member) = self.exception_member(obj, name) {
                let class = self.exact_type(obj)?;
                let field = if self.readable_data_descriptor(member) {
                    None
                } else {
                    self.dictionary_field(dictionary, name)?
                };
                return self.resolved_instance_attribute(obj, class, field, name, Some(member));
            }
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
            return self.builtin_attribute(obj, name, lookup, native_key);
        }
        if let Some(value) = super::paramspec::attribute(self, obj, name) {
            return value;
        }
        if let Some(value) = super::typing::attribute(self, obj, name) {
            return value;
        }
        if let Some(value) = super::array::attribute(self, obj, name) {
            return Ok(value);
        }
        if name == "__name__"
            && let Some(member) = self.wrapper_name(obj)
        {
            return self.heap.string_literal(member);
        }
        if matches!(name, "__get__" | "__reduce__" | "__reduce_ex__") {
            let class = match self.heap.get(obj) {
                Value::Native(kind)
                    if matches!(
                        kind.as_str(),
                        "method" | "builtin_function_or_method" | "method-wrapper"
                    ) =>
                {
                    Some(obj)
                }
                Value::Method { .. } | Value::Native(_)
                    if matches!(
                        self.builtin_type_name(obj),
                        Some("method" | "builtin_function_or_method" | "method-wrapper")
                    ) =>
                {
                    Some(self.exact_type(obj)?)
                }
                _ => None,
            };
            if let Some(class) = class
                && let Some(member) = self.class_attr(class, name)
            {
                let receiver = if obj == class { NONE } else { obj };
                return self.bind_attribute(member, receiver, class);
            }
        }
        match *self.heap.get(obj) {
            Value::Complex(z) if matches!(name, "real" | "imag") => {
                return Ok(self.alloc(Value::Float(if name == "real" { z.real } else { z.imag })));
            }
            Value::Raised(_) if builtins::EXCEPTION_GETSETS.contains(&name) => {
                return self.read_exception_getset(obj, name);
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
            Value::Native(ref class)
                if class == value::ZLIB_COMPRESS_TYPE
                    && value::ZLIB_COMPRESS_METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
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
            Value::FrameLocalsProxy { class, .. } => {
                return self.instance_attribute(obj, class, None, name);
            }
            Value::PythonFrame { .. } if name == "f_locals" => {
                return super::frame_locals::get(self, obj);
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
                    || builtins::CORE_RUNTIME_TYPES.contains(&class.as_str()))
                    && matches!(name, "__name__" | "__qualname__" | "__module__") =>
            {
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(class.clone())),
                    "__module__" => return Ok(self.text("builtins")),
                    _ => {}
                }
            }
            Value::AttrGetter(_) | Value::ItemGetter(_) | Value::MethodCaller { .. }
                if name == "__text_signature__" =>
            {
                return Ok(self.text("(obj, /)"));
            }
            Value::AttrGetter(_) | Value::ItemGetter(_) | Value::MethodCaller { .. }
                if name == "__call__" =>
            {
                let symbol = format!("{}.__call__", self.type_name(obj));
                let descriptor = self.builtin(&symbol);
                let class = self.exact_type(obj)?;
                return self.bind_attribute(descriptor, obj, class);
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
                let class = self.builtin(LRU_CACHE_CLASS);
                if let Some(member) = self.class_attr(class, name) {
                    return self.bind_attribute(member, obj, class);
                }
            }
            Value::ClassMethod {
                callable,
                dictionary,
            }
            | Value::StaticMethod {
                callable,
                dictionary,
            } => {
                if let Some(value) =
                    super::method_decorator::attribute(self, callable, dictionary, name)?
                {
                    return Ok(value);
                }
            }
            Value::None if name == "__doc__" => {
                return Ok(self.text("The type of the None singleton."));
            }
            Value::MemoryView {
                owner,
                anonymous,
                lease,
                ..
            } if matches!(
                name,
                "obj"
                    | "readonly"
                    | "nbytes"
                    | "itemsize"
                    | "format"
                    | "shape"
                    | "ndim"
                    | "strides"
                    | "suboffsets"
                    | "c_contiguous"
                    | "f_contiguous"
                    | "contiguous"
            ) =>
            {
                let owner = Self::memoryview_owner(owner)?;
                let view = self.view_layout(obj)?;
                let (format, width) = self.view_format_width(owner, &view);
                return Ok(match name {
                    "obj" => {
                        if let Some(lease) = lease {
                            let Value::PythonBufferLease { wrapper, .. } = *self.heap.get(lease)
                            else {
                                unreachable!()
                            };
                            wrapper
                        } else if anonymous {
                            NONE
                        } else {
                            owner
                        }
                    }
                    "readonly" => builtins::boolean(self.view_is_readonly(obj)?),
                    "ndim" => self.int(view.shape().len() as i64),
                    "strides" => {
                        let strides = view
                            .strides(width)
                            .into_iter()
                            .map(|n| self.int(n))
                            .collect();
                        self.alloc(Value::Tuple(strides))
                    }
                    "suboffsets" => self.alloc(Value::Tuple(vec![])),
                    "c_contiguous" | "contiguous" => builtins::boolean(view.is_contiguous()),
                    "f_contiguous" => builtins::boolean(view.is_f_contiguous()),
                    "nbytes" => self.int((view.len * width) as i64),
                    "itemsize" => self.int(width as i64),
                    "format" => self.text(format),
                    _ => {
                        let shape = view.shape().iter().map(|&n| self.int(n as i64)).collect();
                        self.alloc(Value::Tuple(shape))
                    }
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
            Value::Native(ref class) if class == "module" => {
                if let Some(member) = self.own_class_attr(obj, name) {
                    return Ok(member);
                }
                match name {
                    "__base__" => return Ok(self.builtin("object")),
                    "__name__" | "__qualname__" => return Ok(self.text("module")),
                    "__module__" => return Ok(self.text("builtins")),
                    _ => {}
                }
            }
            Value::SlotMember {
                owner, name: field, ..
            } => match name {
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
                "__doc__" if owner == "typing.TypeVar" => return Ok(NONE),
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
            Value::Native(ref class)
                if class == "member_descriptor"
                    && super::namedtuple::MEMBER_SLOTS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("member_descriptor.{name}")));
            }
            Value::TupleGetter { doc, .. } if name == "__doc__" => return Ok(doc.unwrap_or(NONE)),
            Value::LayoutDescriptor(..) | Value::GetSetDescriptor { .. } if name == "__doc__" => {
                return self.getset_documentation(obj);
            }
            Value::LayoutDescriptor(owner, field) => match name {
                "__objclass__" => return Ok(owner),
                "__name__" => return Ok(self.text(field)),
                "__qualname__" => {
                    let qualname = self.lookup_attr(owner, "__qualname__")?;
                    return Ok(self.text(format!("{}.{field}", self.str_value(qualname)?)));
                }
                "__get__" | "__set__" | "__delete__" => {
                    let class = self.builtin("getset_descriptor");
                    let method = self.own_class_attr(class, name).expect("getset wrapper");
                    return self.bind_attribute(method, obj, class);
                }
                _ => {}
            },
            Value::Property(property) => {
                if let Some(attribute) = property.attribute(self, name)? {
                    return Ok(attribute);
                }
            }
            Value::Native(ref symbol)
                if name == "__doc__"
                    && let Some(doc) = builtins::function_doc(symbol) =>
            {
                return Ok(self.text(doc));
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
            Value::Native(ref symbol) if symbol == "sorted" && name == "__text_signature__" => {
                return Ok(self.text("($module, iterable, /, *, key=None, reverse=False)"));
            }
            Value::Native(ref symbol) if symbol == "sorted" && name == "__self__" => {
                if let Some(module) = self.original_builtins {
                    return Ok(module);
                }
                return self.module("builtins");
            }
            Value::Native(ref symbol) if let Some((module, function)) = module_function(symbol) => {
                match name {
                    "__text_signature__"
                        if matches!(symbol.as_str(), "operator.is_" | "operator.is_not") =>
                    {
                        return Ok(self.text("($module, a, b, /)"));
                    }
                    "__doc__" if symbol == "operator.is_" => {
                        return Ok(self.text("Same as a is b."));
                    }
                    "__doc__" if symbol == "operator.is_not" => {
                        return Ok(self.text("Same as a is not b."));
                    }
                    "__text_signature__" if module == "time" => {
                        return Ok(stdlib::time::text_signature(function)
                            .map_or(NONE, |sig| self.text(sig)));
                    }
                    _ if module == "unicodedata"
                        && matches!(function, "category" | "bidirectional")
                        && let Some(metadata) =
                            stdlib::unicodedata::method_metadata(function, name) =>
                    {
                        return Ok(self.text(metadata));
                    }
                    "__self__" if module == "sys" => {
                        if let Some(original) = self.threads.system_module {
                            return Ok(original);
                        }
                    }
                    "__text_signature__"
                        if matches!(symbol.as_str(), "binascii.b2a_hex" | "binascii.hexlify") =>
                    {
                        return Ok(
                            self.text("($module, /, data, sep=<unrepresentable>, bytes_per_sep=1)")
                        );
                    }
                    "__text_signature__" if symbol == "binascii.b2a_qp" => {
                        return Ok(self.text(
                            "($module, /, data, quotetabs=False, istext=True, header=False)",
                        ));
                    }
                    "__doc__" if symbol == "binascii.b2a_qp" => {
                        return Ok(self.text(concat!(
                            "Encode a string using quoted-printable encoding.\n\n",
                            "On encoding, when istext is set, newlines are not encoded, and white\n",
                            "space at end of lines is.  When istext is not set, \\r and \\n (CR/LF)\n",
                            "are both encoded.  When quotetabs is set, space and tabs are encoded."
                        )));
                    }
                    "__text_signature__" if symbol == "binascii.a2b_qp" => {
                        return Ok(self.text("($module, /, data, header=False)"));
                    }
                    "__doc__" if symbol == "binascii.a2b_qp" => {
                        return Ok(self.text("Decode a string of qp-encoded data."));
                    }
                    "__text_signature__" if symbol == "sys.set_coroutine_origin_tracking_depth" => {
                        return Ok(self.text("($module, /, depth)"));
                    }
                    "__doc__" if symbol == "sys._get_cpu_count_config" => {
                        return Ok(self.text("Private function for getting PyConfig.cpu_count"));
                    }
                    "__doc__" if symbol == "sys.get_coroutine_origin_tracking_depth" => {
                        return Ok(self.text(
                            "Check status of origin tracking for coroutine objects in this thread.",
                        ));
                    }
                    "__doc__" if symbol == "sys.set_coroutine_origin_tracking_depth" => {
                        return Ok(self.text("Enable or disable origin tracking for coroutine objects in this thread.\n\nCoroutine objects will track 'depth' frames of traceback information\nabout where they came from, available in their cr_origin attribute.\n\nSet a depth of 0 to disable."));
                    }
                    "__text_signature__" if symbol == "_typing._idfunc" => {
                        return Ok(self.text("($module, x, /)"));
                    }
                    "__doc__" if symbol == "operator.call" => {
                        return Ok(self.text("Same as obj(*args, **kwargs)."));
                    }
                    "__text_signature__" if symbol == "operator.call" => {
                        return Ok(self.text("($module, obj, /, *args, **kwargs)"));
                    }
                    "__doc__" if symbol == "operator.not_" => {
                        return Ok(self.text("Same as not a."));
                    }
                    "__text_signature__" if symbol == "operator.not_" => {
                        return Ok(self.text("($module, a, /)"));
                    }
                    "__doc__" if symbol == "posix.getgrouplist" => {
                        return Ok(self.text("Returns a list of groups to which a user belongs.\n\n  user\n    username to lookup\n  group\n    base group id of the user"));
                    }
                    "__text_signature__" if symbol == "posix.getgrouplist" => {
                        return Ok(self.text("($module, user, group, /)"));
                    }
                    "__doc__" if symbol == "_typing._idfunc" => return Ok(NONE),
                    "__name__" | "__qualname__" => return Ok(self.text(function.to_owned())),
                    "__module__" => return Ok(self.text(module.to_owned())),
                    "__doc__" if symbol == "_signal.pause" => {
                        return Ok(self.text("Wait until a signal arrives."));
                    }
                    "__doc__" if symbol == "_signal.signal" => {
                        return Ok(self.text("Set the action for the given signal.\n\nThe action can be SIG_DFL, SIG_IGN, or a callable Python object.\nThe previous action is returned.  See getsignal() for possible return values.\n\n*** IMPORTANT NOTICE ***\nA signal handler function is called with two arguments:\nthe first is the signal number, the second is the interrupted stack frame."));
                    }
                    "__text_signature__" if symbol == "_signal.signal" => {
                        return Ok(self.text("($module, signalnum, handler, /)"));
                    }
                    "__doc__" if symbol == "_signal.getsignal" => {
                        return Ok(self.text("Return the current action for the given signal.\n\nThe return value can be:\n  SIG_IGN -- if the signal is being ignored\n  SIG_DFL -- if the default action for the signal is in effect\n  None    -- if an unknown handler is in effect\n  anything else -- the callable Python object used as a handler"));
                    }
                    "__text_signature__" if symbol == "_signal.getsignal" => {
                        return Ok(self.text("($module, signalnum, /)"));
                    }
                    "__text_signature__" if symbol == "_signal.set_wakeup_fd" => {
                        return Ok(self.text("($module, fd, /, *, warn_on_full_buffer=True)"));
                    }
                    "__doc__" if symbol == "_signal.set_wakeup_fd" => {
                        return Ok(self.text("Sets the fd to be written to (with the signal number) when a signal comes in.\n\nA library can use this to wakeup select or poll.\nThe previous fd or -1 is returned.\n\nThe fd must be non-blocking."));
                    }
                    "__text_signature__" if symbol == "_signal.siginterrupt" => {
                        return Ok(self.text("($module, signalnum, flag, /)"));
                    }
                    "__doc__" if symbol == "_signal.siginterrupt" => {
                        return Ok(self.text("Change system call restart behaviour.\n\nIf flag is False, system calls will be restarted when interrupted by\nsignal sig, else system calls will be interrupted."));
                    }
                    "__doc__" if symbol == "_signal.default_int_handler" => {
                        return Ok(self.text("The default handler for SIGINT installed by Python.\n\nIt raises KeyboardInterrupt."));
                    }
                    "__text_signature__" if symbol == "_signal.default_int_handler" => {
                        return Ok(self.text("($module, signalnum, frame, /)"));
                    }
                    "__doc__" if symbol == "_signal.valid_signals" => {
                        return Ok(self.text("Return a set of valid signal numbers on this platform.\n\nThe signal numbers returned by this function can be safely passed to\nfunctions like `pthread_sigmask`."));
                    }
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
                    "__doc__" if symbol == "posix.sched_get_priority_min" => {
                        return Ok(self.text("Get the minimum scheduling priority for policy."));
                    }
                    "__doc__" if symbol == "posix.sched_get_priority_max" => {
                        return Ok(self.text("Get the maximum scheduling priority for policy."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "posix.sched_get_priority_min" | "posix.sched_get_priority_max"
                        ) =>
                    {
                        return Ok(self.text("($module, /, policy)"));
                    }
                    "__doc__" if symbol == "posix.sched_yield" => {
                        return Ok(self.text("Voluntarily relinquish the CPU."));
                    }
                    "__text_signature__" if symbol == "posix.sched_yield" => {
                        return Ok(self.text("($module, /)"));
                    }
                    "__doc__" if symbol == "posix.sched_getaffinity" => {
                        return Ok(self.text("Return the affinity of the process identified by pid (or the current process if zero).\n\nThe affinity is returned as a set of CPU identifiers."));
                    }
                    "__doc__" if symbol == "posix.sched_setaffinity" => {
                        return Ok(self.text("Set the CPU affinity of the process identified by pid to mask.\n\nmask should be an iterable of integers identifying CPUs."));
                    }
                    "__doc__" if symbol == "posix.sched_rr_get_interval" => {
                        return Ok(self.text("Return the round-robin quantum for the process identified by pid, in seconds.\n\nValue returned is a float."));
                    }
                    "__doc__" if symbol == "posix.sched_getparam" => {
                        return Ok(self.text("Returns scheduling parameters for the process identified by pid.\n\nIf pid is 0, returns parameters for the calling process.\nReturn value is an instance of sched_param."));
                    }
                    "__doc__" if symbol == "posix.sched_setparam" => {
                        return Ok(self.text("Set scheduling parameters for the process identified by pid.\n\nIf pid is 0, sets parameters for the calling process.\nparam should be an instance of sched_param."));
                    }
                    "__doc__" if symbol == "posix.sched_setscheduler" => {
                        return Ok(self.text("Set the scheduling policy for the process identified by pid.\n\nIf pid is 0, the calling process is changed.\nparam is an instance of sched_param."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "posix.sched_getaffinity"
                                | "posix.sched_rr_get_interval"
                                | "posix.sched_getparam"
                        ) =>
                    {
                        return Ok(self.text("($module, pid, /)"));
                    }
                    "__text_signature__" if symbol == "posix.sched_setaffinity" => {
                        return Ok(self.text("($module, pid, mask, /)"));
                    }
                    "__text_signature__" if symbol == "posix.sched_setparam" => {
                        return Ok(self.text("($module, pid, param, /)"));
                    }
                    "__text_signature__" if symbol == "posix.sched_setscheduler" => {
                        return Ok(self.text("($module, pid, policy, param, /)"));
                    }
                    "__doc__" if symbol == "posix.sched_getscheduler" => {
                        return Ok(self.text(concat!(
                            "Get the scheduling policy for the process identified by pid.\n\n",
                            "Passing 0 for pid returns the scheduling policy for the calling process.",
                        )));
                    }
                    "__text_signature__" if symbol == "posix.sched_getscheduler" => {
                        return Ok(self.text("($module, pid, /)"));
                    }
                    "__doc__" if symbol == "posix.waitid" => {
                        return Ok(self.text(concat!(
                            "Returns the result of waiting for a process or processes.\n\n",
                            "  idtype\n    Must be one of be P_PID, P_PGID or P_ALL.\n",
                            "  id\n    The id to wait on.\n",
                            "  options\n    Constructed from the ORing of one or more of WEXITED, WSTOPPED\n",
                            "    or WCONTINUED and additionally may be ORed with WNOHANG or WNOWAIT.\n\n",
                            "Returns either waitid_result or None if WNOHANG is specified and there are\n",
                            "no children in a waitable state."
                        )));
                    }
                    "__text_signature__" if symbol == "posix.waitid" => {
                        return Ok(self.text("($module, idtype, id, options, /)"));
                    }
                    "__doc__" if symbol == "posix.uname" => {
                        return Ok(self.text(concat!(
                            "Return an object identifying the current operating system.\n\n",
                            "The object behaves like a named tuple with the following fields:\n",
                            "  (sysname, nodename, release, version, machine)"
                        )));
                    }
                    "__doc__" if symbol == "posix.getloadavg" => {
                        return Ok(self.text(concat!(
                            "Return average recent system load information.\n\n",
                            "Return the number of processes in the system run queue averaged over\n",
                            "the last 1, 5, and 15 minutes as a tuple of three floats.\n",
                            "Raises OSError if the load average was unobtainable."
                        )));
                    }
                    "__doc__" if symbol == "posix.ctermid" => {
                        return Ok(self.text(
                            "Return the name of the controlling terminal for this process.",
                        ));
                    }
                    "__doc__" if symbol == "posix.sync" => {
                        return Ok(self.text("Force write of everything to disk."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "posix.uname" | "posix.getloadavg" | "posix.ctermid" | "posix.sync"
                        ) =>
                    {
                        return Ok(self.text("($module, /)"));
                    }
                    "__doc__" if symbol == "posix.getrandom" => {
                        return Ok(self.text("Obtain a series of random bytes."));
                    }
                    "__text_signature__" if symbol == "posix.getrandom" => {
                        return Ok(self.text("($module, /, size, flags=0)"));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "_socket.htons" | "_socket.ntohs" | "_socket.htonl" | "_socket.ntohl"
                        ) =>
                    {
                        return Ok(self.text("($module, integer, /)"));
                    }
                    "__text_signature__" if symbol == "_socket.inet_aton" => {
                        return Ok(self.text("($module, ip_addr, /)"));
                    }
                    "__doc__" if symbol == "_socket.inet_aton" => {
                        return Ok(self.text("Convert an IP address in string format (123.45.67.89) to the 32-bit packed binary format used in low-level network functions."));
                    }
                    "__doc__" if symbol == "_socket.getprotobyname" => {
                        return Ok(self.text("getprotobyname(name) -> integer\n\nReturn the protocol number for the named protocol.  (Rarely used.)"));
                    }
                    "__doc__" if symbol == "_socket.inet_pton" => {
                        return Ok(self.text("inet_pton(af, ip) -> packed IP address string\n\nConvert an IP address from string format to a packed string suitable\nfor use with low-level network functions."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "_socket.getprotobyname" | "_socket.inet_pton" | "_socket.inet_ntop"
                        ) =>
                    {
                        return Ok(NONE);
                    }
                    "__text_signature__" if symbol == "_socket.inet_ntoa" => {
                        return Ok(self.text("($module, packed_ip, /)"));
                    }
                    "__doc__" if symbol == "_socket.inet_ntop" => {
                        return Ok(self.text("inet_ntop(af, packed_ip) -> string formatted IP address\n\nConvert a packed IP address of the given family to string format."));
                    }
                    "__doc__" if symbol == "_socket.inet_ntoa" => {
                        return Ok(self.text("Convert an IP address from 32-bit packed binary format to string format."));
                    }
                    "__doc__" if symbol == "posix.urandom" => {
                        return Ok(self.text("Return a bytes object containing random bytes suitable for cryptographic use."));
                    }
                    "__text_signature__" if symbol == "posix.urandom" => {
                        return Ok(self.text("($module, size, /)"));
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
                    "__doc__" if module == "resource" => return Ok(NONE),
                    "__text_signature__" if symbol == "resource.setrlimit" => {
                        return Ok(self.text("($module, resource, limits, /)"));
                    }
                    "__text_signature__" if symbol == "resource.getrlimit" => {
                        return Ok(self.text("($module, resource, /)"));
                    }
                    "__doc__" if symbol == "posix.getegid" => {
                        return Ok(self.text("Return the current process's effective group id."));
                    }
                    "__doc__" if symbol == "posix.setgroups" => {
                        return Ok(self.text("Set the groups of the current process to list."));
                    }
                    "__text_signature__" if symbol == "posix.setgroups" => {
                        return Ok(self.text("($module, groups, /)"));
                    }
                    "__doc__" if symbol == "posix.setregid" => {
                        return Ok(
                            self.text("Set the current process's real and effective group ids.")
                        );
                    }
                    "__text_signature__" if symbol == "posix.setregid" => {
                        return Ok(self.text("($module, rgid, egid, /)"));
                    }
                    "__doc__" if symbol == "posix.setreuid" => {
                        return Ok(
                            self.text("Set the current process's real and effective user ids.")
                        );
                    }
                    "__text_signature__" if symbol == "posix.setreuid" => {
                        return Ok(self.text("($module, ruid, euid, /)"));
                    }
                    "__doc__" if symbol == "posix.setresgid" => {
                        return Ok(self.text(
                            "Set the current process's real, effective, and saved group ids.",
                        ));
                    }
                    "__text_signature__" if symbol == "posix.setresgid" => {
                        return Ok(self.text("($module, rgid, egid, sgid, /)"));
                    }
                    "__doc__" if symbol == "posix.setresuid" => {
                        return Ok(self.text(
                            "Set the current process's real, effective, and saved user ids.",
                        ));
                    }
                    "__text_signature__" if symbol == "posix.setresuid" => {
                        return Ok(self.text("($module, ruid, euid, suid, /)"));
                    }
                    "__doc__" if symbol == "posix.getresgid" => {
                        return Ok(self.text("Return a tuple of the current process's real, effective, and saved group ids."));
                    }
                    "__doc__" if symbol == "posix.getresuid" => {
                        return Ok(self.text("Return a tuple of the current process's real, effective, and saved user ids."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "posix.getegid" | "posix.getresgid" | "posix.getresuid"
                        ) =>
                    {
                        return Ok(self.text("($module, /)"));
                    }
                    "__text_signature__" if symbol == "gc.is_tracked" => {
                        return Ok(self.text("($module, obj, /)"));
                    }
                    "__text_signature__" if symbol == "gc.set_threshold" => return Ok(NONE),
                    "__doc__" if module == "gc" => {
                        let doc = match function {
                            "enable" => "Enable automatic garbage collection.",
                            "disable" => "Disable automatic garbage collection.",
                            "isenabled" => {
                                "Returns true if automatic garbage collection is enabled."
                            }
                            "get_threshold" => "Return the current collection thresholds.",
                            "is_tracked" => {
                                "Returns true if the object is tracked by the garbage collector.\n\nSimple atomic objects will return false."
                            }
                            "set_threshold" => {
                                "set_threshold(threshold0, [threshold1, [threshold2]])\nSet the collection thresholds (the collection frequency).\n\nSetting 'threshold0' to zero disables collection."
                            }
                            _ => unreachable!("registered GC control"),
                        };
                        return Ok(self.text(doc));
                    }
                    "__doc__" if symbol == "posix.abort" => {
                        return Ok(self.text("Abort the interpreter immediately.\n\nThis function 'dumps core' or otherwise fails in the hardest way possible\non the hosting operating system.  This function never returns."));
                    }
                    "__doc__" if symbol == "posix.times" => {
                        return Ok(self.text(concat!(
                            "Return a collection containing process timing information.\n\n",
                            "The object returned behaves like a named tuple with these fields:\n",
                            "  (utime, stime, cutime, cstime, elapsed_time)\n",
                            "All fields are floating-point numbers."
                        )));
                    }
                    "__doc__" if symbol == "posix.getpgrp" => {
                        return Ok(self.text("Return the current process group id."));
                    }
                    "__doc__" if symbol == "posix.getpgid" => {
                        return Ok(
                            self.text("Call the system call getpgid(), and return the result.")
                        );
                    }
                    "__text_signature__" if symbol == "posix.getpgid" => {
                        return Ok(self.text("($module, /, pid)"));
                    }
                    "__doc__" if symbol == "posix.pidfd_open" => {
                        return Ok(self.text("Return a file descriptor referring to the process *pid*.\n\nThe descriptor can be used to perform process management without races and\nsignals."));
                    }
                    "__text_signature__" if symbol == "posix.pidfd_open" => {
                        return Ok(self.text("($module, /, pid, flags=0)"));
                    }
                    "__doc__" if symbol == "posix.setpgid" => {
                        return Ok(self.text("Call the system call setpgid(pid, pgrp)."));
                    }
                    "__text_signature__" if symbol == "posix.setpgid" => {
                        return Ok(self.text("($module, pid, pgrp, /)"));
                    }
                    "__doc__" if symbol == "posix.kill" => {
                        return Ok(self.text("Kill a process with a signal."));
                    }
                    "__text_signature__" if symbol == "posix.kill" => {
                        return Ok(self.text("($module, pid, signal, /)"));
                    }
                    "__doc__" if symbol == "posix.WIFSTOPPED" => {
                        return Ok(
                            self.text("Return True if the process returning status was stopped.")
                        );
                    }
                    "__text_signature__" if symbol == "posix.WIFSTOPPED" => {
                        return Ok(self.text("($module, /, status)"));
                    }
                    "__doc__" if symbol == "posix.execv" => {
                        return Ok(self.text("Execute an executable path with arguments, replacing current process.\n\n  path\n    Path of executable file.\n  argv\n    Tuple or list of strings."));
                    }
                    "__text_signature__" if symbol == "posix.execv" => {
                        return Ok(self.text("($module, path, argv, /)"));
                    }
                    "__doc__" if symbol == "posix.execve" => {
                        return Ok(self.text("Execute an executable path with arguments, replacing current process.\n\n  path\n    Path of executable file.\n  argv\n    Tuple or list of strings.\n  env\n    Dictionary of strings mapping to strings."));
                    }
                    "__text_signature__" if symbol == "posix.execve" => {
                        return Ok(self.text("($module, /, path, argv, env)"));
                    }
                    "__doc__" if symbol == "posix.getppid" => {
                        return Ok(self.text("Return the parent's process id.\n\nIf the parent process has already exited, Windows machines will still\nreturn its id; others systems will return the id of the 'init' process (1)."));
                    }
                    "__doc__" if symbol == "posix.cpu_count" => {
                        return Ok(self.text("Return the number of logical CPUs in the system.\n\nReturn None if indeterminable."));
                    }
                    "__doc__" if symbol == "posix.getpid" => {
                        return Ok(self.text("Return the current process id."));
                    }
                    "__text_signature__"
                        if matches!(
                            symbol.as_str(),
                            "posix.abort"
                                | "posix.getpid"
                                | "posix.cpu_count"
                                | "posix.getppid"
                                | "posix.getpgrp"
                                | "posix.times"
                                | "gc.enable"
                                | "gc.disable"
                                | "gc.isenabled"
                                | "gc.get_threshold"
                                | "_signal.pause"
                                | "_signal.sigpending"
                                | "_signal.valid_signals"
                                | "_thread.allocate"
                                | "_thread.allocate_lock"
                                | "_thread.exit"
                                | "_thread.exit_thread"
                                | "_thread._count"
                                | "_thread._shutdown"
                                | "sys._get_cpu_count_config"
                                | "sys.get_coroutine_origin_tracking_depth"
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
                if super::generator_protocol::coroutine_method(class, name)
                    || super::async_generator::method(class, name) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if class == "generator" && super::generator_protocol::generator_method(name) =>
            {
                return Ok(self.builtin(&format!("generator.{name}")));
            }
            Value::Native(ref class) if class == "type" && name == "__text_signature__" => {
                return Ok(NONE);
            }
            Value::Native(ref class) if class == "object" && name == "__text_signature__" => {
                return Ok(self.text("()"));
            }
            Value::Native(ref class) if class == "tuple" && name == "__text_signature__" => {
                return Ok(self.text("(iterable=(), /)"));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "classmethod" | "staticmethod")
                    && name == "__text_signature__" =>
            {
                return Ok(self.text("(function, /)"));
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "classmethod" | "staticmethod" | "functools.partial"
                ) && name == "__get__" =>
            {
                return Ok(self.builtin(&format!("{class}.__get__")));
            }
            Value::Native(ref class)
                if class == "property" && super::property::METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("property.{name}")));
            }
            Value::Native(ref class)
                if class == "code"
                    && let Some(descriptor) = self.own_class_attr(obj, name) =>
            {
                return Ok(descriptor);
            }
            Value::Code(ref code) => match name {
                "co_argcount" | "co_posonlyargcount" | "co_kwonlyargcount" => {
                    if code.parameters == NONE {
                        return Ok(self.int(0));
                    }
                    let parameters = self.parsed_parameters(obj)?;
                    let count = match name {
                        "co_argcount" => parameters.argcount,
                        "co_posonlyargcount" => parameters.positional_only as i64,
                        _ => parameters.keyword_only.len() as i64,
                    };
                    return Ok(self.int(count));
                }
                "co_varnames" | "co_cellvars" | "co_nlocals" | "co_freevars" => {
                    return self.code_local_field(obj, name);
                }
                "co_flags" => {
                    if !code.definition.flags_complete {
                        return Err(Error::new(
                            "NotImplementedError",
                            "co_flags unavailable for this code object",
                        ));
                    }
                    return Ok(self.int(i64::from(code.definition.flags)));
                }
                "co_name" => return Ok(code.name),
                "co_qualname" => return Ok(code.qualname),
                "co_filename" => return Ok(code.filename),
                "co_firstlineno" => return Ok(self.int(i64::from(code.first_line))),
                _ => {}
            },
            Value::Cell(contents) if name == "cell_contents" => {
                return contents.ok_or_else(|| Error::new("ValueError", "Cell is empty"));
            }
            Value::Generator {
                name: simple,
                qualname,
                code,
                running,
                offset,
                ..
            } => match name {
                "__name__" => return Ok(simple),
                "__qualname__" => return Ok(qualname),
                "gi_running" => return Ok(builtins::boolean(running)),
                "gi_suspended" => {
                    return Ok(builtins::boolean(
                        !running && offset.is_some_and(|ip| ip != 0),
                    ));
                }
                "gi_code" | "gi_frame" => {
                    if !self.audit_hooks.is_empty() {
                        let attribute = self.text(name);
                        stdlib::audit::event(self, "object.__getattr__", &[obj, attribute])?;
                    }
                    return if name == "gi_code" {
                        Ok(code)
                    } else {
                        self.generator_frame(obj)
                    };
                }
                _ => {}
            },
            Value::AsyncGenerator { frame, running, .. } => {
                let Value::Generator {
                    code,
                    offset,
                    running: executing,
                    ..
                } = *self.heap.get(frame)
                else {
                    unreachable!()
                };
                match name {
                    "ag_running" => return Ok(builtins::boolean(running)),
                    "ag_suspended" => {
                        return Ok(builtins::boolean(
                            !executing && offset.is_some_and(|ip| ip != 0),
                        ));
                    }
                    "ag_code" | "ag_frame" => {
                        if !self.audit_hooks.is_empty() {
                            let attribute = self.text(name);
                            stdlib::audit::event(self, "object.__getattr__", &[obj, attribute])?;
                        }
                        return if name == "ag_code" {
                            Ok(code)
                        } else {
                            self.generator_frame(frame)
                        };
                    }
                    _ => {}
                }
            }
            Value::Coroutine { origin, .. } if name == "cr_origin" => return Ok(origin),
            Value::Coroutine { frame, .. } if matches!(name, "__name__" | "__qualname__") => {
                return self.default_get_attr(frame, name);
            }
            Value::Native(ref class) if class == "coroutine" && name == "cr_origin" => {
                return Ok(self.readonly_member("coroutine", "cr_origin"));
            }
            Value::Coroutine { frame, .. } if name == "cr_running" => {
                return self.default_get_attr(frame, "gi_running");
            }
            Value::Coroutine { frame, .. } if name == "cr_suspended" => {
                return self.default_get_attr(frame, "gi_suspended");
            }
            Value::Coroutine { frame, .. } if name == "cr_frame" => {
                if !self.audit_hooks.is_empty() {
                    let attribute = self.text(name);
                    stdlib::audit::event(self, "object.__getattr__", &[obj, attribute])?;
                }
                return self.generator_frame(frame);
            }
            Value::Coroutine { frame, .. } if name == "cr_code" => {
                let Value::Generator { code, .. } = *self.heap.get(frame) else {
                    unreachable!("coroutine frame")
                };
                return Ok(code);
            }
            Value::Native(ref class)
                if class == "function" && matches!(name, "__code__" | "__get__") =>
            {
                if let Some(descriptor) = self.own_class_attr(obj, name) {
                    return Ok(descriptor);
                }
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
                    if !code.definition.closure_supported && !code.definition.module_definition {
                        return Err(Error::new(
                            "NotImplementedError",
                            "code lacks closure metadata",
                        ));
                    }
                    if closure == NONE && self.scope_data(scope).closure.environment {
                        let cells = self
                            .scope_data(scope)
                            .closure
                            .cells
                            .values()
                            .copied()
                            .collect();
                        let tuple = self.alloc(Value::Tuple(cells));
                        let Value::Function { closure, .. } = self.heap.get_mut(obj) else {
                            unreachable!()
                        };
                        *closure = tuple;
                        return Ok(tuple);
                    }
                    return Ok(closure);
                }
                if name == "__globals__" {
                    return self.module_globals(scope);
                }
                if name == "__builtins__" {
                    return self.function_builtins(scope);
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
                callable,
                receiver,
                explicit,
            } if explicit || matches!(self.heap.get(callable), Value::Function { .. }) => {
                return match name {
                    "__self__" => Ok(receiver),
                    "__func__" => Ok(callable),
                    "__init__" => self.builtin_attribute(obj, name, lookup, native_key),
                    _ => self.lookup_attr(callable, name),
                };
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
                    let callable = self.builtin(&format!("{}.__get__", slot.class()));
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: false,
                    }));
                }
            }
            Value::Method {
                callable, receiver, ..
            } => match name {
                "__self__" => return Ok(receiver),
                _ if let Value::HeapTypeMethod {
                    owner,
                    name: method,
                    slot_wrapper,
                } = *self.heap.get(callable) =>
                {
                    if self.heap.get(owner).heap_type() == Some(stdlib::unicodedata::CLASS)
                        && matches!(name, "__doc__" | "__text_signature__")
                    {
                        return self.lookup_attr(callable, name);
                    }
                    if self.heap.get(owner).heap_type()
                        == Some(stdlib::datetime::isocalendar::CLASS)
                    {
                        if let Some(value) =
                            stdlib::datetime::isocalendar::method_metadata(self, method, name)
                        {
                            return Ok(value);
                        }
                        if name == "__module__" && matches!(method, "__new__" | "__reduce__") {
                            return Ok(NONE);
                        }
                    }
                    match name {
                        "__doc__" if method == "__str__" => {
                            return Ok(self.text("Return str(self)."));
                        }
                        "__text_signature__" if method == "__str__" => {
                            return Ok(self.text("($self, /)"));
                        }
                        "__objclass__" if slot_wrapper => return Ok(owner),
                        "__name__" => return Ok(self.text(method)),
                        "__qualname__" if method == "md5" => return Ok(self.text(method)),
                        "__qualname__" => {
                            return Ok(self.text(format!("{}.{method}", self.class_name(owner))));
                        }
                        "__module__" if method == "md5" => return Ok(self.text("_md5")),
                        "__text_signature__" if method == "md5" => {
                            return Ok(self.text(
                                "($module, /, data=b'', *, usedforsecurity=True, string=None)",
                            ));
                        }
                        "__doc__" if method == "md5" => return Ok(self.text(
                            "Return a new MD5 hash object; optionally initialized with a string.",
                        )),
                        _ => {}
                    }
                }
                _ if matches!(self.heap.get(callable), Value::IoOpen { .. }) => {
                    let original = self.builtin("open");
                    return self.lookup_attr(original, name);
                }
                _ => {
                    if let Value::Native(method) = self.heap.get(callable) {
                        let module_bound = matches!(self.heap.get(receiver), Value::Module { .. });
                        if matches!(name, "__doc__" | "__text_signature__")
                            && let Some((signature, doc)) = method
                                .strip_prefix("_warnings.")
                                .and_then(stdlib::warnings_accel::method_metadata)
                                .or_else(|| {
                                    method
                                        .strip_prefix("_bisect.")
                                        .and_then(stdlib::bisect::method_metadata)
                                })
                                .or_else(|| {
                                    method
                                        .strip_prefix("_tracemalloc.")
                                        .and_then(stdlib::tracemalloc::method_metadata)
                                })
                        {
                            return Ok(if name == "__doc__" {
                                doc.map_or(NONE, |text| self.text(text))
                            } else {
                                self.text(signature)
                            });
                        }
                        #[cfg(unix)]
                        if module_bound
                            && let Some(text) =
                                stdlib::ssl::provider::objects::function_metadata(method, name)
                        {
                            return Ok(self.text(text));
                        }
                        let value = match name {
                            "__doc__" if method == "functools.cmp_to_key" => Some(
                                "Convert a cmp= function into a key= function.\n\n  mycmp\n    Function that compares two objects.",
                            ),
                            "__text_signature__" if method == "functools.cmp_to_key" => {
                                Some("($module, /, mycmp)")
                            }
                            "__doc__" if method == "_functools.reduce" => Some(concat!(
                                "Apply a function of two arguments cumulatively to the items of an iterable, from left to right.\n\n",
                                "This effectively reduces the iterable to a single value.  If initial is present,\n",
                                "it is placed before the items of the iterable in the calculation, and serves as\n",
                                "a default when the iterable is empty.\n\n",
                                "For example, reduce(lambda x, y: x+y, [1, 2, 3, 4, 5])\n",
                                "calculates ((((1 + 2) + 3) + 4) + 5)."
                            )),
                            "__text_signature__" if method == "_functools.reduce" => {
                                Some("($module, function, iterable, /, initial=<unrepresentable>)")
                            }
                            "__doc__" if method == "_locale.localeconv" => {
                                Some("Returns numeric and monetary locale-specific parameters.")
                            }
                            "__text_signature__" if method == "_locale.localeconv" => {
                                Some("($module, /)")
                            }
                            "__doc__" if method == "posix.sysconf" => {
                                Some("Return an integer-valued system configuration variable.")
                            }
                            "__doc__" if method == "posix.confstr" => {
                                Some("Return a string-valued system configuration variable.")
                            }
                            "__text_signature__"
                                if matches!(method.as_str(), "posix.sysconf" | "posix.confstr") =>
                            {
                                Some("($module, name, /)")
                            }
                            "__qualname__" if module_bound => method.rsplit('.').next(),
                            "__module__" if module_bound => {
                                method.rsplit_once('.').map(|(module, _)| module)
                            }
                            "__text_signature__"
                                if matches!(
                                    method.as_str(),
                                    "_operator._compare_digest" | "_hashlib.compare_digest"
                                ) =>
                            {
                                Some("($module, a, b, /)")
                            }
                            "__doc__"
                                if matches!(
                                    method.as_str(),
                                    "_operator._compare_digest" | "_hashlib.compare_digest"
                                ) =>
                            {
                                Some(concat!(
                                    "Return 'a == b'.\n\n",
                                    "This function uses an approach designed to prevent\n",
                                    "timing analysis, making it appropriate for cryptography.\n\n",
                                    "a and b must both be of the same type: either str (ASCII only),\n",
                                    "or any bytes-like object.\n\n",
                                    "Note: If a and b are of different lengths, or if an error occurs,\n",
                                    "a timing attack could theoretically reveal information about the\n",
                                    "types and lengths of a and b--but not their values."
                                ))
                            }
                            "__doc__" if method == "mappingproxy.__class_getitem__" => {
                                Some("See PEP 585")
                            }
                            "__text_signature__" if method == "mappingproxy.__class_getitem__" => {
                                Some("($type, object, /)")
                            }
                            "__text_signature__" if method == "sys._getframe" => {
                                Some("($module, depth=0, /)")
                            }
                            "__text_signature__" if method == "sys._getframemodulename" => {
                                Some("($module, /, depth=0)")
                            }
                            "__doc__" if let Some(name) = method.strip_prefix("_opcode.") => {
                                stdlib::opcode::doc(name)
                            }
                            "__text_signature__"
                                if let Some(name) = method.strip_prefix("_opcode.") =>
                            {
                                Some(stdlib::opcode::signature(name))
                            }
                            "__text_signature__" if method == "sys.excepthook" => {
                                Some("($module, exctype, value, traceback, /)")
                            }
                            "__doc__" if method == "sys.excepthook" => Some(
                                "Handle an exception by displaying it with a traceback on sys.stderr.",
                            ),
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
            Value::RegexMatch(ref m) if matches!(name, "pos" | "endpos") => {
                return std::rc::Rc::clone(m).window_position(self, name == "endpos");
            }
            Value::RegexMatch(ref m) if name == "regs" => return std::rc::Rc::clone(m).regs(self),
            Value::RegexMatch(ref m) if name == "re" => return Ok(m.pattern),
            Value::RegexMatch(ref m) if name == "string" => return Ok(m.subject),
            Value::RegexMatch(ref m) if matches!(name, "lastindex" | "lastgroup") => {
                return std::rc::Rc::clone(m).last_attribute(self, name == "lastgroup");
            }
            Value::RegexPattern(ref pattern) if name == "flags" => {
                return Ok(self.int(pattern.flags));
            }
            Value::RegexPattern(ref pattern) if name == "groups" => {
                return Ok(self.int(pattern.group_count() as i64));
            }
            Value::RegexPattern(ref pattern) if name == "groupindex" => {
                return pattern.clone().group_index(self);
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
            Value::Native(ref class)
                if name == "__getitem__" && Self::native_base(class) == Some("tuple") =>
            {
                return Ok(self.builtin("tuple.__getitem__"));
            }
            Value::Native(ref class) if let Some(layout) = stdlib::structseq::layout(class) => {
                if let Some(member) = self.own_class_attr(obj, name) {
                    return Ok(member);
                }
                match name {
                    "__bases__" | "__mro__" => return Ok(self.builtin_hierarchy_tuple(obj, name)),
                    method if layout.methods().contains(&method) => {
                        return Ok(self.builtin(&format!("{class}.{method}")));
                    }
                    field if let Some(id) = stdlib::structseq::member(self, layout, field) => {
                        return Ok(id);
                    }
                    "__module__" => return Ok(self.text(layout.class.rsplit_once('.').unwrap().0)),
                    "__name__" | "__qualname__" => return Ok(self.text(layout.name())),
                    "__base__" => return Ok(self.builtin("tuple")),
                    "__match_args__" => {
                        let names = layout
                            .match_fields()
                            .iter()
                            .map(|&n| self.text(n))
                            .collect();
                        return Ok(self.alloc(Value::Tuple(names)));
                    }
                    field if let Some(n) = stdlib::structseq::count_attribute(layout, field) => {
                        return Ok(self.int(n));
                    }
                    _ => {}
                }
            }
            Value::HeapTypeMethod {
                owner,
                name: member,
                slot_wrapper,
            } => match name {
                _ if self.heap.get(owner).heap_type() == Some(stdlib::unicodedata::CLASS)
                    && let Some(metadata) = stdlib::unicodedata::method_metadata(member, name) =>
                {
                    return Ok(self.text(metadata));
                }
                _ if self.heap.get(owner).heap_type()
                    == Some(stdlib::datetime::isocalendar::CLASS)
                    && let Some(value) =
                        stdlib::datetime::isocalendar::method_metadata(self, member, name) =>
                {
                    return Ok(value);
                }
                "__doc__" if member == "__str__" => return Ok(self.text("Return str(self).")),
                "__text_signature__" if member == "__str__" => return Ok(self.text("($self, /)")),
                "__name__" => return Ok(self.text(member)),
                "__objclass__" => return Ok(owner),
                "__qualname__" => {
                    return Ok(self.text(format!("{}.{member}", self.class_name(owner))));
                }
                "__get__" => {
                    let callable = self.builtin(if slot_wrapper {
                        "wrapper_descriptor.__get__"
                    } else {
                        "method_descriptor.__get__"
                    });
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: false,
                    }));
                }
                _ => {}
            },
            Value::GetSetDescriptor {
                get,
                owner,
                name: field,
                ..
            } if matches!(
                owner,
                "code"
                    | "int"
                    | "float"
                    | "wrapper_descriptor"
                    | "getset_descriptor"
                    | super::deque::CLASS
                    | super::thread_handle::CLASS
                    | "typing.Union"
                    | "typing.TypeAliasType"
                    | "_ssl.MemoryBIO"
                    | stdlib::hashlib::md5::CLASS
                    | stdlib::datetime::isocalendar::CLASS
            ) || matches!(
                self.heap.get(get),
                Value::HeapTypeMethod { .. } | Value::IoMethod { .. }
            ) || HashState::is_provider_class(owner)
                || Self::dict_view_owner(owner).is_some()
                || owner == value::TEMPLATE_TYPE
                || stdlib::datetime::members(owner).is_some() =>
            {
                match name {
                    "__objclass__" => {
                        if let Value::HeapTypeMethod { owner, .. } | Value::IoMethod { owner, .. } =
                            *self.heap.get(get)
                        {
                            return Ok(owner);
                        }
                        return Ok(self.builtin(owner));
                    }
                    "__name__" => return Ok(self.text(field)),
                    "__qualname__" => {
                        return Ok(
                            self.text(format!("{}.{field}", owner.rsplit('.').next().unwrap()))
                        );
                    }
                    "__get__" | "__set__" | "__delete__"
                        if name == "__get__"
                            || matches!(
                                owner,
                                "code"
                                    | "int"
                                    | "float"
                                    | "wrapper_descriptor"
                                    | "getset_descriptor"
                                    | "typing.Union"
                                    | "typing.TypeAliasType"
                                    | "_ssl.MemoryBIO"
                                    | super::thread_handle::CLASS
                                    | stdlib::hashlib::md5::CLASS
                                    | stdlib::datetime::isocalendar::CLASS
                            )
                            || matches!(
                                self.heap.get(get),
                                Value::HeapTypeMethod { .. } | Value::IoMethod { .. }
                            )
                            || HashState::is_provider_class(owner)
                            || Self::dict_view_owner(owner).is_some() =>
                    {
                        let symbol = format!("getset_descriptor.{name}");
                        let get = self.builtin(&symbol);
                        return Ok(self.alloc(Value::Method {
                            callable: get,
                            receiver: obj,
                            explicit: false,
                        }));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if name == "__new__" && nonconstructible_result_type(class) =>
            {
                return Ok(self.builtin("object.__new__"));
            }
            Value::Native(ref class) if let Some(owner) = HashState::provider_class(class) => {
                let (module, short) = owner.rsplit_once('.').unwrap();
                match name {
                    attr if let Some(field) = HashState::descriptor_name(owner, attr) => {
                        let key = format!("{owner}.{name}");
                        if let Some(&descriptor) = self.builtins.get(&key) {
                            return Ok(descriptor);
                        }
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
                        | value::REGEX_SCANNER_TYPE
                        | value::STRUCT_UNPACK_ITERATOR
                        | "_contextvars.Context"
                        | "_contextvars.ContextVar"
                        | "_contextvars.Token"
                        | KEY_WRAPPER
                        | LRU_CACHE_CLASS
                        | HashState::CLASS
                        | HashState::XOF_CLASS
                        | super::namedtuple::GETTER
                        | super::deque::CLASS
                        | super::deque::ITERATOR
                        | super::deque::REVERSE_ITERATOR
                ) || (class == defaultdict::CLASS && name == "__class_getitem__") =>
            {
                let class = class.as_str().to_owned();
                let (module, short) = class.rsplit_once('.').unwrap();
                if super::deque::iterator_class(&class).is_some()
                    && let Some(value) = self.own_class_attr(obj, name)
                {
                    return Ok(value);
                }
                if class == LRU_CACHE_CLASS
                    && let Some(member) = self.class_attr(obj, name)
                {
                    return Ok(member);
                }
                if class == "array.array" && super::array::has_method(name) {
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
                    "__reduce_ex__" if class == super::deque::CLASS => {
                        return Ok(self.builtin("object.__reduce_ex__"));
                    }
                    "__dict__" if class == super::deque::CLASS => {
                        return super::deque::class_dictionary(self, obj);
                    }
                    "__base__" if class == value::STRUCT_UNPACK_ITERATOR => {
                        return Ok(self.builtin("object"));
                    }
                    "__bases__" | "__mro__" if class == value::STRUCT_UNPACK_ITERATOR => {
                        return Ok(self.builtin_hierarchy_tuple(obj, name));
                    }
                    "__module__" if class == super::deque::CLASS => {
                        return Ok(self.heap.code_name(module));
                    }
                    "__module__" => return Ok(self.text(module.to_owned())),
                    "__class_getitem__" | "MISSING"
                        if let Some(member) = self.own_class_attr(obj, name) =>
                    {
                        return self.bind_attribute(member, NONE, obj);
                    }
                    "__hash__" | "__doc__" | "__text_signature__" if class == KEY_WRAPPER => {
                        return Ok(NONE);
                    }
                    _ if matches!(
                        class.as_str(),
                        KEY_WRAPPER
                            | value::REGEX_PATTERN_TYPE
                            | value::REGEX_MATCH_TYPE
                            | value::STRUCT_UNPACK_ITERATOR
                    ) && BuiltinDescriptor::lookup(&format!("{class}.{name}")).is_some() =>
                    {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ if class == super::namedtuple::GETTER
                        && super::namedtuple::GETTER_METHODS.contains(&name) =>
                    {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ if super::deque::iterator_class(&class).is_some()
                        && let Some(member) = self.class_attr(obj, name) =>
                    {
                        return Ok(member);
                    }
                    _ if class == super::deque::CLASS && super::deque::owns(name) => {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if class == value::ZLIB_DECOMPRESS_TYPE
                    && value::ZLIB_DECOMPRESS_COPY_METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if class == "tuple"
                    && matches!(
                        name,
                        "__new__" | "__getitem__" | "__getnewargs__" | "count" | "index"
                    ) =>
            {
                return Ok(self.builtin(&format!("tuple.{name}")));
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "filter" | "range" | "range_iterator" | "longrange_iterator"
                ) && name == "__reduce_ex__" =>
            {
                return Ok(self.builtin("object.__reduce_ex__"));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "filter" | "map" | "zip")
                    && let Some(member) = self.own_class_attr(obj, name) =>
            {
                return Ok(member);
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "map" | "zip" | "filter" | "memory_iterator") =>
            {
                match name {
                    "__iter__" | "__next__" => return Ok(self.builtin(&format!("{class}.{name}"))),
                    "__name__" | "__qualname__" => return Ok(self.text(class.clone())),
                    "__module__" => return Ok(self.text("builtins")),
                    _ => {}
                }
            }
            Value::Native(ref class)
                if matches!(name, "__module__" | "__name__" | "__qualname__")
                    && let Some(short) = class.strip_prefix("datetime.")
                    && self.builtin_type_name(obj) == Some("type") =>
            {
                return Ok(self.text(if name == "__module__" {
                    "datetime".to_owned()
                } else {
                    short.to_owned()
                }));
            }
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
            Value::Native(ref class) if class == "_io._BytesIOBuffer" => match name {
                "__module__" => return Ok(self.text("_io")),
                "__name__" | "__qualname__" => return Ok(self.text("_BytesIOBuffer")),
                _ => {}
            },
            Value::DirEntry(ref entry) if name == "name" => return Ok(entry.name),
            Value::DirEntry(ref entry) if name == "path" => return Ok(entry.path),
            Value::DequeIterator { .. } => {
                let class = self.exact_type(obj)?;
                if let Some(member) = self.class_attr(class, name) {
                    return self.bind_attribute(member, obj, class);
                }
            }
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
            Value::Count { .. } | Value::Repeat(_) => {
                let class = self.exact_type(obj)?;
                if let Some(value) = self.own_class_attr(class, name) {
                    return self.bind_attribute(value, obj, class);
                }
            }
            Value::Native(ref class) if class == super::thread_local::CLASS => match name {
                "__module__" => return Ok(self.text("_thread")),
                "__name__" | "__qualname__" => return Ok(self.text("_local")),
                attr if attr == "__new__" || super::thread_local::is_method(attr) => {
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
                    "__text_signature__" => {
                        return Ok(if class == "itertools.count" {
                            self.text("(start=0, step=1)")
                        } else {
                            NONE
                        });
                    }
                    _ => {
                        if let Some(value) = self.own_class_attr(obj, name) {
                            return Ok(value);
                        }
                    }
                }
            }
            Value::Native(ref class)
                if class == stdlib::datetime::TZINFO || class == "datetime.timezone" =>
            {
                let base = class == stdlib::datetime::TZINFO;
                match name {
                    "__base__" => {
                        let parent = if base {
                            "object"
                        } else {
                            stdlib::datetime::TZINFO
                        };
                        return Ok(self.builtin(parent));
                    }
                    "__bases__" | "__mro__" => return Ok(self.builtin_hierarchy_tuple(obj, name)),
                    _ => {}
                }
            }
            Value::MappingProxy(_) | Value::Native(_)
                if matches!(name, "__doc__" | "__new__")
                    && (matches!(self.heap.get(obj), Value::MappingProxy(_))
                        || self.builtins.get("mappingproxy") == Some(&obj)) =>
            {
                return Ok(if name == "__doc__" {
                    self.text("Read-only proxy of a mapping.")
                } else {
                    self.builtin("mappingproxy.__new__")
                });
            }
            Value::Native(ref class)
                if class == "mappingproxy"
                    && (builtins::MAPPING_PROXY_SLOTS.contains(&name)
                        || builtins::MAPPING_PROXY_METHODS.contains(&name)) =>
            {
                let member = self.builtin(&format!("{class}.{name}"));
                if name == "__class_getitem__" {
                    return Ok(self.bind_native_classmethod(member, obj));
                }
                return Ok(member);
            }
            Value::MappingProxy(_)
                if builtins::MAPPING_PROXY_SLOTS.contains(&name)
                    || builtins::MAPPING_PROXY_METHODS.contains(&name) =>
            {
                let descriptor = self.builtin(&format!("mappingproxy.{name}"));
                let owner = self.builtin("mappingproxy");
                return self.bind_attribute(descriptor, obj, owner);
            }
            Value::Native(ref symbol) if symbol == "open" && name == "__self__" => {
                if !self.builtins.contains_key("_io.open.__self__") {
                    self.module("_io")?;
                }
                return self
                    .builtins
                    .get("_io.open.__self__")
                    .copied()
                    .ok_or_else(|| Error::new("ImportError", "native _io module is unavailable"));
            }
            Value::Native(ref symbol)
                if matches!(
                    name,
                    "__doc__"
                        | "__text_signature__"
                        | "__self__"
                        | "__name__"
                        | "__qualname__"
                        | "__module__"
                ) && let Some(owner) = builtins::constructor_owner(symbol) =>
            {
                return Ok(match name {
                    "__doc__" => self.text(
                        "Create and return a new object.  See help(type) for accurate signature.",
                    ),
                    "__text_signature__" => self.text("($type, *args, **kwargs)"),
                    "__self__" => match self.builtins.get(owner) {
                        Some(&class) => class,
                        None => {
                            // Creating the owner mutates the heap that holds its name.
                            let owner = owner.to_owned();
                            self.builtin(&owner)
                        }
                    },
                    "__name__" => self.text("__new__"),
                    "__qualname__" => {
                        let class = owner.rsplit('.').next().unwrap_or(owner);
                        self.text(format!("{class}.__new__"))
                    }
                    _ => NONE,
                });
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "int" | "float" | "bytes" | "function" | "cell" | "type"
                ) && name == "__new__" =>
            {
                return Ok(self.builtin(&format!("{class}.__new__")));
            }
            Value::Super { .. } => {
                let class = self.builtin("super");
                if let Some(member) = self.class_attr(class, name) {
                    return self.bind_attribute(member, obj, class);
                }
            }
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
                    let class = self.builtin(stdlib::functools::PARTIAL_CLASS);
                    if let Some(member) = self.class_attr(class, name) {
                        return self.bind_attribute(member, obj, class);
                    }
                }
            },
            Value::Native(ref class)
                if class == stdlib::functools::PARTIAL_CLASS
                    && matches!(name, "__name__" | "__qualname__") =>
            {
                return Ok(self.heap.code_name("partial"));
            }
            #[cfg(target_os = "linux")]
            Value::Mmap { ref region, .. } if name == "closed" => {
                return Ok(if region.is_none() { TRUE } else { FALSE });
            }
            #[cfg(target_os = "linux")]
            Value::Native(ref class) if class == "mmap.mmap" => match name {
                "__name__" | "__qualname__" | "__module__" => return Ok(self.text("mmap")),
                method if stdlib::mmap::METHODS.contains(&method) => {
                    return Ok(self.builtin(&format!("mmap.mmap.{method}")));
                }
                _ => {}
            },
            #[cfg(target_os = "linux")]
            Value::Native(ref class) if class == "select.epoll" => match name {
                "__name__" | "__qualname__" => return Ok(self.text("epoll")),
                "__module__" => return Ok(self.text("select")),
                "fromfd" => return Ok(self.builtin("select.epoll.fromfd")),
                method if stdlib::epoll::METHODS.contains(&method) => {
                    return Ok(self.builtin(&format!("select.epoll.{method}")));
                }
                _ => {}
            },
            #[cfg(target_os = "linux")]
            Value::Epoll(ref epoll) if name == "closed" => {
                return Ok(if epoll.fd() < 0 { TRUE } else { FALSE });
            }
            #[cfg(target_os = "linux")]
            Value::Epoll(_) if name == "fromfd" => return Ok(self.builtin("select.epoll.fromfd")),
            #[cfg(unix)]
            Value::TlsSession(_, owner) | Value::TlsBioSession { owner, .. }
                if name == "context" =>
            {
                return Ok(owner.context());
            }
            #[cfg(unix)]
            Value::TlsBioSession { .. } if name == "do_handshake" => {
                let module = self.module("ssl")?;
                let callable = self.lookup_attr(module, "_bio_handshake")?;
                return Ok(self.alloc(Value::Method {
                    callable,
                    receiver: obj,
                    explicit: false,
                }));
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
            Value::Float(_) if matches!(name, "__getformat__" | "fromhex") => {
                let owner = self.builtin("float");
                let method = self.builtin(&format!("float.{name}"));
                return Ok(self.bind_native_classmethod(method, owner));
            }
            Value::Native(ref class)
                if class == "float" && matches!(name, "__getformat__" | "fromhex") =>
            {
                let method = self.builtin(&format!("float.{name}"));
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
                if class == super::defaultdict::CLASS && name == "__dict__" =>
            {
                return self.native_class_dictionary(
                    obj,
                    "<collections.defaultdict namespace>",
                    super::defaultdict::METHODS
                        .iter()
                        .copied()
                        .chain(["__module__"]),
                );
            }
            Value::Native(ref class) if class == "str" && name == "__dict__" => {
                return self.native_class_dictionary(
                    obj,
                    "<str namespace>",
                    builtins::STR_METHODS.iter().copied().chain([
                        "__new__",
                        "__sizeof__",
                        "maketrans",
                    ]),
                );
            }
            Value::Native(ref class)
                if class == super::defaultdict::CLASS && name == "__module__" =>
            {
                return Ok(self.heap.code_name("collections"));
            }
            Value::Native(ref class)
                if class == "collections.defaultdict"
                    && matches!(name, "__name__" | "__qualname__") =>
            {
                return Ok(self.text("defaultdict"));
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "collections.defaultdict" | "super" | "struct.Struct"
                ) && let Some(member) = self.own_class_attr(obj, name) =>
            {
                return Ok(member);
            }
            Value::Struct { .. } => {
                let class = self.builtin("struct.Struct");
                if let Some(member) = self.own_class_attr(class, name) {
                    return self.bind_attribute(member, obj, class);
                }
            }
            Value::ZlibDecompressor { .. }
                if matches!(name, "eof" | "unused_data" | "unconsumed_tail") =>
            {
                return stdlib::zlib::decompressor(self, name, &[obj], &[]);
            }
            Value::Hash(ref hash) => {
                let owner = hash.lock().class_name();
                match name {
                    "__new__" if HashState::is_provider_class(owner) => {
                        return Ok(self.builtin(&format!("{owner}.__new__")));
                    }
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
                    "_rate_bits" | "_capacity_bits" if owner.starts_with("_sha3.") => {
                        let rate = hash.lock().block_size() * 8;
                        let bits = if name == "_rate_bits" {
                            rate
                        } else {
                            1600 - rate
                        };
                        return Ok(self.int(bits as i64));
                    }
                    "_suffix" if owner.starts_with("_sha3.") => {
                        let suffix = if hash.lock().is_xof() { 0x1f } else { 0x06 };
                        return Ok(self.alloc(Value::Bytes(vec![suffix])));
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
                if (class == weakref::REFERENCE_TYPE
                    && weakref::REFERENCE_METHODS.contains(&name))
                    || (class == weakref::PROXY_TYPE && weakref::PROXY_METHODS.contains(&name))
                    || (class == weakref::CALLABLE_PROXY_TYPE
                        && weakref::CALLABLE_PROXY_METHODS.contains(&name)) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref builtin)
                if name == "__qualname__"
                    && let Some(qualified) = builtin.strip_prefix("weakref.")
                    && (builtin == weakref::REFERENCE_HASH
                        || self.builtin_type_name(obj) == Some("type")) =>
            {
                return Ok(self.text(qualified.to_owned()));
            }
            Value::Native(ref builtin)
                if name == "__module__"
                    && (builtin.starts_with("weakref.") || builtin.starts_with("_weakref.")) =>
            {
                return Ok(self.text(builtin.split('.').next().unwrap().to_owned()));
            }
            Value::ContextVar { .. } | Value::DefaultDict { .. } if name == "__class_getitem__" => {
                let owner = self.exact_type(obj)?;
                return self.lookup_attr(owner, name);
            }
            Value::ContextVar { name: var_name, .. } if name == "name" => return Ok(var_name),
            Value::ContextToken { variable, .. } if name == "var" => return Ok(variable),
            Value::ContextToken { old, .. } if name == "old_value" => {
                return Ok(old.unwrap_or(self.token_missing));
            }
            Value::ContextToken { .. } => {
                let owner = self.exact_type(obj)?;
                if let Some(member) = self.own_class_attr(owner, name) {
                    return self.bind_attribute(member, obj, owner);
                }
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
                    "gzip.BadGzipFile"
                        | "binascii.Error"
                        | "binascii.Incomplete"
                        | "compression.zstd.ZstdError"
                        | "_lzma.LZMAError"
                        | "socket.gaierror"
                        | "socket.herror"
                        | "zlib.error"
                ) =>
            {
                let (module, short) = builtin.rsplit_once('.').unwrap();
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(short.to_owned())),
                    "__module__" => return Ok(self.text(module.to_owned())),
                    "__base__" | "__bases__" => {
                        let base = self.builtin(match short {
                            "BadGzipFile" | "gaierror" | "herror" => "OSError",
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
                if matches!(name, "__base__" | "__bases__" | "__mro__")
                    && (builtins::CORE_RUNTIME_TYPES.contains(&class.as_str())
                        || class == defaultdict::CLASS
                        || builtins::is_exported_type(class)) =>
            {
                if name == "__base__" {
                    return Ok(self.base_lineage(obj, true).get(1).copied().unwrap_or(NONE));
                }
                return Ok(self.builtin_hierarchy_tuple(obj, name));
            }
            Value::Native(ref builtin)
                if (name == "__name__"
                    || (name == "__qualname__"
                        && builtin.starts_with("typing.")
                        && self.is_type_object(obj)))
                    && self.builtins.get(builtin) == Some(&obj) =>
            {
                return Ok(self.text(builtin.rsplit('.').next().unwrap_or(builtin).to_owned()));
            }
            Value::Native(ref method)
                if method.starts_with("abc.abstract.") && name == "__isabstractmethod__" =>
            {
                return Ok(TRUE);
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "object" | "staticmethod" | "classmethod")
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
                if class == "bool"
                    && (builtins::real_methods("int").contains(&name)
                        || matches!(
                            name,
                            "__add__" | "__index__" | "bit_count" | "bit_length" | "to_bytes"
                        )) =>
            {
                return Ok(self.builtin(&format!("int.{name}")));
            }
            Value::Native(ref class)
                if matches!(class.as_str(), "wrapper_descriptor" | "getset_descriptor")
                    && let Some(member) = self.own_class_attr(obj, name) =>
            {
                return Ok(member);
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "method_descriptor" | "wrapper_descriptor" | "classmethod_descriptor"
                ) && name == "__get__" =>
            {
                return Ok(self.builtin(&format!("{class}.__get__")));
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
            Value::Set { frozen, .. } if name == "__class_getitem__" => {
                let owner = if frozen { "frozenset" } else { "set" };
                let member = self.builtin(&format!("{owner}.{name}"));
                let class = self.builtin(owner);
                return Ok(self.bind_native_classmethod(member, class));
            }
            Value::Native(ref owner)
                if builtins::set_methods(owner).contains(&name)
                    || builtins::set_operators(owner).contains(&name) =>
            {
                let member = self.builtin(&format!("{owner}.{name}"));
                if name == "__class_getitem__" {
                    return Ok(self.bind_native_classmethod(member, obj));
                }
                return Ok(member);
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "dict" | "list" | "tuple" | stdlib::functools::PARTIAL_CLASS
                ) || (class == "function" && name == "__call__")
                    || builtins::is_enum_base(class)
                    || builtins::is_exception(class)
                    || (class == "type" && name == "__setattr__")
                    || (class == "memoryview" && builtins::MEMORYVIEW_METHODS.contains(&name))
                    || (class == "bytes" && builtins::BYTES_METHODS.contains(&name))
                    || (class == "complex" && super::complex::METHODS.contains(&name))
                    || (class == "bytearray" && builtins::BYTEARRAY_METHODS.contains(&name))
                    || (name == "__repr__"
                        && matches!(class.as_str(), "bool" | "int" | "float"))
                    || (class == "int"
                        && (builtins::real_methods("int").contains(&name)
                            || matches!(
                                name,
                                "__add__" | "__index__" | "bit_count" | "bit_length" | "to_bytes"
                            )))
                    || (class == "float"
                        && (builtins::real_methods(class).contains(&name)
                            || name == "as_integer_ratio"))
                    || (class == "str"
                        && (name == "maketrans" || builtins::STR_METHODS.contains(&name))) =>
            {
                if let Some(method) = self.class_attr(obj, name) {
                    if name == "__class_getitem__"
                        && matches!(
                            class.as_str(),
                            stdlib::functools::PARTIAL_CLASS
                                | "BaseExceptionGroup"
                                | "ExceptionGroup"
                        )
                    {
                        return Ok(self.bind_native_classmethod(method, obj));
                    }
                    return Ok(method);
                }
            }
            Value::Native(ref class) if class == "object" => {
                match name {
                    "__dict__" => return Ok(self.object_dictionary()),
                    "__doc__" => return Ok(self.text(Self::OBJECT_DOC)),
                    _ => {}
                }
                if let Some(member) = self.raw_object_member(name) {
                    return self.bind_attribute(member, NONE, obj);
                }
            }
            Value::Native(ref class)
                if (class == "set_iterator" && builtins::SET_ITERATOR_METHODS.contains(&name))
                    || (DictIterator::methods(class).contains(&name)
                        && DictIterator::class(class).is_some()) =>
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
            Value::Native(ref class)
                if let Some((owner, signature)) = stdlib::operator::callable_type(class) =>
            {
                match name {
                    "__text_signature__" => return Ok(self.text(signature)),
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(owner.trim_start_matches("operator.")));
                    }
                    "__module__" => return Ok(self.text("operator")),
                    "__call__" => return Ok(self.builtin(&format!("{owner}.__call__"))),
                    "__reduce__" => {
                        return Ok(self.builtin(&format!("{owner}.__reduce__")));
                    }
                    _ => {}
                }
            }
            Value::Native(ref class)
                if (class == "range" && super::range::METHODS.contains(&name))
                    || (matches!(class.as_str(), "range_iterator" | "longrange_iterator")
                        && super::range::ITER_METHODS.contains(&name)) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(ref class)
                if matches!(
                    class.as_str(),
                    "list_iterator"
                        | "tuple_iterator"
                        | "str_ascii_iterator"
                        | "str_iterator"
                        | "enumerate"
                ) && matches!(name, "__iter__" | "__next__") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
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
                if let Some(class) = constructor
                    && matches!(self.heap.get(class), Value::Class { .. })
                    && self.class_attr(class, name).is_some()
                {
                    return self.module_instance_attribute(obj, class, scope, name);
                }
                if name == "__dict__" {
                    return self.module_globals(scope);
                }
                if stdlib::has_lazy_attributes(module) {
                    if let Some(value) = self.scope_name(scope, name) {
                        return Ok(value);
                    }
                    let bind_to_module = module == "functools" && name == "cmp_to_key";
                    match stdlib::attribute(self, &module.to_owned(), name) {
                        Ok(value) => {
                            let value = if bind_to_module {
                                self.alloc(Value::Method {
                                    callable: value,
                                    receiver: obj,
                                    explicit: false,
                                })
                            } else {
                                value
                            };
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
                if name == "__base__"
                    && metaclass_member.is_none()
                    && let Some(bases) = self.class_bases_tuple(obj)
                {
                    if bases.is_empty() {
                        return Ok(NONE);
                    }
                    return self.member_layout_base(&bases);
                }
                // type.__abstractmethods__ exposes only this class's stored value.
                if name == "__abstractmethods__" && metaclass_member.is_none() {
                    return self
                        .own_class_attr(obj, name)
                        .ok_or_else(|| self.missing_attribute(obj, name));
                }
                if matches!(name, "__annotations__" | "__annotate__")
                    && metaclass_member.is_none()
                    && self.has_class_annotation_provider(scope)
                {
                    return self.class_annotation_attr(obj, scope, name);
                }
                if name == "__type_params__" {
                    return Ok(self
                        .own_class_attr(obj, name)
                        .unwrap_or_else(|| self.heap.packed_tuple(Vec::new(), false)));
                }
                // A type's stored qualified name is metadata even if its str subclass is a descriptor.
                if name == "__qualname__"
                    && let Some(value) = self.own_class_attr(obj, name)
                    && self.string_storage(value).is_some()
                {
                    return Ok(value);
                }
                if name == "__name__" && metaclass_member.is_none() {
                    if let Some(name) = self.assigned_class_name(scope) {
                        return Ok(name);
                    }
                    let class_name = class_name.clone();
                    return Ok(self.heap.code_name(&class_name));
                }
                if let Some(value) = self.class_attr(obj, name) {
                    return self.bind_attribute(value, NONE, obj);
                }
                if self.allocation_base(obj) == Some(self.builtin("object"))
                    && let Some(member) = self.raw_object_member(name)
                {
                    return self.bind_attribute(member, NONE, obj);
                }
                if name == "__init__" && self.has_base(obj, "type") {
                    return Ok(self.builtin("type.__init__"));
                }
                if matches!(name, "_abc_caches_clear" | "_abc_registry_clear")
                    && metaclass_member.is_none()
                    && self.is_bundled_abc(obj)
                {
                    let callable = self.builtin(&format!("abc.{name}"));
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
                if name == "__new__" && self.uses_object_new(self.allocation_base(obj)) {
                    return Ok(self.builtin("object.__new__"));
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
                if let Some(layout) = class.and_then(stdlib::structseq::layout)
                    && let Some(n) = stdlib::structseq::count_attribute(layout, name)
                {
                    return Ok(self.int(n));
                }
                if class == Some(stdlib::fixed_structseq::SCHED_PARAM)
                    && matches!(name, "__reduce__" | "__reduce_ex__" | "__replace__")
                {
                    let owner = self.builtin(stdlib::fixed_structseq::SCHED_PARAM);
                    if let Some(member) = self.class_attr(owner, name) {
                        return self.bind_attribute(member, obj, owner);
                    }
                }
                if class == Some(stdlib::flags::TYPE)
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
        if let Value::Raised(error) = self.heap.get(obj) {
            let dictionary = error.trace.as_ref().and_then(|trace| trace.dictionary);
            if let Some(field) = self.dictionary_field(dictionary, name)? {
                return Ok(field);
            }
            if matches!(
                name,
                "add_note" | "__reduce__" | "__reduce_ex__" | "__setstate__"
            ) {
                let class = self.exact_type(obj)?;
                return self.instance_attribute(obj, class, None, name);
            }
        }
        self.builtin_attribute(obj, name, lookup, native_key)
    }

    fn builtin_attribute(
        &mut self,
        obj: Id,
        name: &str,
        lookup: AttributeLookup,
        native_key: Option<(Id, Id, Id)>,
    ) -> Result<Id> {
        if name == "__doc__" {
            let owner = match self.heap.get(obj) {
                Value::Native(symbol) => Some(symbol.as_str()),
                _ => self.builtin_type_name(obj),
            };
            if let Some(doc) = owner.and_then(builtins::type_doc) {
                return Ok(self.text(doc));
            }
        }
        if name == "__prepare__" && self.is_type_object(obj) {
            let type_class = self.builtin("type");
            let owner = if self.direct_subclass(obj, type_class) {
                obj
            } else {
                self.exact_type(obj)?
            };
            let prepare = self.builtin("type.__prepare__");
            return Ok(self.bind_native_classmethod(prepare, owner));
        }
        if name == "__call__" && self.is_type_object(obj) {
            let owner = self.builtin("type");
            let slot = self.builtin("type.__call__");
            let receiver = if self.direct_subclass(obj, owner) {
                NONE
            } else {
                obj
            };
            return self.bind_attribute(slot, receiver, owner);
        }
        if matches!(name, "mro" | "__subclasses__") && self.is_type_object(obj) {
            let owner = self.builtin("type");
            let descriptor = self.builtin(if name == "mro" {
                "type.mro"
            } else {
                "type.__subclasses__"
            });
            let receiver = if obj == owner { NONE } else { obj };
            return self.bind_attribute(descriptor, receiver, owner);
        }
        if name == "__init_subclass__" {
            let class = if self.builtin_type_name(obj) == Some("type") {
                obj
            } else {
                self.exact_type(obj)?
            };
            let hook = self
                .own_class_attr(class, name)
                .unwrap_or_else(|| self.builtin("object.__init_subclass__"));
            return self.bind_attribute(hook, NONE, class);
        }
        if name == "__init__" {
            let (class, receiver) = if self.is_type_object(obj) {
                (obj, NONE)
            } else {
                (self.exact_type(obj)?, obj)
            };
            if class == obj && self.builtins.get("type") == Some(&class) {
                return Ok(self.builtin("type.__init__"));
            }
            if self.inherits_native_object_init(class) {
                let init = self.builtin("object.__init__");
                return self.bind_attribute(init, receiver, class);
            }
        }
        if name == "__sizeof__" {
            let class = if self.builtin_type_name(obj) == Some("type") {
                obj
            } else {
                self.exact_type(obj)?
            };
            if let Some(callable) = self.class_attr(class, name) {
                if class == obj {
                    return Ok(callable);
                }
                return Ok(self.alloc(Value::Method {
                    callable,
                    receiver: obj,
                    explicit: false,
                }));
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
                Value::List(_) => return Ok(self.builtin("list.__new__")),
                Value::Int(_) => return Ok(self.builtin("int.__new__")),
                Value::Bytes(_) => return Ok(self.builtin("bytes.__new__")),
                Value::Function { .. } => return Ok(self.builtin("function.__new__")),
                Value::Cell(_) => return Ok(self.builtin("cell.__new__")),
                Value::ThreadLocal(_) => return Ok(self.builtin("_thread._local.__new__")),
                Value::Str(_) | Value::SurrogateStr(_) => return Ok(self.builtin("str.__new__")),
                Value::Instance { class, .. }
                    if self.uses_object_new(self.allocation_base(*class)) =>
                {
                    return Ok(self.builtin("object.__new__"));
                }
                _ => {}
            }
        }
        if let Some(method) = builtins::method_name(self.heap.get(base), name) {
            if BuiltinDescriptor::lookup(&method).is_some() {
                let class = self.exact_type(obj)?;
                if let Some(member) = self.class_attr(class, name) {
                    return self.bind_attribute(member, obj, class);
                }
            }
            let fun = self.alloc(Value::Native(method));
            return Ok(self.alloc(Value::Method {
                callable: fun,
                receiver: obj,
                explicit: false,
            }));
        }
        // Native directories can list unmaterialized slots. A bound descriptor already
        // returned its value or error above; only an intrinsic miss reaches this point.
        if matches!(self.heap.get(obj), Value::Native(_))
            && let Some(member) = self.own_class_attr(obj, name)
            && (lookup == AttributeLookup::ListedClassMember
                || matches!(self.heap.get(member), Value::ReadOnlyMember(_, _)))
        {
            return self.bind_attribute(member, NONE, obj);
        }
        if lookup == AttributeLookup::ListedClassMember
            && builtins::dir::listed_by_native_base(self, obj, name)?
        {
            return Ok(NONE);
        }
        if let Some((class, receiver, key)) = native_key {
            return self.native_attribute_with_key(obj, class, receiver, key, name);
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

    fn check_match_attribute_write(&self, obj: Id, name: &str) -> Result<()> {
        if matches!(self.heap.get(obj), Value::RegexMatch(_)) {
            match name {
                "pos" | "endpos" => return Err(Error::new("AttributeError", "readonly attribute")),
                "regs" => {
                    return Err(Error::new(
                        "AttributeError",
                        "attribute 'regs' of 're.Match' objects is not writable",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn check_zone_constant_write(&self, obj: Id, name: &str) -> Result<()> {
        if matches!(name, "utc" | "min" | "max")
            && matches!(self.heap.get(obj), Value::Calendar { value, .. } if value.class() == "timezone")
        {
            return Err(Error::new(
                "AttributeError",
                format!("'datetime.timezone' object attribute '{name}' is read-only"),
            ));
        }
        Ok(())
    }

    pub(super) fn immutable_type_name(&self, obj: Id) -> Option<String> {
        if let Some(class) = self.heap.get(obj).heap_type()
            && class != stdlib::random::instance::CLASS
        {
            return Some(class.to_owned());
        }
        if let Value::Native(class) = self.heap.get(obj)
            && (super::deque::iterator_class(class).is_some()
                || matches!(
                    class.as_str(),
                    value::STRUCT_UNPACK_ITERATOR
                        | value::REGEX_SCANNER_TYPE
                        | super::thread_handle::CLASS
                )
                || builtins::is_static_gc_type(class)
                || stdlib::itertools::cursor_type(class).is_some()
                || HashState::is_provider_class(class)
                || matches!(
                    class.as_str(),
                    HashState::CLASS
                        | HashState::XOF_CLASS
                        | "datetime.timezone"
                        | "typing.TypeVar"
                        | "NoDefaultType"
                        | KEY_WRAPPER
                        | stdlib::flags::TYPE
                        | value::REGEX_PATTERN_TYPE
                        | value::REGEX_MATCH_TYPE
                ))
        {
            return Some(class.to_owned());
        }
        if let Value::Class { name, scope, .. } = self.heap.get(obj) {
            let frame = self.scope_data(*scope);
            if frame
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.immutable_type)
            {
                return Some(format!("{}.{name}", frame.module));
            }
        }
        None
    }

    fn check_immutable_type_write(&self, obj: Id, attr: &str) -> Result<()> {
        if let Some(class) = self.immutable_type_name(obj) {
            return Err(Error::new(
                "TypeError",
                format!("cannot set '{attr}' attribute of immutable type '{class}'"),
            ));
        }
        Ok(())
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

        self.delete_named_attr(obj, AttributeName::Text(name))
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn delete_named_attr(&mut self, obj: Id, attr: AttributeName<'_>) -> Result<()> {
        let spelling = attr.text(self)?;
        let name = spelling.as_ref();
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::DirEntry(_)) && matches!(name, "name" | "path") {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_local_attribute(obj, attr, None);
        }
        if matches!(self.heap.get(obj), Value::Range(_) | Value::Slice(_))
            && matches!(name, "start" | "stop" | "step")
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(hook) = self.bound_slot(obj, "__delattr__")? {
            let name = attr.key(self);
            self.call(hook, vec![name], Vec::new())?;
            return Ok(());
        }
        self.default_object_attribute(obj, attr, ObjectAttribute::Edit(None))
            .map(|_| ())
    }

    pub(super) fn delete_dict_attribute(
        &mut self,
        obj: Id,
        dictionary: Id,
        name: &str,
    ) -> Result<()> {
        let key = self.text(name);
        match self.delete_item(dictionary, key) {
            Ok(()) => {
                self.finish_binding_release();
                Ok(())
            }
            Err(error) if error.kind == "KeyError" => Err(self.missing_attribute(obj, name)),
            result => result,
        }
    }

    /// Bypass the object's deletion hook, while honoring data descriptors on its class.
    pub(super) fn default_delete_attr(&mut self, obj: Id, name: &str) -> Result<()> {
        if name == "tb_next" && matches!(self.heap.get(obj), Value::Traceback { .. }) {
            return Err(Error::new("TypeError", "can't delete tb_next attribute"));
        }
        self.check_immutable_type_write(obj, name)?;
        if let Some(result) = self.update_alias_name(obj, name, None) {
            return result;
        }
        if matches!(name, "__name__" | "__qualname__")
            && let Some(frame) = self.generator_name_frame(obj)
        {
            return self.set_generator_name(frame, name, NONE);
        }
        if let Some(result) = super::paramspec::update(self, obj, name, None) {
            return result;
        }
        self.check_match_attribute_write(obj, name)?;
        self.check_zone_constant_write(obj, name)?;
        self.check_template_write(obj, name)?;
        super::namespace::check_class_write(self, obj, name)?;
        stdlib::io::check_class_write(self, obj, name)?;
        if let Some(class) = self.heap.get(obj).heap_type()
            && class != stdlib::random::instance::CLASS
        {
            return Err(Error::new(
                "TypeError",
                format!("cannot set '{name}' attribute of immutable type '{class}'"),
            ));
        }
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
        let member = self.writable_attribute_member(obj, name)?;
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
            return self.edit_data_descriptor(descriptor, obj, None);
        }
        if exception_member.is_some() {
            return self.update_exception_field(obj, name, None);
        }
        if self.update_key_wrapper_field(obj, name, None)? {
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
        if member.is_none() && self.exposes_object_class(obj, name) {
            return Err(Error::new("TypeError", "can't delete __class__ attribute"));
        }
        if member.is_none()
            && matches!(name, "__name__" | "fget" | "fset" | "fdel")
            && self.heap.get(obj).property().is_some()
        {
            if name == "__name__" {
                self.replace_property_name(obj, None);
                return Ok(());
            }
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::Raised(_)) && name != "__class__" {
            return self.update_exception_field(obj, name, None);
        }
        match *self.heap.get(obj) {
            ref file if file.native_io_class_name().is_some() => {
                let dictionary = self.native_file_dictionary(obj);
                self.update_dictionary_field(obj, dictionary, name, None)
            }
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
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
            Value::Property(_) if name == "__doc__" => {
                self.heap.get_mut(obj).property_mut().unwrap().doc = NONE;
                Ok(())
            }
            Value::StaticMethod { dictionary, .. } | Value::ClassMethod { dictionary, .. } => {
                super::method_decorator::update_attribute(self, obj, dictionary, name, None)
            }
            Value::Module { scope, .. } => {
                if name == "builtin_module_names" && self.pending_builtin_names_scope == Some(scope)
                {
                    self.pending_builtin_names_scope = None;
                    if self.scope_name(scope, name).is_none() {
                        return Ok(());
                    }
                }
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
            Value::Native(_) if let Some(&scope) = self.enum_scopes.get(&obj) => {
                self.update_enum_base(scope, name, None)
            }
            Value::Class { scope, .. } => {
                if matches!(name, "__annotations__" | "__annotate__")
                    && member.is_none()
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
                    "__annotate__" if member.is_none() => {
                        Error::new("TypeError", "cannot delete __annotate__ attribute")
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
            Value::Instance {
                class, namespace, ..
            } if self.builtins.get("object") != Some(&class) => {
                let layout = self.checked_instance_layout(obj, class, name)?;
                if name == "__dict__" && member.is_none() && !self.layout_descriptors_cleared(class)
                {
                    if !layout.dictionary {
                        return Err(self.missing_attribute(obj, name));
                    }
                    self.replace_instance_dictionary(obj, class, None);
                    return Ok(());
                }
                if !layout.active_slot
                    && let Some(storage) = namespace.or_else(|| {
                        layout
                            .materialize_dictionary_on_write
                            .then(|| self.instance_dictionary(obj, class))
                    })
                {
                    return self.update_dictionary_field(obj, storage, name, None);
                }
                if layout.declared_slot && !layout.active_slot {
                    return Err(self.missing_attribute(obj, name));
                }
                if !self.heap.remove_instance_field(obj, name) {
                    return Err(self.missing_attribute(obj, name));
                }
                self.finish_binding_release();
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

    /// A native class can publish ordinary Python instance storage in its own namespace.
    pub(super) fn native_owns_layout_descriptor(&self, class: Id, name: &str) -> bool {
        self.own_class_attr(class, name).is_some_and(|member| {
            matches!(self.heap.get(member),
                Value::LayoutDescriptor(owner, field) if *owner == class && *field == name)
        })
    }

    fn native_has_instance_dictionary(&self, class: Id) -> bool {
        if self.enum_scopes.contains_key(&class) {
            return self
                .find_in_mro(class, |base| {
                    self.native_owns_layout_descriptor(base, "__dict__")
                        .then_some(())
                })
                .is_some();
        }
        self.native_owns_layout_descriptor(class, "__dict__")
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
        let ancestors = match self.heap.get(class) {
            Value::Class { ancestors, .. } => ancestors.as_slice(),
            _ => &[],
        };
        let mut shadowed = false;
        for &owner_id in std::iter::once(&class).chain(ancestors) {
            let base = self.heap.get(owner_id);
            let Value::Class {
                scope, name: owner, ..
            } = base
            else {
                layout.dictionary |= self.native_has_instance_dictionary(owner_id);
                // These native instances carry their namespace in their own representation.
                if let Value::Native(kind) = base {
                    layout.dictionary |=
                        matches!(kind.as_str(), "module" | "classmethod" | "staticmethod")
                            || builtins::is_exception(kind);
                }
                if matches!(base, Value::Native(kind) if super::slot_member::VARIABLE_SIZE_BASES.contains(&kind.as_str()))
                {
                    layout.materialize_dictionary_on_write = true;
                }
                if matches!(base, Value::Native(kind) if kind == stdlib::functools::PARTIAL_CLASS) {
                    // Nested partials can flatten only while the inner object's dict is absent.
                    layout.dictionary = true;
                    layout.materialize_dictionary_on_write = true;
                }
                continue;
            };
            // In-memory I/O pickle state distinguishes a missing dictionary from an emptied one.
            layout.materialize_dictionary_on_write |=
                matches!(base.io_kind(), Some("BytesIO" | "StringIO"));
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

    /// Slots and data descriptor methods operate on the object even with a thread-specific dictionary.
    pub(super) fn local_data_attribute(&self, class: Id, name: &str, writing: bool) -> bool {
        if self.instance_layout(class, name).active_slot {
            return true;
        }
        self.class_attr(class, name).is_some_and(|member| {
            self.native_data_descriptor(member)
                || self.data_descriptor(member)
                    && (writing || self.slot(member, "__get__").is_some())
        })
    }

    fn writable_attribute_member(&mut self, obj: Id, name: &str) -> Result<Option<Id>> {
        if name == "__doc__"
            && matches!(
                self.builtin_type_name(obj),
                Some("wrapper_descriptor" | "getset_descriptor")
            )
        {
            let class = self.exact_type(obj)?;
            return Ok(self.own_class_attr(class, name));
        }
        if matches!(
            self.heap.get(obj),
            Value::Method { .. }
                | Value::Code(_)
                | Value::PythonFrame { .. }
                | Value::LzmaDecompressor(_)
                | Value::Bz2Decompressor(_)
                | Value::ThreadHandle(_)
                | Value::ZstdDecompressor(_)
                | Value::ZstdCompressor(_)
        ) {
            let class = self.exact_type(obj)?;
            return Ok(self.own_class_attr(class, name));
        }
        if self.heap.get(obj).native_io_class_name().is_some() {
            let class = self.exact_type(obj)?;
            return Ok(self.class_attr(class, name));
        }
        if name == "cr_origin" && matches!(self.heap.get(obj), Value::Coroutine { .. }) {
            return Ok(Some(self.readonly_member("coroutine", "cr_origin")));
        }
        if name == "mapping" && matches!(self.heap.get(obj), Value::DictView { .. }) {
            return Ok(self.dict_view_mapping_member(obj));
        }
        if matches!(name, "numerator" | "denominator" | "real" | "imag")
            && matches!(
                self.heap.get(obj),
                Value::Int(_) | Value::Bool(_) | Value::Float(_)
            )
        {
            return Ok(self.builtin_numeric_member(obj, name));
        }
        if let Value::NamedTuple {
            class: Some(owner), ..
        } = *self.heap.get(obj)
        {
            let member = if let Some(layout) = stdlib::structseq::layout(owner) {
                stdlib::structseq::member(self, layout, name)
            } else if let Some(class) = stdlib::fixed_structseq::fixed_type(owner) {
                stdlib::fixed_structseq::member(self, class, name)
            } else {
                None
            };
            if member.is_some() {
                return Ok(member);
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
            return Ok(Some(member));
        }
        if matches!(self.heap.get(obj), Value::TypeVar(_)) {
            let owner = self.builtin("typing.TypeVar");
            return Ok(self.own_class_attr(owner, name));
        }
        if name == "__text_signature__"
            && matches!(
                self.heap.get(obj),
                Value::Class {
                    metaclass: None,
                    ..
                }
            )
        {
            let meta = self.builtin("type");
            return Ok(self.own_class_attr(meta, name));
        }
        let Some(member) = self.slot(obj, name) else {
            return Ok(None);
        };
        if let Value::Instance { class, .. } = *self.heap.get(obj)
            && self.instance_layout(class, name).active_slot
        {
            return Ok(None);
        }
        Ok(Some(member))
    }

    /// An inline Python slot can shadow object's class descriptor.
    fn exposes_object_class(&self, obj: Id, name: &str) -> bool {
        name == "__class__"
            && !matches!(*self.heap.get(obj), Value::Instance { class, .. } if self.is_inline_instance_slot(class, name))
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
        let original = std::mem::take(fields.as_mut());
        let mut ordinary = Vec::new();
        let mut retained = indexmap::IndexMap::new();
        for (name, value) in original {
            if self.instance_layout(class, &name).declared_slot {
                retained.insert(name, value);
            } else {
                ordinary.push((name, value));
            }
        }
        let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        **fields = retained;
        ordinary
    }

    pub(super) fn raw_instance_field(&self, obj: Id, name: &str) -> Option<Id> {
        self.heap.raw_instance_field(obj, name)
    }

    /// Read a non-special field on an ordinary object only if lookup needs no Python call.
    pub(super) fn plain_instance_field(&self, obj: Id, name: &str) -> Option<Id> {
        let Value::Instance {
            class,
            fields,
            namespace,
            ..
        } = self.heap.get(obj)
        else {
            return None;
        };
        if self.allocation_base(*class)? != *self.builtins.get("object")?
            || self.class_attr(*class, "__getattribute__").is_some()
            || self.class_attr(*class, "__getattr__").is_some()
            || self.class_attr(*class, name).is_some()
        {
            return None;
        }
        let layout = self.instance_layout(*class, name);
        if layout.active_slot {
            return fields.get(name).copied();
        }
        if !layout.declared_slot
            && let Some(&value) = fields.get(name)
        {
            return Some(value);
        }
        self.scalar_dict_name(self.dict_storage((*namespace)?)?, name)?
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
            Value::ClassMethod { dictionary, .. }
            | Value::StaticMethod { dictionary, .. }
            | Value::Cached { dictionary, .. } => Some(dictionary),
            Value::Class { scope, .. }
                if self.classes.get(super::namespace::CLASS) != Some(&obj) =>
            {
                let proxy = self.class_dictionary(scope);
                Some(self.mapping_proxy_target(proxy))
            }
            ref value => value.native_file_dict(),
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

    /// Introduce a getset where a source class first adds an ordinary instance dictionary.
    pub(super) fn install_instance_dictionary_descriptor(
        &mut self,
        class: Id,
        scope: Id,
        layout_base: Id,
    ) {
        // A metaclass instance exposes its class namespace through type's mapping proxy.
        if self.has_base(layout_base, "type")
            || self.scope_name(scope, "__dict__").is_some()
            || !self.class_instances_have_dict(class)
            || self.class_instances_have_dict(layout_base)
        {
            return;
        }
        let descriptor = self.alloc(Value::LayoutDescriptor(class, "__dict__"));
        self.bind(scope, "__dict__", descriptor);
    }

    fn layout_owner_name(&self, owner: Id) -> &str {
        match self.heap.get(owner) {
            Value::Native(_) => self.class_qualname(owner),
            _ => self.class_name(owner),
        }
    }

    fn layout_receiver_class(&mut self, owner: Id, field: &str, receiver: Id) -> Result<Id> {
        let class = self.exact_type(receiver)?;
        if self.direct_subclass(class, owner) {
            return Ok(class);
        }
        Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{field}' for '{}' objects doesn't apply to a '{}' object",
                self.layout_owner_name(owner),
                self.type_name(receiver)
            ),
        ))
    }

    pub(super) fn getset_receiver<'a>(
        &self,
        method: &str,
        args: &'a [Id],
    ) -> Result<(Id, &'a [Id])> {
        let Some((&descriptor, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '{method}' of 'getset_descriptor' object needs an argument"),
            ));
        };
        if !self.builtin_slot_accepts(descriptor, "getset_descriptor") {
            let actual = self
                .builtin_type_name(descriptor)
                .unwrap_or_else(|| self.type_name(descriptor));
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' requires a 'getset_descriptor' object but received a '{actual}'"
                ),
            ));
        }
        Ok((descriptor, rest))
    }

    /// A mixed-in Python dictionary getset defers to the native storage base's own getset.
    fn native_dictionary_getset(&self, class: Id) -> Option<(Id, Option<Id>)> {
        let base = self.allocation_base(class)?;
        let descriptor = self.own_class_attr(base, "__dict__")?;
        match *self.heap.get(descriptor) {
            Value::GetSetDescriptor { get, set, .. } => Some((get, set)),
            _ => None,
        }
    }

    /// Read the Argument Clinic header from the original doc using the current type name.
    fn class_text_signature(&self, class: Id) -> Option<&str> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            return None;
        };
        let doc = self
            .scope_data(scope)
            .class_namespace
            .as_ref()?
            .original_doc?;
        let doc = self.str_value(doc).ok()?.split('\0').next()?;
        let name = self.class_name(class).rsplit('.').next()?;
        let signature = doc.strip_prefix(name)?;
        let (header, _) = signature.split_once(value::TEXT_SIGNATURE_END)?;
        (header.starts_with('(') && !header.contains("\n\n")).then(|| &signature[..=header.len()])
    }

    pub(super) fn read_layout_descriptor(
        &mut self,
        owner: Id,
        name: &str,
        receiver: Id,
    ) -> Result<Id> {
        match name {
            "f_builtins" => {
                self.layout_receiver_class(owner, name, receiver)?;
                let Value::PythonFrame { builtins, .. } = *self.heap.get(receiver) else {
                    unreachable!("validated frame receiver")
                };
                Ok(builtins)
            }
            "__code__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                let Value::Function { code, .. } = *self.heap.get(receiver) else {
                    unreachable!("validated function receiver")
                };
                Ok(code)
            }
            "characters_written" => {
                self.layout_receiver_class(owner, name, receiver)?;
                Ok(self.os_attribute(receiver, name)?.expect("OSError member"))
            }
            field if builtins::EXCEPTION_GETSETS.contains(&field) => {
                self.layout_receiver_class(owner, field, receiver)?;
                self.read_exception_getset(receiver, field)
            }
            "__annotations__" | "__annotate__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                let Value::Module { scope, .. } = *self.heap.get(receiver) else {
                    return Err(self.missing_attribute(receiver, name));
                };
                self.module_annotation(scope, name)
            }
            "__class__" => self.exact_type(receiver),
            "__text_signature__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                // Native types publish their existing constructor metadata on the type itself.
                if matches!(self.heap.get(receiver), Value::Native(_)) {
                    return Ok(self.optional_attr(receiver, name)?.unwrap_or(NONE));
                }
                let text = self.class_text_signature(receiver).map(str::to_owned);
                Ok(text.map_or(NONE, |text| self.text(text)))
            }
            "__weakref__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                Ok(self
                    .heap
                    .python_weak_references(receiver)
                    .first()
                    .copied()
                    .unwrap_or(NONE))
            }
            "__dict__" => {
                let class = self.layout_receiver_class(owner, name, receiver)?;
                if let Some((get, _)) = self.native_dictionary_getset(class) {
                    return self.call(get, vec![receiver], Vec::new());
                }
                match *self.heap.get(receiver) {
                    Value::Instance { .. } => Ok(self.instance_dictionary(receiver, class)),
                    Value::Partial { dictionary, .. } => {
                        Ok(self.partial_dictionary(receiver, dictionary))
                    }
                    Value::Module { scope, .. } => self.module_globals(scope),
                    Value::Class { scope, .. } => Ok(self.class_dictionary(scope)),
                    Value::Raised(_) => Ok(self.exception_dictionary(receiver)),
                    _ => Err(self.missing_attribute(receiver, name)),
                }
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn write_layout_descriptor(
        &mut self,
        owner: Id,
        name: &str,
        receiver: Id,
        value: Option<Id>,
    ) -> Result<()> {
        match name {
            "__code__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                self.set_function_attr(receiver, name, value.unwrap_or(NONE))
            }
            "characters_written" => {
                self.layout_receiver_class(owner, name, receiver)?;
                self.update_os_field(receiver, name, value).map(|_| ())
            }
            field if builtins::EXCEPTION_GETSETS.contains(&field) => {
                self.layout_receiver_class(owner, field, receiver)?;
                self.write_exception_getset(receiver, field, value)
            }
            "f_builtins" | "__text_signature__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                Err(self.readonly_getset_error(self.layout_owner_name(owner), name, receiver))
            }
            "__dict__" => {
                let class = self.layout_receiver_class(owner, name, receiver)?;
                if let Some((_, set)) = self.native_dictionary_getset(class) {
                    let set =
                        set.ok_or_else(|| Error::new("AttributeError", "readonly attribute"))?;
                    let mut args = vec![receiver];
                    args.extend(value);
                    return self.call(set, args, Vec::new()).map(|_| ());
                }
                match self.heap.get(receiver) {
                    Value::Class { .. } => {
                        return Err(Error::new(
                            "AttributeError",
                            "attribute '__dict__' of 'type' objects is not writable",
                        ));
                    }
                    Value::Module { .. } => {
                        return Err(Error::new("AttributeError", "readonly attribute"));
                    }
                    Value::Instance { .. } | Value::Raised(_) | Value::Partial { .. } => {}
                    _ => return Err(self.missing_attribute(receiver, name)),
                }
                if let Some(dictionary) = value
                    && self.dict_storage(dictionary).is_none()
                {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            self.type_name(dictionary)
                        ),
                    ));
                }
                if matches!(
                    self.heap.get(self.scalar_value(receiver)),
                    Value::Partial { .. }
                ) {
                    let dictionary =
                        value.ok_or_else(|| Error::new("TypeError", "cannot delete __dict__"))?;
                    if matches!(self.heap.get(receiver), Value::Partial { .. }) {
                        self.replace_partial_dictionary(receiver, dictionary);
                        return Ok(());
                    }
                }
                if let Value::Raised(error) = self.heap.get_mut(receiver) {
                    let dictionary =
                        value.ok_or_else(|| Error::new("TypeError", "cannot delete __dict__"))?;
                    let previous = error.tracing().dictionary.replace(dictionary);
                    if let Some(old) = previous
                        && old != dictionary
                    {
                        self.heap.weak_edge_removed(old);
                    }
                } else {
                    self.replace_instance_dictionary(receiver, class, value);
                }
                Ok(())
            }
            "__weakref__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                Err(Error::new(
                    "AttributeError",
                    format!(
                        "attribute '__weakref__' of '{}' objects is not writable",
                        self.layout_owner_name(owner)
                    ),
                ))
            }
            "__annotations__" | "__annotate__" => {
                self.layout_receiver_class(owner, name, receiver)?;
                let Value::Module { scope, .. } = *self.heap.get(receiver) else {
                    return Err(self.missing_attribute(receiver, name));
                };
                self.update_module_annotations(scope, name, value)?;
                if let Some(value) = value {
                    self.bind(scope, name, value);
                } else {
                    let key = self.text(name);
                    self.remove_binding(scope, name, key)?;
                }
                Ok(())
            }
            "__class__" => self.assign_instance_class(receiver, value),
            _ => unreachable!(),
        }
    }

    /// Lazily expose the dictionary that subsequent ordinary attribute operations use.
    pub(super) fn instance_dictionary(&mut self, obj: Id, class: Id) -> Id {
        self.register_stream_with_attributes(obj, class);
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

    pub(super) fn replace_instance_dictionary(&mut self, obj: Id, class: Id, new: Option<Id>) {
        self.register_stream_with_attributes(obj, class);
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

    /// Find the direct base where a slot-free Python hierarchy first acquired this layout.
    fn dict_layout_base(&self, mut class: Id, allocator: Id, dict: bool, weak: bool) -> Option<Id> {
        loop {
            let Value::Class { scope, .. } = self.heap.get(class) else {
                return None;
            };
            let namespace = self.scope_data(*scope).class_namespace.as_ref()?;
            let Value::Tuple(bases) = self.heap.get(namespace.bases) else {
                return None;
            };
            let base = bases.first().copied().unwrap_or(allocator);
            if base == allocator
                || self.class_instances_have_dict(base) != dict
                || self.class_instances_support_weakref(base) != weak
            {
                return Some(base);
            }
            class = base;
        }
    }

    fn assign_instance_class(&mut self, receiver: Id, value: Option<Id>) -> Result<()> {
        let Some(target) = value.filter(|&id| self.is_type_object(id)) else {
            return Err(self.immutable_class_assignment(value));
        };
        if !self.audit_hooks.is_empty() {
            let name = self.text("__class__");
            super::stdlib::audit::event(self, "object.__setattr__", &[receiver, name, target])?;
        }
        let source = match *self.heap.get(receiver) {
            Value::Instance { class, .. } => class,
            Value::Raised(ref error) => error
                .class()
                .ok_or_else(|| self.immutable_class_assignment(value))?,
            Value::Module { constructor, .. } => {
                constructor.unwrap_or_else(|| self.builtin("module"))
            }
            _ => return Err(self.immutable_class_assignment(value)),
        };
        let module = self.builtin("module");
        let module_pair = self.subclass(source, module) && self.subclass(target, module);
        let mutable_class = |class| {
            matches!(self.heap.get(class), Value::Class { scope, .. }
                if self.scope_data(*scope).class_cell.is_some()
                    && !self.scope_data(*scope).class_namespace.as_ref().is_some_and(|ns| ns.immutable_type))
        };
        if !module_pair && (!mutable_class(source) || !mutable_class(target)) {
            return Err(self.immutable_class_assignment(value));
        }
        if source == target {
            return Ok(());
        }
        if matches!(self.heap.get(receiver), Value::Module { .. }) {
            if !module_pair
                || self.module_slot_layout(source)? != self.module_slot_layout(target)?
            {
                return Err(self.class_layout_mismatch(source, target));
            }
            let Value::Module {
                constructor, name, ..
            } = self.heap.get_mut(receiver)
            else {
                unreachable!()
            };
            // Imported base modules keep their native lookup. Independently constructed modules
            // have no internal import name and continue to carry their constructor.
            *constructor = (target != module || name.is_empty()).then_some(target);
            self.finish_class_change(receiver, source, target);
            return Ok(());
        }
        let object = self.builtin("object");
        let layout = |class| {
            (
                self.allocation_base(class),
                self.class_instances_have_dict(class),
                self.class_instances_support_weakref(class),
                self.has_declared_storage_slots(class),
            )
        };
        let old = layout(source);
        let new = layout(target);
        if old != new {
            return Err(self.class_layout_mismatch(source, target));
        }
        let (allocator, dict, weak, member_slots) = old;
        let raised = matches!(self.heap.get(receiver), Value::Raised(_));
        let Some(base) = allocator
            .filter(|&base| base == object || raised)
            .filter(|_| !member_slots)
        else {
            return Err(Error::new(
                "NotImplementedError",
                "__class__ assignment between native-backed or slotted instance layouts",
            ));
        };
        let (Some(source_base), Some(target_base)) = (
            self.dict_layout_base(source, base, dict, weak),
            self.dict_layout_base(target, base, dict, weak),
        ) else {
            return Err(Error::new(
                "NotImplementedError",
                "unavailable __class__ assignment layout",
            ));
        };
        if source_base != target_base {
            return Err(self.class_layout_mismatch(source, target));
        }
        // Inline ordinary fields are keyed by name. Both they and an exposed __dict__
        // keep their storage and identity when the source and target layouts agree.
        if raised {
            let name = self.class_name(target).to_owned();
            let Value::Raised(error) = self.heap.get_mut(receiver) else {
                unreachable!()
            };
            error.class = target;
            error.kind = name.into();
        } else if let Value::Instance { class, .. } = self.heap.get_mut(receiver) {
            *class = target;
        }
        self.finish_class_change(receiver, source, target);
        Ok(())
    }

    fn class_layout_mismatch(&self, source: Id, target: Id) -> Error {
        Error::new(
            "TypeError",
            format!(
                "__class__ assignment: '{}' object layout differs from '{}'",
                self.class_name(target),
                self.class_name(source)
            ),
        )
    }

    fn finish_class_change(&mut self, receiver: Id, source: Id, target: Id) {
        self.heap.weak_edge_removed(source);
        if self.class_attr(target, "__del__").is_some() {
            self.heap.register_finalizer(receiver);
        } else if self.heap.is_pending_finalizer(receiver) {
            self.heap.take_finalizer(receiver, false);
        }
    }

    /// Reject changes through an immutable instance's object.__class__ descriptor.
    pub(super) fn immutable_class_assignment(&self, value: Option<Id>) -> Error {
        let message = match value {
            None => "can't delete __class__ attribute".to_owned(),
            Some(value) if !self.is_type_object(value) => {
                format!(
                    "__class__ must be set to a class, not '{}' object",
                    self.type_name(value)
                )
            }
            Some(_) => {
                "__class__ assignment only supported for mutable types or ModuleType subclasses"
                    .to_owned()
            }
        };
        Error::new("TypeError", message)
    }

    /// Resolve the field's storage and reject writes to a slot-only instance without that field.
    fn checked_instance_layout(
        &mut self,
        obj: Id,
        class: Id,
        name: &str,
    ) -> Result<InstanceLayout> {
        let layout = self.instance_layout(class, name);
        let exact_tzinfo = self.builtins.get(stdlib::datetime::TZINFO) == Some(&class);
        if layout.dictionary
            || layout.active_slot
            || (!exact_tzinfo && !matches!(self.heap.get(class), Value::Class { .. }))
        {
            return Ok(layout);
        }
        let attribute = self.text(name);
        let quoted = self.repr(attribute);
        let kind = self.type_name(obj);
        // Exact native tzinfo has no stored MRO for object methods.
        let inherited =
            exact_tzinfo && (name == "__doc__" || crate::objects::OBJECT_METHODS.contains(&name));
        let message = if self.class_attr(class, name).is_some() || inherited {
            format!("'{kind}' object attribute {quoted} is read-only")
        } else {
            format!(
                "'{kind}' object has no attribute {quoted} and no __dict__ for setting new attributes"
            )
        };
        Err(Error::new("AttributeError", message))
    }

    pub(super) fn set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<Id> {
        self.set_named_attr(obj, AttributeName::Text(name), value)?;
        Ok(value)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn set_named_attr(
        &mut self,
        obj: Id,
        attr: AttributeName<'_>,
        value: Id,
    ) -> Result<()> {
        let spelling = attr.text(self)?;
        let name = spelling.as_ref();
        let obj = self.weak_proxy_target(obj)?;
        let obj = super::typing::generic_attr_target(self, obj, name, true)?;
        if matches!(self.heap.get(obj), Value::DirEntry(_)) && matches!(name, "name" | "path") {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_local_attribute(obj, attr, Some(value));
        }
        if matches!(self.heap.get(obj), Value::Range(_) | Value::Slice(_))
            && matches!(name, "start" | "stop" | "step")
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(hook) = self.bound_slot(obj, "__setattr__")? {
            let name = attr.key(self);
            self.call(hook, vec![name, value], Vec::new())?;
            return Ok(());
        }
        self.default_object_attribute(obj, attr, ObjectAttribute::Edit(Some(value)))
            .map(|_| ())
    }

    pub(super) fn default_set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<()> {
        if name == "tb_next" && matches!(self.heap.get(obj), Value::Traceback { .. }) {
            return self.set_traceback_next(obj, value);
        }
        self.check_immutable_type_write(obj, name)?;
        if let Some(result) = self.update_alias_name(obj, name, Some(value)) {
            return result;
        }
        if matches!(name, "__name__" | "__qualname__")
            && let Some(frame) = self.generator_name_frame(obj)
        {
            return self.set_generator_name(frame, name, value);
        }
        if let Some(result) = super::paramspec::update(self, obj, name, Some(value)) {
            return result;
        }
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
        self.check_match_attribute_write(obj, name)?;
        self.check_zone_constant_write(obj, name)?;
        self.check_template_write(obj, name)?;
        super::namespace::check_class_write(self, obj, name)?;
        stdlib::io::check_class_write(self, obj, name)?;
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
        if let Value::StaticMethod { dictionary, .. } | Value::ClassMethod { dictionary, .. } =
            *self.heap.get(obj)
        {
            return super::method_decorator::update_attribute(
                self,
                obj,
                dictionary,
                name,
                Some(value),
            );
        }
        #[cfg(unix)]
        if name == "context"
            && matches!(
                self.heap.get(obj),
                Value::TlsSession(..) | Value::TlsBioSession { .. }
            )
        {
            return stdlib::ssl::set_public_context(self, obj, value);
        }
        if name == "cell_contents" && matches!(self.heap.get(obj), Value::Cell(_)) {
            self.heap.replace_cell(obj, Some(value));
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::Function { .. }) {
            return self.set_function_attr(obj, name, value);
        }
        let exception_member = self.exception_member(obj, name);
        let member = self.writable_attribute_member(obj, name)?;
        if let Some(descriptor) = member.or(exception_member)
            && self.data_descriptor(descriptor)
        {
            return self.edit_data_descriptor(descriptor, obj, Some(value));
        }
        if name == "__abstractmethods__"
            && member.is_none()
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            return self.set_abstract_methods(scope, value);
        }
        if (name == "__qualname__" || name == "__name__" && member.is_none())
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            let storage = self.string_storage(value).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "can only assign string to {}.{name}, not '{}'",
                        self.class_name(obj),
                        self.type_name(value)
                    ),
                )
            })?;
            if name == "__qualname__" {
                self.bind(scope, name, value);
                return Ok(());
            }
            if matches!(self.heap.get(storage), Value::SurrogateStr(_)) {
                self.encode_utf8_codepoints(storage, false, "strict")?;
            }
            if self.str_value(storage)?.contains('\0') {
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
        if exception_member.is_some() {
            return self.update_exception_field(obj, name, Some(value));
        }
        if self.update_key_wrapper_field(obj, name, Some(value))? {
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
                Value::Partial { .. } => self.replace_partial_dictionary(obj, value),
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
        if member.is_none()
            && matches!(name, "__name__" | "fget" | "fset" | "fdel")
            && self.heap.get(obj).property().is_some()
        {
            match name {
                "__name__" => {
                    self.replace_property_name(obj, Some(value));
                    return Ok(());
                }
                "fget" | "fset" | "fdel" => {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                _ => {}
            }
        }
        if matches!(self.heap.get(obj), Value::Raised(_)) && name != "__class__" {
            return self.update_exception_field(obj, name, Some(value));
        }
        if member.is_none()
            && self.exposes_defaultdict_factory(obj, name)
            && self.replace_defaultdict_factory(obj, value)
        {
            return Ok(());
        }
        if member.is_none()
            && self.exposes_object_class(obj, name)
            && matches!(
                self.heap.get(obj),
                Value::Instance { .. } | Value::Module { .. } | Value::Raised(_)
            )
        {
            return self.assign_instance_class(obj, Some(value));
        }
        if self.heap.get(obj).native_io_class_name().is_some() && name != "__class__" {
            let dictionary = self.native_file_dictionary(obj);
            return self.update_dictionary_field(obj, dictionary, name, Some(value));
        }
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
            let layout = self.checked_instance_layout(obj, class, name)?;
            if layout.dictionary
                && name == "__dict__"
                && member.is_none()
                && !self.layout_descriptors_cleared(class)
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
                self.replace_instance_dictionary(obj, class, Some(value));
                return Ok(());
            }
            if !layout.active_slot {
                let storage = namespace.or_else(|| {
                    (layout.materialize_dictionary_on_write || layout.declared_slot)
                        .then(|| self.instance_dictionary(obj, class))
                });
                if let Some(storage) = storage {
                    return self.update_dictionary_field(obj, storage, name, Some(value));
                }
                self.register_stream_with_attributes(obj, class);
            }
        }
        let object_class = self.builtins.get("object").copied();
        match self.heap.get_mut(obj) {
            Value::Property(property) if name == "__doc__" => property.doc = value,
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Module { scope, .. } => {
                if name == "__dict__" {
                    return Err(Error::new("AttributeError", "readonly attribute"));
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
            Value::Native(_) if let Some(&scope) = self.enum_scopes.get(&obj) => {
                self.update_enum_base(scope, name, Some(value))?;
            }
            Value::Class { scope, .. } => {
                // A non-data metaclass member shadows the native type descriptor.
                if member.is_none() {
                    match name {
                        "__base__" | "__basicsize__" | "__itemsize__" | "__flags__"
                        | "__weakrefoffset__" | "__dictoffset__" => {
                            return Err(Error::new("AttributeError", "readonly attribute"));
                        }
                        "__dict__" | "__mro__" | "__text_signature__" => {
                            return Err(Error::new(
                                "AttributeError",
                                format!("attribute '{name}' of 'type' objects is not writable"),
                            ));
                        }
                        "__bases__" => return self.assign_class_bases(obj, value),
                        _ => {}
                    }
                }
                let scope = *scope;
                if matches!(name, "__annotations__" | "__annotate__")
                    && member.is_none()
                    && self.has_class_annotation_provider(scope)
                {
                    return self.update_class_annotation_attr(scope, name, Some(value));
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
            itertools: None,
            member_values: None,
            socket: None,
            memory_stream: None,
            fields: Default::default(),
            scalar: None,
        })
    }

    /// The first non-Python base owns the instance layout. Native classes
    /// represented with a class namespace (e.g. BytesIO) have no Python class cell.
    pub(super) fn allocation_base(&self, class: Id) -> Option<Id> {
        if self.builtins.get("typing.Any") == Some(&class) {
            return self.builtins.get("object").copied();
        }
        self.find_in_mro(class, |base| self.owns_allocation(base).then_some(base))
    }

    pub(super) fn owns_allocation(&self, base: Id) -> bool {
        match self.heap.get(base) {
            // Any is a Python class with ordinary instance storage despite its native identity.
            Value::Native(name) => name != "typing.Any",
            Value::Class { .. } if self.itertools_layout(base).is_some() => true,
            // These bootstrapped classes inherit their allocator from the next base.
            Value::Class { scope, .. } => {
                self.scope_data(*scope).class_cell.is_none()
                    && self.classes.get("collections.abc.Callable") != Some(&base)
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

    /// Invert only the receiver's equality result, preserving a declined comparison.
    pub(super) fn object_ne(&mut self, object: Id, other: Id) -> Result<Id> {
        let class = self.exact_type(object)?;
        if let Some(slot) = self.class_attr(class, "__eq__") {
            // Re-entering the same native descriptor cannot reach a Python equality hook.
            if self.builtins.get("object.__ne__") == Some(&slot) {
                return Err(Error::new(
                    "RecursionError",
                    "maximum recursion depth exceeded in comparison",
                ));
            }
            let method = self.bind_attribute(slot, object, class)?;
            let equal = self.call(method, vec![other], Vec::new())?;
            return if equal == value::NOT_IMPLEMENTED {
                Ok(equal)
            } else {
                Ok(builtins::boolean(!self.truth(equal)?))
            };
        }
        if self.uses_object_new(self.allocation_base(class)) {
            return Ok(if object == other {
                FALSE
            } else {
                value::NOT_IMPLEMENTED
            });
        }
        Err(Error::new(
            "NotImplementedError",
            format!(
                "object.__ne__(): the equality slot of '{}' is unsupported",
                self.type_name(object)
            ),
        ))
    }

    /// An intrinsic allocator consumes the constructor arguments only when the MRO
    /// leaves initialization to object. Some native descriptors are exposed lazily.
    pub(super) fn inherits_native_object_init(&self, class: Id) -> bool {
        let Some(layout) = self.allocation_base(class) else {
            return false;
        };
        let Value::Native(owner) = self.heap.get(layout) else {
            return false;
        };
        if self.builtins.get("object") == Some(&layout) || !builtins::owns_native_new(owner) {
            return false;
        }
        match self.class_attr(class, "__init__") {
            Some(init) => self.builtins.get("object.__init__") == Some(&init),
            None => wrapper_descriptor::BuiltinDescriptor::lookup(&format!("{owner}.__init__"))
                .is_none(),
        }
    }

    /// These native layouts inherit object.__new__ for bare instance allocation.
    pub(super) fn uses_object_new(&self, base: Option<Id>) -> bool {
        base == self.builtins.get("object").copied()
            || base.is_some_and(|base| {
                matches!(
                    self.heap.get(base).io_kind(),
                    Some(
                        stdlib::io::NEWLINE_DECODER
                            | "BufferedRWPair"
                            | "BufferedReader"
                            | "BufferedWriter"
                            | "BufferedRandom"
                            | "TextIOWrapper"
                            | "IOBase"
                            | "BufferedIOBase"
                    )
                )
            })
    }

    /// These class objects expose the inherited object.__new__ but cannot allocate instances.
    fn nonconstructible_heap_type(&self, class: Id) -> Option<&'static str> {
        let value = self.heap.get(class);
        if matches!(value, Value::Class { .. }) && self.classes.get("typing.Union") == Some(&class)
        {
            return Some("typing.Union");
        }
        if value.heap_type() == Some(stdlib::unicodedata::CLASS) {
            return Some(stdlib::unicodedata::CLASS);
        }
        value.hash_type()
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
        if let Some(name @ ("_ssl.MemoryBIO" | stdlib::datetime::isocalendar::CLASS)) =
            self.heap.get(class).heap_type()
        {
            return Err(Error::new(
                "TypeError",
                format!("object.__new__({name}) is not safe, use {name}.__new__()"),
            ));
        }
        if let Some(name) = self.nonconstructible_heap_type(class) {
            return Err(Error::new(
                "TypeError",
                format!("cannot create '{name}' instances"),
            ));
        }
        let object = self.builtin("object");
        if let Value::Native(name) = self.heap.get(class)
            && (nonconstructible_result_type(name)
                || matches!(
                    name.as_str(),
                    stdlib::os::scandir::ENTRY | "_io._BytesIOBuffer"
                ))
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
        if !self.uses_object_new(Some(base)) {
            let name = self
                .itertools_layout(class)
                .unwrap_or_else(|| self.class_name(class));
            let owner = if let Some(layout) = self.itertools_layout(base) {
                // Bundled cursor constructors implement a native allocator in Python.
                // An inherited allocator is reported on the requested subtype.
                let callable = |value| match *self.heap.get(value) {
                    Value::StaticMethod { callable, .. } => callable,
                    _ => value,
                };
                if new.map(callable) == self.class_attr(base, "__new__").map(callable) {
                    name
                } else {
                    layout
                }
            } else if new.is_some_and(|new| !matches!(self.heap.get(new), Value::Native(_))) {
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

    /// Allocate native storage while leaving constructor arguments to the initializer.
    fn new_mutable_scalar(&mut self, kind: &str, args: &[Id], empty: Value) -> Result<Id> {
        let Some(&class) = args.first() else {
            return Err(Error::new(
                "TypeError",
                format!("{kind}.__new__(): not enough arguments"),
            ));
        };
        let base = self.builtin(kind);
        if !self.direct_subclass(class, base) {
            return Err(self.invalid_new_subtype(kind, class));
        }
        let storage = self.alloc(empty);
        Ok(self.with_subclass_storage(class, base, storage))
    }

    /// Attach native storage to a Python subclass, keeping the exact builtin unwrapped.
    pub(super) fn with_subclass_storage(&mut self, class: Id, base: Id, storage: Id) -> Id {
        if class == base {
            return storage;
        }
        let instance = self.instance(class, None);
        let Value::Instance { scalar, .. } = self.heap.get_mut(instance) else {
            unreachable!()
        };
        *scalar = Some(storage);
        instance
    }

    /// Allocate the requested tuple subtype from the iterable before invoking any initializer.
    fn new_tuple(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&class, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "tuple.__new__(): not enough arguments",
            ));
        };
        let name = match self.heap.get(class) {
            Value::Native(name) => Some(name.as_str()),
            class => class.heap_type(),
        };
        match name {
            Some(stdlib::flags::TYPE) => {
                return Err(Error::new(
                    "TypeError",
                    "cannot create 'sys.flags' instances",
                ));
            }
            Some(
                name @ (stdlib::fixed_structseq::SCHED_PARAM
                | stdlib::datetime::isocalendar::CLASS
                | stdlib::time::STRUCT_TIME),
            ) => {
                return Err(Error::new(
                    "TypeError",
                    format!("tuple.__new__({name}) is not safe, use {name}.__new__()"),
                ));
            }
            _ => {}
        }
        if !kwargs.is_empty() {
            let tuple = self.builtin("tuple");
            if self.subclass(class, tuple)
                && (class == tuple || self.class_attr(class, "__init__").is_none_or(|init| {
                    matches!(self.heap.get(init), Value::Native(name) if name == "object.__init__")
                }))
            {
                return Err(Error::new("TypeError", "tuple() takes no keyword arguments"));
            }
        }
        self.new_immutable_scalar("tuple", class, args, &[])
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
            heap_type if let Some(name) = heap_type.heap_type() => {
                format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
            }
            Value::Class { name, .. } => {
                format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
            }
            Value::Native(name) if self.is_type_object(class) => {
                format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
            }
            _ => format!(
                "{kind}.__new__(X): X is not a type object ({})",
                self.type_name(class)
            ),
        };
        Error::new("TypeError", message)
    }

    /// Construct the builtin value and retain the requested Python class for a subtype.
    pub(super) fn new_immutable_scalar(
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
        let value = builtins::call(self, kind, args, kwargs)?;
        if class == base {
            return Ok(value);
        }
        let storage = self.scalar_value(value);
        Ok(self.alloc(Value::Instance {
            namespace: None,
            class,
            fields: Default::default(),
            dictionary: None,
            scalar: Some(storage),
            property: None,
            weak_reference: None,
            itertools: None,
            member_values: None,
            socket: None,
            memory_stream: None,
        }))
    }

    /// Initialize a class returned by __new__ when it is an instance of the called metaclass.
    pub(super) fn finish_type_call(
        &mut self,
        caller: Id,
        result: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        let actual = self.exact_type(result)?;
        if self.direct_subclass(actual, caller)
            && let Some(init) = self.class_attr(actual, "__init__")
        {
            self.call_initializer(init, result, actual, args, kwargs)?;
        }
        Ok(result)
    }

    /// Invoke the initializer found on the type, including its descriptor binding.
    pub(super) fn call_initializer(
        &mut self,
        init: Id,
        instance: Id,
        class: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<()> {
        let init = self.bind_attribute(init, instance, class)?;
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
        Ok(())
    }

    /// Include Python frames represented by native operations in the active call depth.
    pub(super) fn check_call_depth(&self, additional: usize) -> Result<()> {
        if self.call_depth.max(self.heap.python_depth()) + self.suspended_typing_frames + additional
            >= crate::sys::recursion_limit() as usize
        {
            return Err(Error::new(
                "RecursionError",
                "maximum recursion depth exceeded",
            ));
        }
        Ok(())
    }

    /// Return the bound Python frame to the caller that owns execution of this heap.
    pub(super) fn prepare_call(
        &mut self,
        mut callable: Id,
        mut args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<RegisteredCall> {
        loop {
            match *self.heap.get(callable) {
                Value::Function { .. } => {
                    return self.prepare_function_call(
                        callable,
                        FunctionArguments::Written(args, kwargs, None),
                    );
                }
                Value::Method {
                    callable: target,
                    receiver,
                    ..
                } => {
                    args = method_arguments(receiver, args)?;
                    callable = target;
                }
                Value::HeapTypeMethod { owner, name, .. }
                    if self.heap.get(owner).heap_type() == Some(stdlib::socket::CLASS) =>
                {
                    return stdlib::socket::prepare_descriptor(self, owner, name, &args, &kwargs);
                }
                Value::Native(ref name) => {
                    let wait = match name.as_str() {
                        "_imre_thread.start" => {
                            return thread_worker::prepare_start(self, &args, &kwargs)
                                .map(|start| RegisteredCall::Start(Box::new(start)))
                                .map_err(|error| stdlib::io::qualify_error(self, error, None));
                        }
                        "_thread.start_new_thread" => {
                            return super::lowlevel_threads::prepare_start(self, &args, &kwargs)
                                .map(|start| RegisteredCall::Start(Box::new(start)))
                                .map_err(|error| stdlib::io::qualify_error(self, error, None));
                        }
                        "_thread._ThreadHandle.join" => {
                            return thread_handle::prepare_join(self, &args, &kwargs);
                        }
                        "_thread.RLock._acquire_restore" => {
                            return thread_rlock::Restore::prepare(self, &args, &kwargs)
                                .map(RegisteredCall::Restore)
                                .map_err(|error| stdlib::io::qualify_error(self, error, None));
                        }
                        "_thread.lock.release" => {
                            return thread_lock::prepare_release(self, &args, &kwargs)
                                .map(RegisteredCall::Release);
                        }
                        "_thread.lock.acquire" => {
                            thread_lock::Acquire::prepare(self, "acquire", &args, &kwargs)
                        }
                        "_thread.lock.__enter__" => {
                            thread_lock::Acquire::prepare(self, "__enter__", &args, &kwargs)
                        }
                        "_thread.RLock.acquire" => {
                            thread_rlock::prepare_acquire(self, "acquire", &args, &kwargs)
                        }
                        "_thread.RLock.__enter__" => {
                            thread_rlock::prepare_acquire(self, "__enter__", &args, &kwargs)
                        }
                        _ => break,
                    };
                    return wait
                        .map(RegisteredCall::Acquire)
                        .map_err(|error| stdlib::io::qualify_error(self, error, None));
                }
                _ => break,
            }
        }
        self.call_other(callable, args, kwargs)
            .map(RegisteredCall::Returned)
    }

    /// Bind the call's private keyword dictionary directly when every key belongs in **kwargs.
    pub(super) fn prepare_call_with_keyword_dict(
        &mut self,
        scope: Id,
        callable: Id,
        args: Vec<Id>,
        mapping: Id,
    ) -> Result<RegisteredCall> {
        let (callable, args) = if matches!(self.heap.get(callable), Value::Native(name) if name == "operator.call")
        {
            match self.operator_call_target(args) {
                Ok(target) => target,
                Err(error) => {
                    self.validate_keyword_keys(mapping)?;
                    return Err(error);
                }
            }
        } else {
            (callable, args)
        };
        if let Value::Function { code, .. } = *self.heap.get(callable)
            && self.code_object(code).parameters != NONE
        {
            let sig = self.parsed_parameters(code)?;
            if sig.kwargs.is_some() && sig.positional.is_empty() && sig.keyword_only.is_empty() {
                self.validate_keyword_keys(mapping)?;
                return self.prepare_function_call(
                    callable,
                    FunctionArguments::Written(args, Vec::new(), Some(mapping)),
                );
            }
        }
        let kwargs = self.keyword_args(mapping)?;
        self.prepare_call_in_scope(scope, callable, args, kwargs)
    }

    fn validate_keyword_keys(&self, mapping: Id) -> Result<()> {
        let Value::Dict(entries) = self.heap.get(mapping) else {
            return Err(Error::new("TypeError", "expected keyword dict"));
        };
        if entries
            .iter()
            .any(|&(key, _)| self.string_storage(key).is_none())
        {
            return Err(Error::new("TypeError", "keywords must be strings"));
        }
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        callable: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        let call = self.prepare_call(callable, args, kwargs)?;
        // SAFETY: the call was prepared in this context for synchronous execution.
        unsafe { call.run(self) }
    }

    fn call_other(
        &mut self,
        callable: Id,
        mut args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        // An explicit type.__call__ invokes the ordinary constructor without
        // dispatching back to an override on the receiver's metaclass.
        let default_type_call =
            matches!(self.heap.get(callable), Value::Native(name) if name == "type.__call__");
        let callable = if default_type_call {
            if args.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "descriptor '__call__' of 'type' object needs an argument",
                ));
            }
            let receiver = args.remove(0);
            if !self.is_type_object(receiver) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '__call__' requires a 'type' object but received a '{}'",
                        self.builtin_type_name(receiver)
                            .unwrap_or_else(|| self.type_name(receiver))
                    ),
                ));
            }
            receiver
        } else {
            callable
        };
        if let Value::FractionHash(function) = *self.heap.get(callable) {
            return stdlib::fraction_hash(self, function, args, kwargs);
        }
        if let Value::ConstEvaluator(value) = *self.heap.get(callable) {
            return super::paramspec::eval(self, value, &args, &kwargs);
        }
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
        #[cfg(unix)]
        if self.heap.get(callable).heap_type() == Some(stdlib::ssl::provider::bio::CLASS) {
            return stdlib::ssl::provider::bio::construct(self, callable, &args, &kwargs);
        }
        if let Some(name) = self.nonconstructible_heap_type(callable) {
            return Err(Error::new(
                "TypeError",
                format!("cannot create '{name}' instances"),
            ));
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
                heap_type,
                pattern_flags,
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
                    heap_type: *heap_type,
                    pattern_flags: *pattern_flags,
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
                if let Err(error) = self.set_attr(result, "__orig_class__", callable) {
                    let ignored = if alias.is_builtin_alias() {
                        self.is_attribute_error(&error)
                            || self.is_builtin_error(&error, "TypeError")
                    } else {
                        self.is_builtin_error(&error, "Exception")
                    };
                    if !ignored {
                        return Err(error);
                    }
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
            Value::Native(name) if name == "super.__get__" => {
                wrapper_descriptor::descriptor_get(self, "super", &args, &kwargs)
            }
            Value::Native(name) if name == "method.__get__" => {
                wrapper_descriptor::descriptor_get(self, "method", &args, &kwargs)
            }
            Value::Native(name) if name == "function.__get__" => {
                wrapper_descriptor::descriptor_get(self, "function", &args, &kwargs)
            }
            Value::Native(name) if name == "functools._lru_cache_wrapper.__get__" => {
                wrapper_descriptor::descriptor_get(self, LRU_CACHE_CLASS, &args, &kwargs)
            }
            Value::Native(name) if name == "functools.partial.__get__" => {
                wrapper_descriptor::descriptor_get(self, "functools.partial", &args, &kwargs)
            }
            Value::Native(name) if name == "classmethod.__get__" => {
                wrapper_descriptor::descriptor_get(self, "classmethod", &args, &kwargs)
            }
            Value::Native(name) if name == "wrapper_descriptor.__get__" => {
                wrapper_descriptor::descriptor_get(self, "wrapper_descriptor", &args, &kwargs)
            }
            Value::Native(name) if name == "classmethod_descriptor.__get__" => {
                wrapper_descriptor::descriptor_get(self, "classmethod_descriptor", &args, &kwargs)
            }
            Value::Native(name) if name == "method_descriptor.__get__" => {
                wrapper_descriptor::descriptor_get(self, "method_descriptor", &args, &kwargs)
            }
            Value::Native(ref name)
                if let Some((owner, _)) = name
                    .strip_suffix(".__call__")
                    .and_then(stdlib::operator::callable_type) =>
            {
                let Some((&receiver, rest)) = args.split_first() else {
                    return Err(Error::new(
                        "TypeError",
                        format!("descriptor '__call__' of '{owner}' object needs an argument"),
                    ));
                };
                if !self.builtin_slot_accepts(receiver, owner) {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "descriptor '__call__' requires a '{owner}' object but received a '{}'",
                            self.type_name(receiver)
                        ),
                    ));
                }
                self.call(receiver, rest.to_vec(), kwargs)
            }
            Value::Native(name) if name == stdlib::flags::TYPE => Err(Error::new(
                "TypeError",
                "cannot create 'sys.flags' instances",
            )),
            Value::Native(ref name)
                if let Some(method) = name.strip_prefix("posix.sched_param.") =>
            {
                stdlib::fixed_structseq::sched_param_method(self, method, &args, &kwargs)
            }
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
            Value::Native(name) if name == "type.__new__" => self.type_new(&args, &kwargs),
            Value::Native(name) if name == "type.__prepare__" => Ok(self.dict(Vec::new())),
            Value::Native(name) if name == "type.mro" => self.type_mro(&args, &kwargs),
            Value::Native(name) if name == "type.__subclasses__" => {
                self.type_subclasses(&args, &kwargs)
            }
            Value::Native(ref name)
                if let Some((owner, "__new__")) = name.rsplit_once('.')
                    && let Some(owner) = stdlib::itertools::cursor_type(owner) =>
            {
                stdlib::itertools::allocate(self, owner, &args, &kwargs)
            }
            Value::Native(ref name)
                if let Some((owner, "__new__")) = name.rsplit_once('.')
                    && let Some(owner) = super::deque::iterator_class(owner) =>
            {
                let Some((&class, rest)) = args.split_first() else {
                    return Err(Error::new(
                        "TypeError",
                        format!("{owner}.__new__(): not enough arguments"),
                    ));
                };
                if class != self.builtin(owner) {
                    return Err(self.invalid_new_subtype(owner, class));
                }
                super::deque::construct_iterator(self, owner, rest)
            }
            Value::Native(name) if name == "object.__new__" => self.new_object(&args, &kwargs),
            Value::Native(name) if name == "dict.__new__" => self.new_dict(&args),
            Value::Native(name) if name == "list.__new__" => {
                self.new_mutable_scalar("list", &args, Value::List(Vec::new()))
            }
            Value::Native(name) if name == "struct.Struct.__new__" => self.new_mutable_scalar(
                "struct.Struct",
                &args,
                Value::Struct {
                    format: String::new(),
                    layout: None,
                },
            ),
            Value::Native(name) if name == "_thread._local.__new__" => {
                super::thread_local::allocate(self, &args, &kwargs)
            }
            Value::Native(name) if name == "tuple.__new__" => self.new_tuple(&args, &kwargs),
            Value::Native(name)
                if name
                    .strip_suffix(".__sizeof__")
                    .and_then(object_size::descriptor)
                    .is_some() =>
            {
                self.native_sizeof(name.trim_end_matches(".__sizeof__"), &args, &kwargs)
            }
            Value::Native(ref name)
                if let Some(owner) = name.strip_suffix(".__reduce__")
                    && builtins::is_exception(owner) =>
            {
                let object =
                    builtins::noargs_method_receiver(self, owner, "__reduce__", &args, &kwargs)?;
                if matches!(owner, "BaseException" | "ImportError") {
                    self.reduce_exception(object, owner)
                } else {
                    // These classes declare reducers for additional native fields.
                    Err(stdlib::pickle::unsupported_reducer(self, object))
                }
            }
            Value::Native(name) if name == "BaseException.__setstate__" => {
                let (object, rest) = builtins::method_receiver_args(
                    self,
                    "BaseException",
                    "__setstate__",
                    &args,
                    &kwargs,
                )?;
                let state = builtins::one_positional_arg(&name, rest, &kwargs)?;
                self.set_exception_state(object, state)?;
                Ok(NONE)
            }
            Value::Native(name) if name == "object.__getstate__" => {
                self.object_getstate(&args, &kwargs)
            }
            Value::Native(name)
                if matches!(name.as_str(), "object.__reduce__" | "object.__reduce_ex__") =>
            {
                stdlib::pickle::object_reduce(self, &name["object.".len()..], &args, &kwargs)
            }
            Value::Native(ref name)
                if let Some(method) = name.strip_prefix("BaseExceptionGroup.")
                    && super::exception_group::METHODS.contains(&method) =>
            {
                super::exception_group::call(self, method, &args, &kwargs)
            }
            Value::Native(name) if builtins::is_enum_base(&name) => {
                self.functional_enum(callable, args, kwargs)
            }
            Value::Native(ref name)
                if let Some((value::STRUCT_UNPACK_ITERATOR, method)) = name.rsplit_once('.')
                    && builtins::UNPACK_ITERATOR_METHODS.contains(&method) =>
            {
                self.iterator_method(value::STRUCT_UNPACK_ITERATOR, method, &args, &kwargs)
            }
            Value::Native(name) if nonconstructible_result_type(&name) => Err(Error::new(
                "TypeError",
                format!("cannot create '{name}' instances"),
            )),
            Value::Native(name)
                if builtins::is_builtin(&name)
                    || builtins::constructor_owner(&name).is_some_and(|owner| {
                        !HashState::is_provider_class(owner) && owner != super::thread_handle::CLASS
                    })
                    || builtins::exception_init_descriptor(&name).is_some()
                    || matches!(
                        name.as_str(),
                        "builtin_function_or_method.__reduce__"
                            | "method.__reduce__"
                            | "method-wrapper.__reduce__"
                            | "type.__init__"
                            | "type.__setattr__"
                            | "BaseException.add_note"
                            | "float.__format__"
                            | "float.__getformat__"
                            | "float.fromhex"
                            | "bool.__repr__"
                            | "float.__repr__"
                            | "float.as_integer_ratio"
                            | "getset_descriptor.__set__"
                            | "getset_descriptor.__delete__"
                    )
                    || name
                        .strip_prefix("complex.")
                        .is_some_and(|method| super::complex::METHODS.contains(&method))
                    || name.strip_prefix("float.").is_some_and(|method| {
                        builtins::real_methods("float").contains(&method)
                    })
                    || !name.contains('.')
                    || matches!(
                        name.split('.').next(),
                        Some(
                            "str"
                                | "super"
                                | "enumerate"
                                | "generator"
                                | "memory_iterator"
                                | "coroutine"
                                | "coroutine_wrapper"
                                | "anext_awaitable"
                                | "async_generator"
                                | "async_generator_asend"
                                | "async_generator_athrow"
                                | "dict"
                                | "list"
                                | "list_iterator"
                                | "set_iterator"
                                | "str_ascii_iterator"
                                | "str_iterator"
                                | "tuple_iterator"
                                | "map"
                                | "zip"
                                | "filter"
                                | "mappingproxy"
                                | "set"
                                | "int"
                                | "_int_getset"
                                | "_float_getset"
                                | "_code_local"
                                | "_dictview_mapping"
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
            Value::Native(ref name)
                if let Some(method) = name.strip_prefix("FrameLocalsProxy.") =>
            {
                super::frame_locals::method(self, method, &args, &kwargs)
            }
            Value::Native(name) => stdlib::call(self, &name, &args, &kwargs)
                .map_err(|error| stdlib::io::qualify_error(self, error, None)),
            Value::StaticMethod { callable, .. } => self.call(callable, args, kwargs),
            Value::KeyWrapper { comparator, .. } => {
                let object = stdlib::functools::key_argument("K", "obj", &args, &kwargs)?;
                Ok(self.alloc(Value::KeyWrapper {
                    comparator,
                    object: Some(object),
                }))
            }
            Value::CodecEncoder(codec) => stdlib::codecs::entry_encode(self, codec, &args, &kwargs),
            Value::CodecDecoder(codec) => stdlib::codecs::decode(self, codec, &args, &kwargs),
            Value::HeapTypeMethod { owner, name, .. } => {
                if self.heap.get(owner).heap_type() == Some(stdlib::unicodedata::CLASS) {
                    return stdlib::unicodedata::legacy_method(self, owner, name, &args, &kwargs);
                }
                #[cfg(unix)]
                if self.heap.get(owner).heap_type() == Some(value::SSL_ERROR_TYPE) {
                    return stdlib::ssl::provider::objects::error_str(self, owner, &args, &kwargs);
                }
                #[cfg(unix)]
                if self.heap.get(owner).heap_type() == Some(stdlib::ssl::provider::bio::CLASS) {
                    return stdlib::ssl::provider::bio::call(self, owner, name, &args, &kwargs);
                }
                if self.heap.get(owner).heap_type() == Some(stdlib::random::instance::CLASS) {
                    return stdlib::random::instance::call(self, owner, name, &args, &kwargs);
                }
                if self.heap.get(owner).heap_type() == Some(stdlib::datetime::isocalendar::CLASS) {
                    stdlib::datetime::isocalendar::call(self, owner, name, &args, &kwargs)
                } else {
                    stdlib::hashlib::md5::call(self, owner, name, &args, &kwargs)
                }
            }
            Value::IoOpen { types, unsupported } => {
                let (_, arguments) = args
                    .split_first()
                    .ok_or_else(|| Error::new("TypeError", "_io.open needs its module"))?;
                stdlib::io::open(self, arguments, &kwargs, Some(types))
                    .map_err(|error| stdlib::io::qualify_error(self, error, Some(unsupported)))
            }
            Value::IoMethod {
                owner,
                name,
                unsupported,
            } => stdlib::io::call(self, owner, name, &args, &kwargs)
                .map_err(|error| stdlib::io::qualify_error(self, error, Some(unsupported))),
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
                let bound_keyword_count = merged.len();
                if kwargs.len() >= 8 && merged.len().saturating_add(kwargs.len()) >= 64 {
                    stdlib::functools::merge_wide_keywords(&mut merged, kwargs)?;
                } else {
                    for (key, value) in kwargs {
                        if let Some((_, previous)) = merged.iter_mut().find(|(k, _)| k == &key) {
                            *previous = value;
                        } else {
                            merged.push((key, value));
                        }
                    }
                }
                if bound_keyword_count == 0 {
                    return self.call(function, positional, merged);
                }
                // partial_call owns its merged arguments while the callee can change
                // the partial's state. An overridden keyword still uses its original key.
                let mark = self.heap.temporary_mark();
                self.heap.reserve_temporaries(
                    1 + positional.len() + merged.len() + bound_keyword_count,
                )?;
                self.heap.keep(function);
                for &id in &positional {
                    self.heap.keep(id);
                }
                for &(_, id) in &merged {
                    self.heap.keep(id);
                }
                for i in 0..bound_keyword_count {
                    let key = self
                        .dict_entries(keywords)
                        .expect("partial keyword dictionary")[i]
                        .0;
                    self.heap.keep(key);
                }
                let result = self.call(function, positional, merged);
                self.release_with_result(mark, result)
            }
            Value::AttrGetter(paths) => self.apply_attrgetter(&paths, &args, &kwargs),
            Value::ItemGetter(keys) => self.apply_itemgetter(keys, &args, &kwargs),
            Value::MethodCaller { .. } => self.apply_methodcaller(callable, &args, &kwargs),
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
                if !default_type_call
                    && let Some(meta) = metaclass
                    && let Some(slot) = self.class_attr(meta, "__call__")
                    && self.builtins.get("type.__call__") != Some(&slot)
                {
                    return self.with_slot_depth(
                        "maximum recursion depth exceeded while calling a Python object",
                        |ctx| {
                            let call = ctx.bind_attribute(slot, callable, meta)?;
                            ctx.call(call, args, kwargs)
                        },
                    );
                }
                if let [input] = args.as_slice()
                    && kwargs.is_empty()
                    && self.itertools_layout(callable) == Some("itertools.pairwise")
                {
                    let source = self.iterator(*input)?;
                    return Ok(stdlib::itertools::pairwise_cursor(
                        self, callable, callable, source,
                    ));
                }
                let (mut args, mut kwargs) = (args, kwargs);
                let value = match (&mut args[..], &mut kwargs[..]) {
                    ([value], []) => Some(value),
                    ([], [(key, value)]) if key == "value" => Some(value),
                    _ => None,
                };
                if let Some(value) = value
                    && let Some(decimal) = std::iter::once(callable)
                        .chain(ancestors.iter().copied())
                        .find(|&base| self.is_bundled_decimal(base))
                    && (callable == decimal
                        || (self.class_attr(callable, "__new__").is_none()
                            && self.class_attr(callable, "__init__")
                                == self.class_attr(decimal, "__init__")))
                {
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
                if let Some(values) = self.enum_values(callable) {
                    return self.enum_lookup(callable, values, &args, &kwargs);
                }
                if self.scope_data(scope).is_abstract {
                    return Err(self.abstract_class_error(scope, &name)?);
                }
                if self.is_protocol_declaration(callable)
                    && self.class_attr(callable, "__init__").is_none()
                {
                    return Err(Error::new("TypeError", "Protocols cannot be instantiated"));
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
                    [
                        ("int", "int.__new__"),
                        ("tuple", "tuple.__new__"),
                        ("type", "type.__new__"),
                    ]
                    .into_iter()
                    .find(|(base, _)| self.has_base(callable, base))
                    .map(|(_, new)| self.builtin(new))
                });
                let scalar_base = ["str", "bytes", "complex", "float"]
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
                let rlock_subclass = self.has_base(callable, super::thread_rlock::CLASS);
                let collection_kind = imre_ast::class_attributes::COLLECTION_STORAGE_BASES
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
                let instance = if let Some(new) = new {
                    let allocate = self.bind_attribute(new, NONE, callable)?;
                    let positional = std::iter::once(callable)
                        .chain(args.iter().copied())
                        .collect();
                    let instance = self.call(allocate, positional, kwargs.clone())?;
                    if self.has_base(callable, "type") {
                        return self.finish_type_call(callable, instance, args, kwargs);
                    }
                    if !self.instance_of(instance, callable) {
                        return Ok(instance);
                    }
                    instance
                } else if self.has_base(callable, "module") {
                    self.allocate_module(callable)
                } else if let Some(kind) = scalar_base {
                    self.new_immutable_scalar(kind, callable, &args, &kwargs)?
                } else {
                    let storage = if rlock_subclass {
                        Some(self.alloc(Value::HandlerLock(Default::default())))
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
                    self.call_initializer(init, instance, actual, args, kwargs)?;
                } else if new.is_none()
                    && !native_tzinfo_new
                    && scalar_base.is_none()
                    && !rlock_subclass
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
            _ => Err(non_callable_error(self.type_name(callable))),
        }
    }

    /// Read a default selected by the Python argument binder from the function's live storage.
    pub(super) fn bound_default(
        &self,
        shape: &crate::source_arguments::Shape<'_, String>,
        slot: usize,
        defaults: Id,
        kwdefaults: Id,
    ) -> Id {
        if slot < shape.positional.len() {
            let values = self
                .tuple_elements(defaults)
                .expect("bound positional default");
            values[slot + values.len() - shape.positional.len()]
        } else {
            self.dict_named_item(kwdefaults, shape.parameter(slot))
                .expect("bound keyword default")
        }
    }

    pub(super) fn prepare_function_call(
        &mut self,
        callable: Id,
        arguments: FunctionArguments,
    ) -> Result<RegisteredCall> {
        let Value::Function {
            ref fields,
            scope,
            class_cell,
            code,
            defaults,
            kwdefaults,
            ..
        } = *self.heap.get(callable)
        else {
            unreachable!("registered Python function");
        };
        let (argc, kwargs) = match &arguments {
            FunctionArguments::Written(positional, keywords, _) => {
                (positional.len(), keywords.as_slice())
            }
            FunctionArguments::Bound(slots) => (slots.len(), &[][..]),
        };
        let parameters = self.code_object(code).parameters;
        let name = self.copy_name(fields.qualname)?;
        if parameters == NONE {
            if let Some((key, _)) = kwargs.first() {
                return Err(Error::new(
                    "TypeError",
                    format!("{name}() got an unexpected keyword argument '{key}'"),
                ));
            }
            if argc != 0 {
                let verb = if argc == 1 { "was" } else { "were" };
                return Err(Error::new(
                    "TypeError",
                    format!("{name}() takes 0 positional arguments but {argc} {verb} given"),
                ));
            }
            let globals = self.module_globals(scope)?;
            return self.prepare_namespace_code(code, globals, None, Some(callable), "exec", None);
        }
        // Root the scope, packed arguments, keyword names, and returning value.
        self.heap
            .reserve_temporaries(kwargs.len().saturating_add(4))?;
        let module = copy_label(&self.scope_data(scope).module)?;
        let local = allocator_api2::boxed::Box::try_new(Scope::new(module, Some(scope)))
            .map_err(|_| Error::new("MemoryError", ""))?;
        let local = self.alloc(Value::Scope(local));
        let sig = self.parsed_parameters(code)?;
        let Value::Scope(frame) = self.heap.get_mut(local) else {
            unreachable!()
        };
        frame.closure.declared = sig.locals.clone();
        frame
            .closure
            .nonlocals
            .try_reserve(sig.nonlocals.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        frame.closure.declared_complete = sig.locals_complete;
        for name in &sig.nonlocals {
            frame.closure.nonlocals.insert(copy_label(name)?);
        }

        let shape = sig.shape();
        let omitted = match arguments {
            FunctionArguments::Written(args, kwargs, variadic) => {
                let pos = &sig.positional;
                let num_defaults = self.tuple_elements(defaults).map_or(0, <[_]>::len);
                let keys = kwargs.iter().map(|(key, _)| key.as_str());
                let binding = shape.bind(&name, args.len(), keys, num_defaults, |n| {
                    self.dict_named_item(kwdefaults, n).is_some()
                })?;
                for (n, &v) in pos.iter().zip(&args) {
                    self.bind_argument(local, n, v)?;
                }
                if let Some(n) = sig.varargs.as_deref() {
                    let extra = if pos.is_empty() {
                        args
                    } else {
                        copy_ids(args.get(pos.len()..).unwrap_or_default())?
                    };
                    let extra = self.alloc(Value::Tuple(extra));
                    self.bind_argument(local, n, extra)?;
                }
                let mut extra = Vec::new();
                for ((key, value), slot) in kwargs.into_iter().zip(binding.keywords) {
                    if let Some(slot) = slot {
                        self.bind_argument(local, shape.parameter(slot), value)?;
                    } else {
                        let k = self.text(key);
                        extra.push((k, value));
                    }
                }
                if let Some(n) = sig.kwargs.as_deref() {
                    let v = variadic.unwrap_or_else(|| self.dict(extra));
                    self.bind_argument(local, n, v)?;
                }
                binding.defaults
            }
            FunctionArguments::Bound(slots) => {
                debug_assert_eq!(slots.len(), sig.positional.len() + sig.keyword_only.len());
                for (name, value) in sig.positional.iter().chain(&sig.keyword_only).zip(slots) {
                    self.bind_argument(local, name, value)?;
                }
                smallvec::SmallVec::new()
            }
        };
        let supplied_types = self.check_supplied_types(callable, &sig, &name, |param| {
            self.scope_data(local).names.get(param).copied()
        });
        for slot in omitted {
            let value = self.bound_default(&shape, slot, defaults, kwdefaults);
            self.bind_argument(local, shape.parameter(slot), value)?;
        }
        supplied_types?;
        let Value::Scope(frame) = self.heap.get_mut(local) else {
            unreachable!()
        };
        if sig.class_cell {
            frame.class_cell = class_cell;
        }
        self.check_call_depth(0)?;
        self.call_depth += 1;
        let returning = sig.return_type.map(|declared| FunctionReturn {
            callable,
            declared,
            name,
        });
        drop(sig);
        let owner = FrameOwner::Function(callable);
        if let value::Body::Native(entry, _) = self.code_object(code).body {
            self.heap.enter(local, code, owner);
            return Ok(RegisteredCall::Native(RegisteredFrame {
                entry,
                scope: local,
                returning,
            }));
        }
        let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
            self.execute(code, local, owner)
        });
        self.finish_function(returning, result)
            .map(RegisteredCall::Returned)
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

    /// Check scalar annotations on values that were explicitly bound by this call.
    pub(super) fn check_supplied_types(
        &self,
        function: Id,
        sig: &CallParameters,
        name: &str,
        supplied: impl Fn(&str) -> Option<Id>,
    ) -> Result<()> {
        if let Some(types) = sig.types.as_object() {
            for (param, expected) in types {
                if let Some(id) = supplied(param) {
                    let builtin = expected.get("builtin").and_then(serde_json::Value::as_str);
                    let target = builtin.and_then(|name| {
                        let Value::Function { scope, .. } = self.heap.get(function) else {
                            return None;
                        };
                        let canonical = *self.builtins.get(name)?;
                        (self.module_type_reference(*scope, name) == Some(canonical))
                            .then_some(canonical)
                    });
                    self.require_scalar(
                        function,
                        param,
                        id,
                        (builtin.or_else(|| expected.as_str()), target),
                        sig.integer_sign_parameters[param] == true,
                        format_args!("{name} argument {param:?}"),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The outer None means that Python could intercept this dictionary lookup.
    pub(super) fn scalar_dict_name(&self, dict: Id, name: &str) -> Option<Option<Id>> {
        let Value::Dict(entries) = self.heap.get(dict) else {
            return None;
        };
        entries.lookup_text(&self.heap, name).continue_value()
    }

    fn scalar_scope_name(&self, scope: Id, name: &str) -> Option<Option<Id>> {
        let frame = self.scope_data(scope);
        match frame
            .exec_globals
            .map(|globals| globals.dict)
            .or_else(|| frame.binding_namespace(name))
        {
            Some(dict) => self.scalar_dict_name(dict, name),
            None => Some(self.scope_name(scope, name)),
        }
    }

    /// Read a name or a single member of an ordinary module without running Python.
    fn module_type_reference(&self, scope: Id, reference: &str) -> Option<Id> {
        if reference == "None" {
            return Some(NONE);
        }
        let (name, member) = reference
            .split_once('.')
            .map_or((reference, None), |(name, member)| (name, Some(member)));
        let frame = self.scope_data(scope);
        // A method may capture just its class dictionary ahead of its globals.
        let (scope, local) = if let Some(parent) = frame.parent {
            let cells = &frame.closure.cells;
            if cells.len() != 1 {
                return None;
            }
            let Value::Cell(Some(dict)) = *self.heap.get(*cells.get("__classdict__")?) else {
                return None;
            };
            (parent, self.scalar_dict_name(dict, name)?)
        } else {
            (scope, None)
        };
        let frame = self.scope_data(scope);
        if frame.parent.is_some() || frame.exec_locals.is_some() {
            return None;
        }
        let binding = if let Some(value) = local {
            value
        } else if let Some(value) = self.scalar_scope_name(scope, name)? {
            value
        } else if let Some(dict) = frame
            .exec_globals
            .map(|globals| globals.builtins)
            .or(self.source_builtins)
        {
            self.scalar_dict_name(dict, name)??
        } else {
            *self.builtins.get(name)?
        };
        let Some(member) = member else {
            return Some(binding);
        };
        let Value::Module {
            scope, constructor, ..
        } = *self.heap.get(binding)
        else {
            return None;
        };
        if constructor.is_some_and(|class| self.builtins.get("module") != Some(&class)) {
            return None;
        }
        self.scalar_scope_name(scope, member)?
    }

    /// Prove both the compiler and its default policy without running Python before get_data.
    pub(super) fn uses_builtin_compile(&self, converter: Id) -> bool {
        let Value::Function {
            scope, kwdefaults, ..
        } = *self.heap.get(converter)
        else {
            return false;
        };
        let Some(Some(level)) = self.scalar_dict_name(kwdefaults, "_optimize") else {
            return false;
        };
        if !matches!(self.heap.get(level), Value::Int(value) if value.to_i64() == Some(-1)) {
            return false;
        }
        self.builtins
            .get("compile")
            .is_some_and(|&compiler| self.module_type_reference(scope, "compile") == Some(compiler))
    }

    /// A generated annotation can close over a local type name. For an integer
    /// argument, prove that it names the builtin float without evaluating Python.
    fn captured_float_reference(&self, scope: Id, name: &str) -> Option<Id> {
        if name.contains('.') {
            return None;
        }
        let cells = &self.scope_data(scope).closure.cells;
        let binding = |name: &str| match self.heap.get(*cells.get(name)?) {
            Value::Cell(value) => *value,
            _ => None,
        };
        let class_binding = if cells.contains_key("__classdict__") {
            self.scalar_dict_name(binding("__classdict__")?, name)?
        } else {
            None
        };
        let value = class_binding.or_else(|| binding(name))?;
        (self.builtins.get("float") == Some(&value)).then_some(value)
    }

    /// Use published annotations first. An unread generated provider can prove a
    /// module binding without executing it, including when transferred to another function.
    fn checked_annotation(
        &self,
        function: Id,
        slot: &str,
        integer_argument: bool,
    ) -> Result<ScalarAnnotation> {
        let Value::Function {
            fields,
            scope: owner,
            ..
        } = *self.heap.get(function)
        else {
            unreachable!()
        };
        let published = if let Some(dict) = fields.annotations {
            let value = match self.scalar_dict_name(dict, slot) {
                Some(Some(value)) => value,
                Some(None) => return Ok(ScalarAnnotation::Missing),
                None => return Ok(ScalarAnnotation::Unresolved),
            };
            match self.heap.get(value) {
                Value::Str(text) => Some(text.as_str()),
                _ => return Ok(ScalarAnnotation::Value(value)),
            }
        } else if fields.annotate == NONE {
            // Reading __annotations__ here would create an empty dictionary.
            return Ok(ScalarAnnotation::Missing);
        } else {
            None
        };
        let Value::Function { scope, code, .. } = *self.heap.get(fields.annotate) else {
            return Ok(ScalarAnnotation::Unresolved);
        };
        let provider = self.code_object(code);
        if !provider.definition.closure_supported || provider.parameters == NONE {
            return Ok(ScalarAnnotation::Unresolved);
        }
        let sig = self.parsed_parameters(code)?;
        let Some(name) = sig
            .annotation_references
            .as_ref()
            .and_then(|slots| slots[slot].as_str())
        else {
            return Ok(ScalarAnnotation::Unresolved);
        };
        let postponed = provider.definition.flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0;
        if published.is_some_and(|text| !postponed || text != name) {
            return Ok(ScalarAnnotation::Unresolved);
        }
        // A postponed string belongs to the receiving function's type-hint namespace.
        let scope = if postponed {
            if !matches!(self.heap.get(fields.type_params), Value::Tuple(params) if params.is_empty())
            {
                return Ok(ScalarAnnotation::Unresolved);
            }
            self.root_scope(owner)
        } else {
            scope
        };
        let mut target = self.module_type_reference(scope, name);
        if target.is_none() && integer_argument && !postponed {
            target = self.captured_float_reference(scope, name);
        }
        Ok(target.map_or(ScalarAnnotation::Unresolved, ScalarAnnotation::Value))
    }

    fn require_scalar(
        &self,
        function: Id,
        slot: &str,
        value: Id,
        declaration: (Option<&str>, Option<Id>),
        integer_sign: bool,
        context: std::fmt::Arguments<'_>,
    ) -> Result<()> {
        let scalar = |id| match self.heap.get(id) {
            Value::Native(name)
                if self.builtins.get(name) == Some(&id)
                    && matches!(name.as_str(), "str" | "bytes" | "int" | "bool" | "float") =>
            {
                Some(name.as_str())
            }
            _ => None,
        };
        let boxed = self.heap.get(value);
        let (expected, checked_builtin) = declaration;
        let integer_argument = slot != "return" && matches!(boxed, Value::Int(_));
        let annotation = if let Some(target) = checked_builtin {
            ScalarAnnotation::Value(target)
        } else {
            self.checked_annotation(function, slot, integer_argument)?
        };
        let target = match annotation {
            ScalarAnnotation::Value(target) => Some(target),
            ScalarAnnotation::Missing => return Ok(()),
            ScalarAnnotation::Unresolved => None,
        };
        let bound = target.and_then(|target| {
            let family = match self.heap.get(target) {
                Value::Class { ancestors, .. } => ancestors.iter().find_map(|&base| scalar(base)),
                Value::None => Some("None"),
                _ => scalar(target),
            }?;
            Some((target, family))
        });
        let expected = if let Some((target, family)) = bound {
            if let Value::Instance { class, .. } = boxed
                && (*class == target
                    || self
                        .published_mro(*class)
                        .is_some_and(|mro| mro.contains(&target)))
            {
                return Ok(());
            }
            if matches!(self.heap.get(target), Value::Class { .. }) {
                return Err(Error::new(
                    "TypeError",
                    format!("{context} requires {family}"),
                ));
            }
            family
        } else if let Some(fallback) = expected.and_then(|name| ScalarHint::new(name).0) {
            fallback
        } else {
            return Ok(());
        };
        // Resolved numeric arguments keep their original objects in the frame.
        let resolved_argument = bound.is_some() && slot != "return";
        let accepted = match (expected, boxed) {
            ("int", Value::Bool(_)) => resolved_argument,
            ("float", Value::Int(_)) => resolved_argument || integer_sign,
            ("None", Value::None)
            | ("int", Value::Int(_))
            | ("float", Value::Float(_))
            | ("bytes", Value::Bytes(_))
            | ("str", Value::Str(_) | Value::SurrogateStr(_))
            | ("bool", Value::Bool(_)) => true,
            _ => false,
        };
        if accepted {
            Ok(())
        } else {
            Err(Error::new(
                "TypeError",
                format!("{context} requires {expected}"),
            ))
        }
    }

    /// Initialize descriptor names in class body insertion order, before subclass hooks run.
    pub(super) fn initialize_class_members(
        &mut self,
        class: Id,
        scope: Id,
        namedtuple_namespace: Option<Id>,
    ) -> Result<()> {
        let Value::Scope(class_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        debug_assert!(class_scope.in_class_body);
        class_scope.in_class_body = false;
        let dictionary = class_scope
            .class_namespace
            .as_ref()
            .and_then(|ns| ns.dictionary);
        if let Some(dictionary) = dictionary
            && class_scope
                .class_namespace
                .as_ref()
                .unwrap()
                .annotation_dict_cell
                .is_some()
        {
            class_scope.namespace = Some(dictionary);
            class_scope.names.retain(|name, _| name == "__qualname__");
        }
        if let Some(namespace) = namedtuple_namespace {
            self.call_typing_helper("_namedtuple_set_names", vec![class, namespace])
                .map(|_| ())
        } else if let Some(dict) = dictionary {
            let members = self.dict_entries(dict).expect("class dictionary").to_vec();
            for &(key, _) in &members {
                self.heap.keep(key);
            }
            self.set_class_member_names(class, members, |_, key| key)
        } else {
            let members = class_scope.class_namespace_entries(false);
            self.set_class_member_names(class, members, Self::text)
        }
    }

    /// Use the snapshotted keys and values even when an earlier hook mutates the class.
    fn set_class_member_names<K>(
        &mut self,
        class: Id,
        members: Vec<(K, Id)>,
        key_object: impl Fn(&mut Self, K) -> Id,
    ) -> Result<()> {
        for &(_, value) in &members {
            self.heap.keep(value);
        }
        for (key, value) in members {
            if matches!(self.heap.get(value), Value::Property(_)) {
                let name = key_object(self, key);
                self.replace_property_name(value, Some(name));
            } else if let Some(set_name) = self.bound_slot(value, "__set_name__")? {
                let name = key_object(self, key);
                if let Err(error) = self.call(set_name, vec![class, name], Vec::new()) {
                    return Err(self.descriptor_name_error(error, class, name, value));
                }
            }
        }
        Ok(())
    }

    /// Attach type.__new__'s descriptor context while preserving the raised exception's identity.
    fn descriptor_name_error(&mut self, error: Error, class: Id, key: Id, value: Id) -> Error {
        let exception = self.raised_instance(error);
        self.heap.keep(exception);
        let result = self.checked_format(key, true).and_then(|key| {
            let descriptor = self.type_name(value);
            let descriptor = &descriptor[..descriptor.floor_char_boundary(100)];
            let owner = self.class_name(class);
            let owner = &owner[..owner.floor_char_boundary(100)];
            let note = self.text(format!(
                "Error calling __set_name__ on '{descriptor}' instance {key} in '{owner}'"
            ));
            self.add_exception_note(exception, note)
        });
        if let Err(mut error) = result {
            if error.trace.as_ref().and_then(|trace| trace.instance) != Some(exception) {
                error.context = exception;
            }
            return error;
        }
        let Value::Raised(error) = self.heap.get(exception) else {
            unreachable!()
        };
        let mut error = error.clone();
        error.tracing().instance = Some(exception);
        error
    }

    /// Bind ABCMeta.__new__'s positional-only parameters for direct and cooperating calls.
    pub(super) fn abc_new(&mut self, args: &[Id], keywords: &[(String, Id)]) -> Result<Id> {
        let [meta, name, bases, namespace] = args else {
            const PARAMETERS: [&str; 4] = ["mcls", "name", "bases", "namespace"];
            let name = "ABCMeta.__new__";
            return Err(if args.len() < PARAMETERS.len() {
                missing_arguments_error(name, "positional", &PARAMETERS[args.len()..])
            } else {
                excess_positional_error(name, 4, 0, args.len(), 0)
            });
        };
        self.abc_new_from_namespace(*meta, *name, *bases, *namespace, keywords.to_vec())
    }

    /// Call the winning metaclass constructor with the actual prepared namespace.
    pub(super) fn call_metaclass_new(
        &mut self,
        meta: Id,
        name: Id,
        bases: Id,
        namespace: Id,
        keywords: Vec<(String, Id)>,
    ) -> Result<Id> {
        let new = self.lookup_attr(meta, "__new__")?;
        self.call(new, vec![meta, name, bases, namespace], keywords)
    }

    /// Select the most derived compatible metaclass before executing a boxed class body.
    pub(super) fn select_class_metaclass(
        &mut self,
        bases: &[Id],
        explicit: Option<Id>,
        typed_dict_meta: Option<Id>,
        enum_base: bool,
    ) -> Result<Option<Id>> {
        // NamedTuple's marker is replaced with tuple before allocating the class.
        let namedtuple = self.namedtuple_base(bases);
        let metaclass_bases = bases
            .iter()
            .copied()
            .filter(|base| Some(*base) != namedtuple);
        if explicit.is_none()
            && !metaclass_bases.clone().any(|base| {
                !self.is_type_object(base)
                    || self.is_bundled_abc(base)
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
        for base in metaclass_bases {
            let meta =
                if self.heap.get(base).instance_class().is_none() && self.is_bundled_abc(base) {
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
        if enum_base || namedtuple.is_some() {
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
        if !is_abc
            && Some(winner) != typed_dict_meta
            && (self
                .class_attr(winner, "mro")
                .is_some_and(|method| Some(&method) != self.builtins.get("type.mro"))
                || ["__getattr__", "__getattribute__"]
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

    /// Run __prepare__ before executing the class body, retaining the returned dictionary.
    fn prepare_class_dictionary(
        &mut self,
        meta: Id,
        name: Id,
        bases: Id,
        keywords: &[(String, Id)],
    ) -> Result<Id> {
        let hook = match self.lookup_attr(meta, "__prepare__") {
            Ok(hook) => hook,
            Err(error) if self.is_attribute_error(&error) => return Ok(self.dict(Vec::new())),
            Err(error) => return Err(error),
        };
        let namespace = self.call(hook, vec![name, bases], keywords.to_vec())?;
        if matches!(self.heap.get(namespace), Value::Dict(_)) {
            return Ok(namespace);
        }
        if self.has_mapping_subscription(namespace)
            || matches!(
                self.heap.get(namespace),
                Value::GenericAlias(_) | Value::Union(_)
            )
            || matches!(self.heap.get(namespace), Value::Weak(reference)
                if reference.kind != weakref::Kind::Reference)
            || self.enum_values(namespace).is_some()
        {
            return Err(Error::new(
                "NotImplementedError",
                "class bodies with a prepared mapping other than an exact dict are unsupported",
            ));
        }
        let owner = if self.is_type_object(meta) {
            self.class_name(meta)
        } else {
            "<metaclass>"
        };
        Err(Error::new(
            "TypeError",
            format!(
                "{owner}.__prepare__() must return a mapping, not {}",
                self.type_name(namespace)
            ),
        ))
    }

    /// A prepared dictionary may contain keys whose equality raises during an implicit STORE_NAME.
    pub(super) fn store_class_metadata(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        let frame = self.scope_data(scope);
        if frame.class_body_mapping().is_some() && frame.enum_namespace.is_none() {
            self.bind_source_name(scope, name, value)
        } else {
            self.bind(scope, name, value);
            Ok(())
        }
    }

    pub(super) fn class(
        &mut self,
        bases: Id,
        body: Id,
        mut keywords: Vec<(String, Id)>,
        parameterized: bool,
    ) -> Result<Id> {
        let (original, mut bases) = super::typing::resolve_bases(self, bases)?;
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
            self.enum_namespace(&bases)?
        };
        let explicit_metaclass = keywords
            .iter()
            .position(|(name, _)| name == "metaclass")
            .map(|index| keywords.remove(index).1);
        if enum_namespace.is_some() && keywords.iter().any(|(name, _)| name == "_simple") {
            return Err(Error::new(
                "NotImplementedError",
                "boxed enum metaclass options are unsupported",
            ));
        }
        let factory = explicit_metaclass.filter(|&meta| {
            !self.is_type_object(meta)
                && typed_dict_meta.is_none()
                && enum_namespace.is_none()
                && !bases.iter().any(|&base| self.is_bundled_abc(base))
        });
        let class_meta = if factory.is_some() {
            factory
        } else {
            self.select_class_metaclass(
                &bases,
                explicit_metaclass,
                typed_dict_meta,
                enum_namespace.is_some(),
            )?
        };
        let Value::Function {
            scope: parent,
            code,
            ..
        } = self.heap.get(body).clone()
        else {
            return Err(Error::new("TypeError", "invalid class body"));
        };
        let class_name = self.code_object(code).name;
        let name = self.str_value(class_name)?.to_owned();
        let prepares = class_meta.filter(|&meta| {
            factory.is_some()
                || (Some(meta) != typed_dict_meta
                    && self.builtins.get("abc.ABCMeta") != Some(&meta)
                    && self
                        .class_attr(meta, "__prepare__")
                        .is_some_and(|hook| Some(&hook) != self.builtins.get("type.__prepare__")))
        });
        let prepared = if let Some(meta) = prepares {
            let bases = self.alloc(Value::Tuple(copy_ids(&bases)?));
            let mapping = self.prepare_class_dictionary(meta, class_name, bases, &keywords)?;
            Some((bases, mapping))
        } else {
            None
        };
        #[derive(serde::Deserialize)]
        struct ClassBindings {
            #[serde(default)]
            class_globals: std::collections::HashSet<String>,
            #[serde(default)]
            scope_nonlocals: std::collections::HashSet<String>,
            #[serde(default)]
            needs_class_cell: bool,
            #[serde(default)]
            needs_dict_cell: bool,
            #[serde(default)]
            postponed_annotations: bool,
            #[serde(default)]
            conditional_annotations: bool,
            static_attributes: Option<Vec<String>>,
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
        body_scope.in_class_body = true;
        body_scope.closure.nonlocals = bindings.scope_nonlocals;
        body_scope.enum_namespace = enum_namespace;
        body_scope.class_cell = Some(cell);
        if let Some((_, mapping)) = prepared {
            body_scope.globals = bindings.class_globals;
            body_scope.namespace = Some(mapping);
            body_scope
                .class_namespace
                .get_or_insert_with(Default::default)
                .dictionary = Some(mapping);
        }
        let globals = self.root_scope(parent);
        let module_name = if self.binding_scope(scope, "__name__") != scope {
            self.lookup(scope, "__name__")?
        } else if let Some(name) = self.lookup_scope_name(scope, "__name__")? {
            name
        } else if let Some(exec_globals) = self.scope_data(globals).exec_globals {
            self.source_global(exec_globals, "__name__")?
        } else {
            self.scope_name(globals, "__name__")
                .unwrap_or_else(|| self.text("builtins"))
        };
        self.store_class_metadata(scope, "__module__", module_name)?;
        if let Ok(qualname) = self.lookup_attr(body, "__qualname__") {
            self.store_class_metadata(scope, "__qualname__", qualname)?;
        }
        if bindings.static_attributes.is_some() {
            let first = self.int(i64::from(self.code_object(code).first_line));
            self.store_class_metadata(scope, "__firstlineno__", first)?;
        }
        if parameterized {
            let params = self
                .scope_name(parent, ".type_params")
                .expect("class parameter scope");
            self.store_class_metadata(scope, "__type_params__", params)?;
        }
        let annotations = if bindings.postponed_annotations || typed_dict_meta.is_some() {
            if let Some(fields) = self.lookup_scope_name(scope, "__annotations__")? {
                fields
            } else {
                let fields = self.dict(Vec::new());
                if let Some((_, mapping)) = prepared {
                    let key = self.heap.code_name("__annotations__");
                    self.put_item(mapping, key, fields)?;
                } else {
                    self.bind_local(scope, "__annotations__", fields);
                }
                fields
            }
        } else {
            NONE
        };
        let conditional_cell = if bindings.conditional_annotations {
            let active = self.op("set", NONE, NONE, NONE)?;
            Some(self.alloc(Value::Cell(Some(active))))
        } else {
            None
        };
        if let Value::Scope(frame) = self.heap.get_mut(scope) {
            frame
                .class_namespace
                .get_or_insert_with(Default::default)
                .annotation_scratch = Some((annotations, conditional_cell));
        }
        self.execute(code, scope, FrameOwner::Function(body))?;
        if (bindings.needs_class_cell || self.scope_name(scope, "__classcell__").is_some())
            && self.namedtuple_base(&bases).is_some()
        {
            return Err(Error::new(
                "TypeError",
                "uses of super() and __class__ are unsupported in methods of NamedTuple subclasses",
            ));
        }
        if let Some(names) = bindings.static_attributes {
            let names = names.iter().map(|name| self.heap.code_name(name)).collect();
            let attrs = self.alloc(Value::Tuple(names));
            self.store_class_metadata(scope, "__static_attributes__", attrs)?;
        }
        if let Some(meta) = class_meta
            && Some(meta) != typed_dict_meta
            && self.builtins.get("abc.ABCMeta") != Some(&meta)
            && (prepared.is_some()
                || self.slot(meta, "__call__").is_some()
                || self.class_attr(meta, "__new__").is_some()
                || self.class_attr(meta, "__init__").is_some())
        {
            let namespace = self.active_class_namespace(scope);
            if bindings.needs_dict_cell {
                let dict_cell = self.class_annotation_dict_cell(scope);
                self.store_class_metadata(scope, "__classdictcell__", dict_cell)?;
            }
            if bindings.needs_class_cell {
                self.store_class_metadata(scope, "__classcell__", cell)?;
            }
            if let Some(original) = original {
                let key = self.text("__orig_bases__");
                self.put_item(namespace, key, original)?;
            }
            let bases = prepared
                .map(|(tuple, _)| tuple)
                .unwrap_or_else(|| self.alloc(Value::Tuple(bases)));
            let class = self.call(meta, vec![class_name, bases, namespace], keywords)?;
            if bindings.needs_class_cell && self.is_type_object(class) {
                match self.heap.get(cell) {
                    Value::Cell(Some(contents)) if *contents == class => {}
                    Value::Cell(None) => {
                        return Err(Error::new(
                            "RuntimeError",
                            "__class__ not set: was __classcell__ propagated to type.__new__()?",
                        ));
                    }
                    _ => {
                        return Err(Error::new(
                            "TypeError",
                            "__class__ set to a different class during class creation",
                        ));
                    }
                }
            }
            return Ok(class);
        }
        if enum_kind.is_some() {
            let boundary = keywords
                .iter()
                .position(|(name, _)| name == "boundary")
                .map_or(NONE, |index| keywords.remove(index).1);
            self.bind_enum_boundary(scope, &bases, boundary)?;
        }
        let exposed_body = self.finish_class_namespace(scope)?;
        // NamedTupleMeta visits the original values, including defaults replaced by tuple getters.
        let namedtuple_namespace = if self.namedtuple_base(&bases).is_some() {
            let namespace = if let Some(body) = &exposed_body {
                body.mapping
            } else {
                let bindings = self.scope_data(scope).names.clone();
                let entries = bindings
                    .into_iter()
                    .map(|(name, value)| (self.text(name), value))
                    .collect();
                self.dict(entries)
            };
            self.heap.keep(namespace);
            Some(namespace)
        } else {
            None
        };
        if let Value::Scope(frame) = self.heap.get_mut(scope) {
            if let Some(namespace) = &mut frame.class_namespace {
                namespace.annotation_scratch = None;
            }
            // The type keeps the namespace after the body returns. Its readers own their cells.
            frame.parent = Some(globals);
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
            Some((parents, fields, annotate, total))
        } else {
            None
        };
        if let Some(original) = original {
            self.bind(scope, "__orig_bases__", original);
        }
        if typed_dict_meta.is_none() {
            self.enable_class_annotation_provider(scope);
        }
        if bases.iter().any(|&base| self.is_abc_metaclass(base))
            && let Some(new) = self.namespace_binding(self.scope_data(scope), "__new__")
            && matches!(self.heap.get(new), Value::Function { .. })
        {
            let new = super::method_decorator::static_method(self, new)?;
            self.bind_local(scope, "__new__", new);
        }
        let float_enum = enum_kind
            .filter(|kind| kind.scalar_base().is_none())
            .and_then(|_| self.float_enum_type(&bases));
        let object = self.builtin("object");
        // Socket subtypes store their native endpoint separately from Python member slots.
        let ordinary_slots = typed_dict_meta.is_none()
            && enum_kind.is_none()
            && bases.iter().all(|&base| {
                let storage = self.allocation_base(base).unwrap_or(base);
                storage == object
                    || self.heap.get(storage).heap_type() == Some(stdlib::socket::CLASS)
            });
        let layout = if ordinary_slots {
            super::typing::ClassLayout::OrdinarySlots
        } else {
            super::typing::ClassLayout::Specialized
        };
        let (class, layout_base) = self.allocate_class_with_slots(
            name.clone(),
            scope,
            bases,
            layout,
            namedtuple_namespace,
        )?;
        let Value::Class { metaclass, .. } = self.heap.get_mut(class) else {
            unreachable!()
        };
        *metaclass = class_meta;
        self.install_instance_dictionary_descriptor(class, scope, layout_base);
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
            class_scope.names.entry("__doc__".into()).or_insert(NONE);
        }
        if let Some(body) = exposed_body {
            // type.__new__ replaces the body namespace with the finished type's dictionary.
            let dict = self.class_namespace_dict(scope, false);
            let extras = self
                .dict_entries(body.mapping)
                .expect("class body dictionary")
                .iter()
                .copied()
                .filter(|&(key, _)| self.string_storage(key).is_none())
                .collect::<Vec<_>>();
            // NamedTuple's annotation provider closes over the declaration namespace,
            // which remains distinct from the generated type's dictionary.
            if namedtuple_namespace.is_none()
                && let Some(cell) = body.annotation_cell
            {
                self.heap.replace_cell(cell, Some(dict));
            }
            if namedtuple_namespace.is_none() && !extras.is_empty() {
                for (key, value) in extras {
                    self.put_item(dict, key, value)?;
                }
                self.emit_builtin_warning(
                    crate::objects::exceptions::Kind::RuntimeWarning,
                    &format!("non-string key in the __dict__ of class {name:.200}"),
                    1,
                )?;
            }
        }
        self.initialize_class_members(class, scope, namedtuple_namespace)?;
        if let Some(kind) = enum_kind {
            let scalar = if let Some((member_type, first_enum)) = float_enum {
                self.bind(scope, "_member_type_", member_type);
                self.install_float_enum_display(class, member_type, first_enum)?;
                Some(member_type)
            } else {
                kind.scalar_base().map(|base| self.builtin(base))
            };
            self.populate_enum(class, scalar)?;
        }
        // Look up the inherited hook after __set_name__, before publishing the class.
        let hook = self.super_attribute(class, class, class, "__init_subclass__")?;
        self.call(hook, Vec::new(), keywords)?;
        if module == "_collections_abc"
            && matches!(
                name.as_str(),
                "Hashable"
                    | "Awaitable"
                    | "AsyncIterable"
                    | "Buffer"
                    | "Iterable"
                    | "Sized"
                    | "Container"
                    | "Collection"
                    | "Mapping"
                    | "MutableMapping"
            )
        {
            self.classes
                .insert(format!("collections.abc.{name}"), class);
        }
        self.init_abstract_class(class, scope)?;
        if self.is_protocol_declaration(class) {
            let members = self.call_typing_helper("_get_protocol_attrs", vec![class])?;
            self.set_attr(class, "__protocol_attrs__", members)?;
        }
        if let Some((parents, fields, annotate, total)) = typed_dict_fields {
            let configure = self
                .module_value("typing", "_configure_typeddict")
                .ok_or_else(|| Error::new("RuntimeError", "missing TypedDict initializer"))?;
            self.call(
                configure,
                vec![class, fields, total, annotate, parents],
                Vec::new(),
            )?;
        }
        if module == "typing" && parent == globals && name == "_Final" {
            self.classes.insert("typing._Final".into(), class);
        }
        Ok(class)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_exception_namespaces_are_unique_and_reused() {
        let mut ctx = super::super::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let first = ctx.builtin("FileNotFoundError");
        let second = ctx.builtin("PermissionError");
        assert!(!ctx.native_type_dicts.contains_key(&first));
        assert!(!ctx.native_type_dicts.contains_key(&second));

        let proxy = ctx.default_get_attr(first, "__dict__").unwrap();
        let dictionary = ctx.native_type_dicts[&first];
        assert!(matches!(ctx.heap.get(proxy), Value::MappingProxy(id) if *id == dictionary));
        let next = ctx.default_get_attr(first, "__dict__").unwrap();
        assert!(matches!(ctx.heap.get(next), Value::MappingProxy(id) if *id == dictionary));

        ctx.default_get_attr(second, "__dict__").unwrap();
        assert_ne!(ctx.native_type_dicts[&second], dictionary);
    }
}
