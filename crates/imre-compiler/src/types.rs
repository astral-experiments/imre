//! Interns concrete container types while keeping IR type identities small and exact.

use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Write};

use crate::ir::Type;
use crate::modules::DefinitionId;
use std::rc::Rc;

const MAX_DESCRIPTION_BYTES: usize = 1024;

const STAT_RESULT_NAME: &str = "os.stat_result";
pub(crate) const STAT_SEQUENCE_FIELDS: [&str; 10] = [
    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid", "st_size", "st_atime",
    "st_mtime", "st_ctime",
];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Shape {
    StringLiterals(Vec<String>),
    Cursor(Type),
    List(Type),
    Dict(Type, Type),
    OrderedDict(Type, Type),
    DefaultListDict(Type, Type),
    Counter(Type),
    Set(Type),
    Deque(Type),
    Tuple(Vec<Type>),
    ModuleSpec(Vec<Type>),
    NamedTuple(TupleOrigin, Vec<Type>),
    PartialFrame(Vec<Type>),
    LenFrame(Vec<Type>),
    /// The class name, owned positional tuple and its formatting capability travel together.
    CallbackArgument(Type),
    /// Its concrete record descriptor owns the argument and describes its real type.
    CallbackPositional,
    VarTuple(Type),
    /// Display segments are fixed fields or repetitions of an item of known type.
    DisplayTuple(Vec<(Type, bool)>),
    DeferredCmpFrame((usize, u32), Vec<Type>),
    Callable(Vec<Type>, Type),
    IndexCallable(Type),
    RecordConstructor(usize, Vec<Type>),
    DeclaredCache(Type, usize),
    CachedCallable(CachedFunction),
    DispatchCallable(DispatchFunction),
    DispatchRegistry(Type),
    Awaitable(Type),
    ContextValue(Type),
    DirectoryContext(Type),
    ProcessArgs(Type),
    Optional(Type),
    Union(Vec<Type>),
}

/// The callable ABI stays unchanged; the frame owns both the wrappee and its cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct CachedFunction {
    pub callable: Type,
    pub frame: Type,
    pub table: Type,
    pub capacity: i64,
    pub annotations: Option<AnnotationOrigin>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct CachePolicy {
    /// Negative represents an unbounded cache; negative Python sizes normalize to zero.
    pub capacity: i64,
    pub typed: bool,
}

/// A dispatcher owns its original function and a private mutable registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct DispatchFunction {
    pub callable: Type,
    pub frame: Type,
    pub registry: Type,
    pub metadata: Type,
}

/// A cache delegates annotation evaluation to the captured declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum AnnotationOrigin {
    Module(crate::modules::DefinitionId),
    Local(usize),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TupleOrigin {
    Declaration(DefinitionId),
    StructSequence(&'static str, &'static [&'static str]),
    LibraryNamedTuple(&'static str, &'static [&'static str]),
}

pub(crate) const CACHE_INFO: TupleOrigin =
    TupleOrigin::LibraryNamedTuple("CacheInfo", &["hits", "misses", "maxsize", "currsize"]);

#[derive(Clone)]
pub(crate) struct NamedException {
    pub id: u32,
    pub name: String,
    pub base: u32,
    pub ancestors: Vec<u32>,
}

pub(crate) struct NamedTuple {
    pub(crate) name: String,
    pub(crate) fields: Vec<String>,
}

pub(crate) struct FunctionSite {
    pub module: String,
    pub qualname: String,
    pub line: u32,
}
#[derive(Default)]
struct Storage {
    shapes: Vec<Shape>,
    has_module_spec: bool,
    named_tuples: HashMap<DefinitionId, Rc<NamedTuple>>,
    cache_adapters: HashMap<usize, CachePolicy>,
    source_sites: HashMap<(usize, u32), Rc<FunctionSite>>,
    function_sites: HashMap<usize, Rc<FunctionSite>>,
    plain_callbacks: HashSet<Type>,
    constructor_targets: HashMap<usize, usize>,
    constructor_slots: BTreeMap<usize, usize>,
    exceptions: Vec<NamedException>,
    ids: HashMap<Shape, usize>,
    hashable_records: HashSet<usize>,
    value_structs: HashSet<usize>,
    custom_truth: HashSet<usize>,
    pathlike_records: HashSet<usize>,
    fallible_keys: HashSet<usize>,
    concrete_subclasses: HashMap<usize, Vec<Type>>,
}

/// An annotation and a matching inferred literal always receive the same identity.
/// Interior mutability lets independent function checkers share this compilation's
/// type universe without retaining references into its growable storage.
#[derive(Default)]
pub(crate) struct Types(RefCell<Storage>);

#[derive(Default)]
pub(crate) struct ShapePrefix(Vec<Shape>);

impl Types {
    pub(crate) fn shape_count(&self) -> usize {
        self.0.borrow().shapes.len()
    }

    pub(crate) fn save_prefix(&self, previous: &mut ShapePrefix) {
        let storage = self.0.borrow();
        if previous.0 != storage.shapes {
            previous.0.clone_from(&storage.shapes);
        }
    }

    pub(crate) fn matches_prefix(&self, previous: &ShapePrefix) -> bool {
        previous.0.starts_with(&self.0.borrow().shapes)
    }
}

#[cfg(any(debug_assertions, test, feature = "verify-ir"))]
impl Types {
    /// Checks an identity without using the unchecked shape accessors below.
    pub(crate) fn verify_type(&self, ty: Type, classes: usize) -> Result<(), String> {
        let storage = self.0.borrow();
        let valid = match ty {
            Type::StrLiteral(id) => {
                matches!(storage.shapes.get(id), Some(Shape::StringLiterals(_)))
            }
            Type::ModuleSpec(id) => {
                matches!(storage.shapes.get(id), Some(Shape::ModuleSpec(fields)) if fields.len() == 8)
            }
            Type::SimpleNamespace(id) => {
                matches!(storage.shapes.get(id), Some(Shape::Dict(Type::Str, _)))
            }
            Type::Cursor(id) => matches!(storage.shapes.get(id), Some(Shape::Cursor(_))),
            Type::List(id) => matches!(storage.shapes.get(id), Some(Shape::List(_))),
            Type::CacheState(id) => matches!(storage.shapes.get(id), Some(Shape::Dict(..))),
            Type::Dict(id) => matches!(
                storage.shapes.get(id),
                Some(
                    Shape::Dict(..)
                        | Shape::OrderedDict(..)
                        | Shape::DefaultListDict(..)
                        | Shape::Counter(_)
                        | Shape::DispatchRegistry(_)
                )
            ),
            Type::Set(id) => matches!(storage.shapes.get(id), Some(Shape::Set(_))),
            Type::ChainMap(id) => matches!(storage.shapes.get(id), Some(Shape::Dict(..))),
            Type::Deque(id) => matches!(storage.shapes.get(id), Some(Shape::Deque(_))),
            Type::Tuple(id) => matches!(
                storage.shapes.get(id),
                Some(
                    Shape::Tuple(_)
                        | Shape::NamedTuple(..)
                        | Shape::PartialFrame(_)
                        | Shape::LenFrame(_)
                        | Shape::CallbackArgument(_)
                        | Shape::CallbackPositional
                        | Shape::DeferredCmpFrame(..)
                )
            ),
            Type::CmpKey(id, bound) => {
                matches!(storage.shapes.get(id), Some(Shape::PartialFrame(fields) | Shape::DeferredCmpFrame(_, fields)) if fields.len() == if bound { 2 } else { 1 })
            }
            Type::ForwardRef(id) => {
                matches!(storage.shapes.get(id), Some(Shape::Tuple(fields)) if fields.len() == 5)
            }
            Type::AnnotationValue(id) => {
                matches!(storage.shapes.get(id), Some(Shape::Tuple(fields))
                if fields.iter().all(|t| matches!(t, Type::BuiltinClass | Type::Str | Type::None | Type::ForwardRef(_))))
            }
            Type::VarTuple(id) => matches!(
                storage.shapes.get(id),
                Some(Shape::VarTuple(_) | Shape::DisplayTuple(_))
            ),
            Type::Callable(id) => matches!(
                storage.shapes.get(id),
                Some(
                    Shape::Callable(..)
                        | Shape::RecordConstructor(..)
                        | Shape::IndexCallable(_)
                        | Shape::DeclaredCache(..)
                        | Shape::CachedCallable(_)
                        | Shape::DispatchCallable(_)
                )
            ),
            Type::Optional(id) => matches!(storage.shapes.get(id), Some(Shape::Optional(_))),
            Type::Union(id) => matches!(storage.shapes.get(id), Some(Shape::Union(_))),
            Type::Class(id) | Type::NamedException(id) | Type::StrEnum(id) | Type::Enum(id) => {
                id < classes
            }
            Type::Coroutine(id) | Type::Task(id) => {
                matches!(storage.shapes.get(id), Some(Shape::Awaitable(_)))
            }
            Type::DirectoryContext(id) => {
                matches!(storage.shapes.get(id), Some(Shape::DirectoryContext(_)))
            }
            Type::ContextVar(id) | Type::ContextToken(id) => {
                matches!(storage.shapes.get(id), Some(Shape::ContextValue(_)))
            }
            Type::AsyncProcess(bits) => bits <= 3,
            Type::Popen(_, _, id) | Type::CompletedProcess(_, Some(id)) => {
                matches!(storage.shapes.get(id), Some(Shape::ProcessArgs(_)))
            }
            Type::Int
            | Type::Float
            | Type::BuiltinClass
            | Type::Bool
            | Type::Str
            | Type::Bytes
            | Type::ListInt
            | Type::StringIterable
            | Type::FrozenStrings
            | Type::StringKeys
            | Type::Iterator
            | Type::ByteOutput
            | Type::TupleBuffer
            | Type::Exception
            | Type::ObjectValue
            | Type::ReadOnlyObjectMapping
            | Type::ImportException
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::StringIO
            | Type::HandlerLock
            | Type::SourceModule
            | Type::BinaryIO
            | Type::Path
            | Type::PurePath(_)
            | Type::SysImplementation
            | Type::CompletedProcess(_, None)
            | Type::ActivePopen
            | Type::EventLoop
            | Type::TaskGroup
            | Type::GeneratorContext
            | Type::Context
            | Type::ContextMissing
            | Type::Semaphore
            | Type::Document(_)
            | Type::NativeModule
            | Type::ModuleNamespace
            | Type::LogFormatter
            | Type::LogRecord
            | Type::Logger
            | Type::TimedFileHandler
            | Type::ProcessSelectorClass
            | Type::ClockTime
            | Type::Calendar(_)
            | Type::StrDescriptor(_)
            | Type::Placeholder(_)
            | Type::FunctionRef
            | Type::OverloadStub
            | Type::PlainObject
            | Type::PathInfo
            | Type::CodecInfo
            | Type::Poll
            | Type::Socket
            | Type::CodecWriterClass
            | Type::CodecWriter
            | Type::Sha256
            | Type::JsonDecoder
            | Type::ZipFile
            | Type::ZipInfo
            | Type::CsvReader
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::JsonRoot
            | Type::JsonValue
            | Type::JsonObject
            | Type::Patch(_)
            | Type::None => true,
        };
        if valid {
            Ok(())
        } else {
            Err(format!("invalid type identity {ty}"))
        }
    }

    /// Interned structural types refer to earlier shapes; nominal class edges are separate.
    pub(crate) fn verify_shapes(&self, classes: usize) -> Result<(), String> {
        let storage = self.0.borrow();
        for (id, shape) in storage.shapes.iter().enumerate() {
            let children: Vec<Type> = match shape {
                Shape::StringLiterals(_) => Vec::new(),
                Shape::LenFrame(prefix) => prefix.clone(),
                Shape::CallbackArgument(positional) => vec![Type::Str, *positional, Type::Bool],
                Shape::CallbackPositional => Vec::new(),
                Shape::DisplayTuple(parts) => parts.iter().map(|&(ty, _)| ty).collect(),
                Shape::VarTuple(ty)
                | Shape::Cursor(ty)
                | Shape::List(ty)
                | Shape::Deque(ty)
                | Shape::Set(ty)
                | Shape::Counter(ty)
                | Shape::DispatchRegistry(ty)
                | Shape::Awaitable(ty)
                | Shape::ContextValue(ty)
                | Shape::DirectoryContext(ty)
                | Shape::ProcessArgs(ty)
                | Shape::Optional(ty) => vec![*ty],
                Shape::Dict(key, value)
                | Shape::OrderedDict(key, value)
                | Shape::DefaultListDict(key, value) => vec![*key, *value],
                Shape::Tuple(fields)
                | Shape::ModuleSpec(fields)
                | Shape::NamedTuple(_, fields)
                | Shape::PartialFrame(fields)
                | Shape::DeferredCmpFrame(_, fields)
                | Shape::Union(fields) => fields.clone(),
                Shape::IndexCallable(callable) | Shape::DeclaredCache(callable, _) => {
                    vec![*callable]
                }
                Shape::CachedCallable(c) => vec![c.callable, c.frame, c.table],
                Shape::DispatchCallable(d) => vec![d.callable, d.frame, d.registry],
                Shape::Callable(params, returns) => {
                    params.iter().copied().chain([*returns]).collect()
                }
                Shape::RecordConstructor(class, params) => params
                    .iter()
                    .copied()
                    .chain([Type::Class(*class)])
                    .collect(),
            };
            for child in children {
                self.verify_type(child, classes)?;
                if let Type::StrLiteral(child_id)
                | Type::Cursor(child_id)
                | Type::VarTuple(child_id)
                | Type::List(child_id)
                | Type::CacheState(child_id)
                | Type::Dict(child_id)
                | Type::ChainMap(child_id)
                | Type::Deque(child_id)
                | Type::Set(child_id)
                | Type::ModuleSpec(child_id)
                | Type::Tuple(child_id)
                | Type::Callable(child_id)
                | Type::Optional(child_id)
                | Type::Coroutine(child_id)
                | Type::Task(child_id)
                | Type::SimpleNamespace(child_id)
                | Type::DirectoryContext(child_id)
                | Type::ContextVar(child_id)
                | Type::ContextToken(child_id)
                | Type::Popen(_, _, child_id)
                | Type::Union(child_id) = child
                    && child_id >= id
                {
                    return Err(format!(
                        "type shape #{id} contains a forward or cyclic reference to {child}"
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Types {
    pub(crate) fn allow_plain_callbacks(&self, safe: HashSet<Type>) {
        self.0.borrow_mut().plain_callbacks = safe;
    }
    pub(crate) fn is_plain_callback(&self, ty: Type) -> bool {
        self.0.borrow().plain_callbacks.contains(&ty)
    }
    pub(crate) fn has_reflected_functions(&self) -> bool {
        !self.0.borrow().function_sites.is_empty()
    }
    pub(crate) fn add_function_site(&self, module: usize, offset: u32, site: FunctionSite) {
        self.0
            .borrow_mut()
            .source_sites
            .insert((module, offset), Rc::new(site));
    }
    pub(crate) fn bind_function_site(&self, module: usize, offset: u32, id: usize) {
        let mut storage = self.0.borrow_mut();
        if let Some(site) = storage.source_sites.get(&(module, offset)).cloned() {
            storage.function_sites.insert(id, site);
        }
    }
    pub(crate) fn function_site(&self, id: usize) -> Option<Rc<FunctionSite>> {
        self.0.borrow().function_sites.get(&id).cloned()
    }
    pub(crate) fn set_exception_declarations(&self, declarations: Vec<NamedException>) {
        self.0.borrow_mut().exceptions = declarations;
    }

    pub(crate) fn exception_declarations(&self) -> Ref<'_, [NamedException]> {
        Ref::map(self.0.borrow(), |storage| storage.exceptions.as_slice())
    }

    pub(crate) fn string_literal(&self, mut values: Vec<String>) -> Type {
        values.sort_unstable();
        values.dedup();
        Type::StrLiteral(self.intern(Shape::StringLiterals(values)))
    }

    pub(crate) fn literal_strings(&self, ty: Type) -> Option<Ref<'_, [String]>> {
        let Type::StrLiteral(id) = ty else {
            return None;
        };
        Some(Ref::map(self.0.borrow(), |storage| {
            match &storage.shapes[id] {
                Shape::StringLiterals(values) => values.as_slice(),
                _ => unreachable!("literal type identity has a string domain"),
            }
        }))
    }

    pub(crate) fn set_value_structs(&self, ids: HashSet<usize>) {
        self.0.borrow_mut().value_structs = ids;
    }

    pub(crate) fn is_value_struct(&self, ty: Type) -> bool {
        matches!(ty, Type::Class(id) if self.0.borrow().value_structs.contains(&id))
    }

    pub(crate) fn set_hashable_records(&self, records: HashSet<usize>) {
        self.0.borrow_mut().hashable_records = records;
    }

    pub(crate) fn set_pathlike_records(&self, records: HashSet<usize>) {
        self.0.borrow_mut().pathlike_records = records;
    }

    pub(crate) fn set_custom_truth(&self, records: HashSet<usize>) {
        self.0.borrow_mut().custom_truth = records;
    }

    pub(crate) fn concrete_subclasses(&self, class: usize) -> Ref<'_, [Type]> {
        Ref::map(self.0.borrow(), |storage| {
            storage
                .concrete_subclasses
                .get(&class)
                .map_or(&[][..], Vec::as_slice)
        })
    }

    pub(crate) fn add_concrete_subclass(&self, class: usize, bases: &[usize]) {
        let mut storage = self.0.borrow_mut();
        for &base in bases {
            storage
                .concrete_subclasses
                .entry(base)
                .or_default()
                .push(Type::Class(class));
        }
    }

    pub(crate) fn mark_fallible_key(&self, class: usize) -> bool {
        self.0.borrow_mut().fallible_keys.insert(class)
    }

    pub(crate) fn fallible_key(&self, ty: Type) -> bool {
        match ty {
            Type::Class(id) => self.0.borrow().fallible_keys.contains(&id),
            Type::Tuple(_) | Type::Optional(_) => self
                .children(ty)
                .into_iter()
                .any(|child| self.fallible_key(child)),
            _ => false,
        }
    }

    /// Scalar fields for which copying an asdict value needs no recursive copy.
    pub(crate) fn asdict_scalar(&self, ty: Type) -> bool {
        matches!(
            self.payload(ty).unwrap_or(ty),
            Type::None
                | Type::Bool
                | Type::Int
                | Type::Float
                | Type::Str
                | Type::StrLiteral(_)
                | Type::Bytes
        )
    }

    /// Hash keys have immutable value semantics. Tuple and frozen record fields
    /// may be nullable, including nested immutable records.
    pub(crate) fn hashable_key(&self, ty: Type) -> bool {
        matches!(
            ty,
            Type::Int
                | Type::Str
                | Type::StrLiteral(_)
                | Type::StrEnum(_)
                | Type::Enum(_)
                | Type::Bytes
                | Type::Logger
                | Type::Socket
                | Type::Path
                | Type::PurePath(_)
        ) || matches!(ty, Type::Tuple(_) | Type::Class(_)) && self.immutable_key_field(ty)
    }

    pub(crate) fn immutable_key_field(&self, ty: Type) -> bool {
        if self.is_len_argument(ty) {
            return false;
        }
        if let Some(payload) = self.payload(ty) {
            return self.immutable_key_field(payload);
        }
        match ty {
            Type::Int
            | Type::Bool
            | Type::Str
            | Type::StrLiteral(_)
            | Type::StrEnum(_)
            | Type::Enum(_)
            | Type::Bytes
            | Type::Path
            | Type::PurePath(_)
            | Type::Logger
            | Type::Socket
            | Type::None => true,
            Type::Class(id) => self.0.borrow().hashable_records.contains(&id),
            Type::Tuple(_) if self.is_callback_storage(ty) => false,
            Type::Tuple(_) => self
                .fields(ty)
                .unwrap()
                .into_iter()
                .all(|field| self.immutable_key_field(field)),
            _ => false,
        }
    }

    /// A sealed total order permits native stable sorting without comparison
    /// callbacks. Nullable fields and floats are excluded because they need
    /// fallible comparisons or Python's observable boxed-NaN identity.
    pub(crate) fn ordered_key(&self, ty: Type) -> bool {
        match ty {
            Type::Int
            | Type::Bool
            | Type::Str
            | Type::StrLiteral(_)
            | Type::StrEnum(_)
            | Type::Bytes => true,
            Type::Tuple(_) => self
                .fields(ty)
                .unwrap()
                .into_iter()
                .all(|field| field == Type::None || self.ordered_key(field)),
            _ => false,
        }
    }

    /// Joins values only when one existing concrete or nullable layout can hold both.
    pub(crate) fn join(&self, left: Type, right: Type) -> Option<Type> {
        if let (Some(left), Some(right)) = (self.directory_value(left), self.directory_value(right))
        {
            return self
                .join(left, right)
                .map(|value| self.directory_context(value));
        }
        if left == right
            || self.payload(left) == Some(right)
            || self.index_callable_signature(right) == Some(left)
        {
            Some(left)
        } else if self.payload(right) == Some(left)
            || self.index_callable_signature(left) == Some(right)
        {
            Some(right)
        } else if left == Type::None {
            Some(self.optional(right))
        } else if right == Type::None {
            Some(self.optional(left))
        } else {
            self.union(&[left, right])
        }
    }

    /// Canonicalizes a deliberately bounded union. Mutable container layouts
    /// remain invariant, and distinct numeric kinds keep their Python identity.
    pub(crate) fn union(&self, values: &[Type]) -> Option<Type> {
        let mut members = Vec::new();
        let mut nullable = false;
        for &value in values {
            let value = if let Some(payload) = self.payload(value) {
                nullable = true;
                payload
            } else {
                value
            };
            if value == Type::None {
                nullable = true;
            } else {
                for member in self.members(value).unwrap_or_else(|| vec![value]) {
                    if !members.contains(&member) {
                        members.push(member);
                    }
                }
            }
        }
        let ty = match members.as_slice() {
            [] if nullable => Type::None,
            [member] => *member,
            [Type::Bool, Type::Int] | [Type::Int, Type::Bool] => {
                Type::Union(self.intern(Shape::Union(vec![Type::Bool, Type::Int])))
            }
            choices @ [_, _, ..]
                if choices
                    .iter()
                    .all(|ty| matches!(ty, Type::Str | Type::StrLiteral(_))) =>
            {
                if choices.contains(&Type::Str) {
                    Type::Str
                } else {
                    let mut strings = Vec::new();
                    for &ty in choices {
                        strings.extend(self.literal_strings(ty).unwrap().iter().cloned());
                    }
                    self.string_literal(strings)
                }
            }
            [Type::Str, other] | [other, Type::Str]
                if matches!(
                    other,
                    Type::Bytes | Type::Path | Type::FrozenStrings | Type::Enum(_)
                ) || self.element(*other) == Some(Type::Str) =>
            {
                Type::Union(self.intern(Shape::Union(vec![Type::Str, *other])))
            }
            [Type::Str, record @ Type::Class(id)] | [record @ Type::Class(id), Type::Str]
                if self.0.borrow().pathlike_records.contains(id)
                    && !self.0.borrow().custom_truth.contains(id) =>
            {
                Type::Union(self.intern(Shape::Union(vec![Type::Str, *record])))
            }
            [record @ Type::Class(id), Type::Exception]
            | [Type::Exception, record @ Type::Class(id)]
                if !self.0.borrow().custom_truth.contains(id) =>
            {
                Type::Union(self.intern(Shape::Union(vec![*record, Type::Exception])))
            }
            choices @ [_, _, ..]
                if choices
                    .iter()
                    .all(|ty| matches!(ty, Type::Str | Type::Bytes | Type::Path)) =>
            {
                let ordered = [Type::Str, Type::Bytes, Type::Path]
                    .into_iter()
                    .filter(|ty| choices.contains(ty))
                    .collect();
                Type::Union(self.intern(Shape::Union(ordered)))
            }
            _ => return None,
        };
        Some(if nullable { self.optional(ty) } else { ty })
    }

    /// A closed builtin result that can be either a string or an integer.
    /// Narrow the result before using it as either concrete type.
    pub(crate) fn string_or_int(&self) -> Type {
        Type::Union(self.intern(Shape::Union(vec![Type::Str, Type::Int])))
    }

    pub(crate) fn is_bool_or_int(&self, ty: Type) -> bool {
        let Type::Union(id) = ty else { return false };
        matches!(&self.0.borrow().shapes[id], Shape::Union(members) if members == &[Type::Bool, Type::Int])
    }

    /// A universal newline decoder reports either one spelling, a tuple of the
    /// distinct spellings in canonical order, or None before it has seen one.
    pub(crate) fn text_newlines_result(&self) -> Type {
        let pair = self.tuple(vec![Type::Str; 2]);
        let triple = self.tuple(vec![Type::Str; 3]);
        self.optional(Type::Union(self.intern(Shape::Union(vec![
            Type::Str,
            pair,
            triple,
        ]))))
    }

    pub(crate) fn members(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Union(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::Union(members) => Some(members.clone()),
            _ => unreachable!("union type identity has a union shape"),
        }
    }

    /// Fixed tuples and nominal objects have no user-defined truth protocol.
    /// Unknown means the value must be inspected, including nullable payloads.
    pub(crate) fn truthiness(&self, ty: Type) -> Option<bool> {
        match ty {
            Type::None => Some(false),
            Type::Calendar(crate::ir::CalendarType::Delta) => None,
            Type::Tuple(_) => Some(!self.fields(ty).unwrap().is_empty()),
            Type::Class(id) if self.0.borrow().custom_truth.contains(&id) => None,
            Type::SimpleNamespace(_) | Type::ModuleSpec(_) => Some(true),
            Type::Class(_)
            | Type::Callable(_)
            | Type::Enum(_)
            | Type::Iterator
            | Type::Cursor(_)
            | Type::CacheState(_)
            | Type::ByteOutput
            | Type::TupleBuffer
            | Type::Exception
            | Type::ImportException
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::StringIO
            | Type::HandlerLock
            | Type::Path
            | Type::PurePath(_)
            | Type::SysImplementation
            | Type::CompletedProcess(..)
            | Type::EventLoop
            | Type::Coroutine(_)
            | Type::Task(_)
            | Type::TaskGroup
            | Type::Document(_)
            | Type::GeneratorContext
            | Type::DirectoryContext(_)
            | Type::Semaphore
            | Type::Popen(..)
            | Type::ActivePopen
            | Type::AsyncProcess(_)
            | Type::NativeModule
            | Type::ModuleNamespace
            | Type::LogFormatter
            | Type::LogRecord
            | Type::Logger
            | Type::TimedFileHandler
            | Type::ProcessSelectorClass
            | Type::ClockTime
            | Type::Calendar(_)
            | Type::StrDescriptor(_)
            | Type::Placeholder(_)
            | Type::FunctionRef
            | Type::OverloadStub
            | Type::PlainObject
            | Type::PathInfo
            | Type::CodecInfo
            | Type::Poll
            | Type::Socket
            | Type::CodecWriterClass
            | Type::CodecWriter
            | Type::Sha256
            | Type::JsonDecoder
            | Type::ZipFile
            | Type::ZipInfo
            | Type::CsvReader
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::ContextVar(_)
            | Type::ContextToken(_)
            | Type::ContextMissing
            | Type::SourceModule
            | Type::BinaryIO => Some(true),
            Type::Optional(_) if self.truthiness(self.payload(ty).unwrap()) == Some(false) => {
                Some(false)
            }
            _ => None,
        }
    }

    /// Truth excludes None; falsity excludes a payload only if that entire
    /// concrete type is always true. Empty containers and zero remain present.
    pub(crate) fn truth_branch(&self, ty: Type, truth: bool) -> Type {
        if let Some(payload) = self.payload(ty) {
            if truth {
                payload
            } else if self.truthiness(payload) == Some(true) {
                Type::None
            } else {
                ty
            }
        } else {
            ty
        }
    }

    fn intern(&self, shape: Shape) -> usize {
        let mut storage = self.0.borrow_mut();
        if let Some(&id) = storage.ids.get(&shape) {
            return id;
        }
        let id = storage.shapes.len();
        storage.shapes.push(shape.clone());
        storage.ids.insert(shape, id);
        id
    }

    pub(crate) fn var_tuple(&self, element: Type) -> Type {
        Type::VarTuple(self.intern(Shape::VarTuple(element)))
    }

    pub(crate) fn var_tuple_element(&self, ty: Type) -> Option<Type> {
        let Type::VarTuple(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::VarTuple(element) => Some(*element),
            Shape::DisplayTuple(_) => None,
            _ => unreachable!(),
        }
    }

    /// Homogeneous displays share the ordinary `tuple[T, ...]` type. Heterogeneous
    /// ones retain their guaranteed fixed positions on either side of a spread.
    pub(crate) fn display_tuple(&self, parts: Vec<(Type, bool)>) -> Type {
        let first = parts.first().map_or(Type::None, |(ty, _)| *ty);
        if parts.iter().all(|&(ty, _)| ty == first) {
            self.var_tuple(first)
        } else {
            Type::VarTuple(self.intern(Shape::DisplayTuple(parts)))
        }
    }

    /// Return the field type only when every possible length agrees at this index.
    pub(crate) fn display_tuple_index(&self, ty: Type, index: Option<i64>) -> Option<Type> {
        if let Some(element) = self.var_tuple_element(ty) {
            return Some(element);
        }
        let Type::VarTuple(id) = ty else { return None };
        let storage = self.0.borrow();
        let Shape::DisplayTuple(parts) = &storage.shapes[id] else {
            return None;
        };
        let index = index?;
        let reverse = index < 0;
        let at = if reverse {
            index.unsigned_abs() - 1
        } else {
            index as u64
        };
        let mut minimum = 0;
        let mut variable = false;
        let mut result = None;
        for i in 0..parts.len() {
            let (field, repeated) = parts[if reverse { parts.len() - i - 1 } else { i }];
            if at >= minimum && (variable || repeated || at == minimum) {
                if result.is_some_and(|ty| ty != field) {
                    return None;
                }
                result = Some(field);
            }
            variable |= repeated;
            minimum += u64::from(!repeated);
        }
        result
    }

    pub(crate) fn cursor(&self, element: Type) -> Type {
        Type::Cursor(self.intern(Shape::Cursor(element)))
    }

    pub(crate) fn cursor_element(&self, ty: Type) -> Option<Type> {
        let Type::Cursor(id) = ty else { return None };
        let Shape::Cursor(element) = self.0.borrow().shapes[id] else {
            unreachable!()
        };
        Some(element)
    }

    pub(crate) fn list(&self, element: Type) -> Type {
        if element == Type::Int {
            Type::ListInt
        } else {
            Type::List(self.intern(Shape::List(element)))
        }
    }

    /// A live len argument owns its concrete value after its class and hash diagnostic.
    pub(crate) fn len_argument(&self) -> Type {
        Type::Tuple(self.intern(Shape::LenFrame(vec![Type::Str, Type::Str, self.len_key()])))
    }

    /// The private immutable key starts with the class; its normalized value follows.
    pub(crate) fn len_key(&self) -> Type {
        Type::Tuple(self.intern(Shape::LenFrame(vec![Type::Str])))
    }

    pub(crate) fn is_len_argument(&self, ty: Type) -> bool {
        matches!(ty, Type::Tuple(id) if matches!(&self.0.borrow().shapes[id], Shape::LenFrame(prefix) if prefix.len() == 3))
    }

    /// Return only Python sequence fields; private callable frames expose ABI prefixes.
    pub(crate) fn len_tuple_fields(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Tuple(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::Tuple(fields)
            | Shape::NamedTuple(
                TupleOrigin::StructSequence(..) | TupleOrigin::LibraryNamedTuple(..),
                fields,
            ) => Some(fields.clone()),
            _ => None,
        }
    }

    pub(crate) fn tuple(&self, elements: Vec<Type>) -> Type {
        Type::Tuple(self.intern(Shape::Tuple(elements)))
    }

    /// A callback retains its actual argument in a singleton positional tuple.
    pub(crate) fn callback_argument(&self) -> Type {
        let positional = self.callback_positional();
        Type::Tuple(self.intern(Shape::CallbackArgument(positional)))
    }

    pub(crate) fn is_callback_argument(&self, ty: Type) -> bool {
        matches!(ty, Type::Tuple(id) if matches!(self.0.borrow().shapes[id], Shape::CallbackArgument(_)))
    }

    fn is_callback_storage(&self, ty: Type) -> bool {
        matches!(ty, Type::Tuple(id) if matches!(self.0.borrow().shapes[id], Shape::CallbackArgument(_) | Shape::CallbackPositional))
    }

    /// Its element has the concrete type recorded by the allocating call site.
    pub(crate) fn callback_positional(&self) -> Type {
        Type::Tuple(self.intern(Shape::CallbackPositional))
    }

    pub(crate) fn module_spec(&self, fields: Vec<Type>) -> Type {
        let id = self.intern(Shape::ModuleSpec(fields));
        self.0.borrow_mut().has_module_spec = true;
        Type::ModuleSpec(id)
    }
    pub(crate) fn module_spec_fields(&self, id: usize) -> Vec<Type> {
        let storage = self.0.borrow();
        let Shape::ModuleSpec(fields) = &storage.shapes[id] else {
            unreachable!("invalid ModuleSpec layout")
        };
        fields.clone()
    }
    pub(crate) fn module_spec_layouts(&self) -> Vec<(Type, Vec<Type>)> {
        self.0
            .borrow()
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(id, shape)| {
                if let Shape::ModuleSpec(fields) = shape {
                    Some((Type::ModuleSpec(id), fields.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
    /// The type can also appear inside a tuple or optional without an annotation.
    pub(crate) fn contains_module_spec(&self, ty: Type) -> bool {
        if matches!(ty, Type::ModuleSpec(_)) {
            return true;
        }
        // Ordinary programs need no recursive scan for this nominal protocol.
        if !self.0.borrow().has_module_spec {
            return false;
        }
        if !matches!(
            ty,
            Type::ModuleSpec(_)
                | Type::Tuple(_)
                | Type::List(_)
                | Type::Dict(_)
                | Type::Set(_)
                | Type::Optional(_)
                | Type::Union(_)
        ) {
            return false;
        }
        let mut seen = HashSet::new();
        let mut pending = vec![ty];
        while let Some(ty) = pending.pop() {
            if matches!(ty, Type::ModuleSpec(_)) {
                return true;
            }
            if seen.insert(ty) {
                pending.extend(self.children(ty));
            }
        }
        false
    }

    /// The percent formatter reads positional operands from an exact tuple's descriptor.
    pub(crate) fn percent_tuple(&self, ty: Type) -> bool {
        let Type::Tuple(id) = ty else { return false };
        matches!(&self.0.borrow().shapes[id], Shape::Tuple(fields)
            if fields.iter().all(|ty| matches!(ty,
                Type::None | Type::Bool | Type::Int | Type::Float | Type::Str | Type::StrLiteral(_) | Type::Bytes)))
    }

    /// Stat results use ten visible integer slots and a separate tail for named attributes.
    pub(crate) fn stat_result(&self) -> Type {
        self.struct_sequence(
            STAT_RESULT_NAME,
            &STAT_SEQUENCE_FIELDS,
            vec![Type::Int; STAT_SEQUENCE_FIELDS.len()],
        )
    }

    pub(crate) fn is_stat_result(&self, ty: Type) -> bool {
        matches!(ty, Type::Tuple(id) if matches!(self.0.borrow().shapes[id],
            Shape::NamedTuple(TupleOrigin::StructSequence(STAT_RESULT_NAME, _), _)))
    }

    pub(crate) fn declare_namedtuple(&self, definition: DefinitionId, declaration: NamedTuple) {
        self.0
            .borrow_mut()
            .named_tuples
            .insert(definition, Rc::new(declaration));
    }

    pub(crate) fn namedtuple(&self, definition: DefinitionId) -> Option<Rc<NamedTuple>> {
        self.0.borrow().named_tuples.get(&definition).cloned()
    }

    pub(crate) fn namedtuple_layout(&self, origin: TupleOrigin, fields: Vec<Type>) -> Type {
        Type::Tuple(self.intern(Shape::NamedTuple(origin, fields)))
    }

    /// A native struct sequence shares the tuple descriptor but has no Python declaration.
    pub(crate) fn struct_sequence(
        &self,
        name: &'static str,
        names: &'static [&'static str],
        fields: Vec<Type>,
    ) -> Type {
        assert_eq!(names.len(), fields.len());
        self.namedtuple_layout(TupleOrigin::StructSequence(name, names), fields)
    }

    pub(crate) fn tuple_origin(&self, ty: Type) -> Option<TupleOrigin> {
        let Type::Tuple(id) = ty else { return None };
        match self.0.borrow().shapes[id] {
            Shape::NamedTuple(origin, _) => Some(origin),
            _ => None,
        }
    }

    pub(crate) fn namedtuple_metadata(&self, origin: TupleOrigin) -> Option<Rc<NamedTuple>> {
        match origin {
            TupleOrigin::Declaration(id) => self.namedtuple(id),
            TupleOrigin::StructSequence(name, fields)
            | TupleOrigin::LibraryNamedTuple(name, fields) => Some(Rc::new(NamedTuple {
                name: name.to_string(),
                fields: fields.iter().map(|v| v.to_string()).collect(),
            })),
        }
    }

    pub(crate) fn tuple_metadata(&self, ty: Type) -> Option<Rc<NamedTuple>> {
        self.namedtuple_metadata(self.tuple_origin(ty)?)
    }

    /// Library tuple subclasses expose only their sequence slots to slicing.
    pub(crate) fn library_tuple_fields(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Tuple(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::NamedTuple(
                TupleOrigin::StructSequence(..) | TupleOrigin::LibraryNamedTuple(..),
                fields,
            ) => Some(fields.clone()),
            _ => None,
        }
    }

    pub(crate) fn namedtuple_definition(&self, ty: Type) -> Option<DefinitionId> {
        match self.tuple_origin(ty)? {
            TupleOrigin::Declaration(definition) => Some(definition),
            _ => None,
        }
    }

    /// Immutable private storage for the callable and arguments retained by partial.
    pub(crate) fn partial_frame(&self, fields: Vec<Type>) -> Type {
        Type::Tuple(self.intern(Shape::PartialFrame(fields)))
    }

    /// Retain a local comparator's definition until a comparison supplies operand types.
    pub(crate) fn cmp_frame(&self, fields: Vec<Type>, source: Option<(usize, u32)>) -> Type {
        match source {
            Some(site) => Type::Tuple(self.intern(Shape::DeferredCmpFrame(site, fields))),
            None => self.partial_frame(fields),
        }
    }

    pub(crate) fn deferred_cmp_source(&self, layout: usize) -> Option<(usize, u32)> {
        match self.0.borrow().shapes[layout] {
            Shape::DeferredCmpFrame(site, _) => Some(site),
            _ => None,
        }
    }

    pub(crate) fn is_partial_frame(&self, ty: Type) -> bool {
        matches!(ty, Type::Tuple(id) if matches!(self.0.borrow().shapes[id], Shape::PartialFrame(_) | Shape::DeferredCmpFrame(..)))
    }

    pub(crate) fn process_args(&self, command: Type) -> usize {
        self.intern(Shape::ProcessArgs(command))
    }

    pub(crate) fn command_type(&self, id: usize) -> Type {
        let Shape::ProcessArgs(command) = self.0.borrow().shapes[id] else {
            unreachable!("Popen carries its original command type")
        };
        command
    }

    /// An unqualified CompletedProcess keeps its original list-of-strings ABI.
    pub(crate) fn completed_command(&self, id: Option<usize>) -> Type {
        id.map_or_else(|| self.list(Type::Str), |id| self.command_type(id))
    }

    pub(crate) fn completed(&self, capture: crate::ir::ProcessCapture, command: Type) -> Type {
        let id = (command != self.list(Type::Str)).then(|| self.process_args(command));
        Type::CompletedProcess(capture, id)
    }

    pub(crate) fn directory_context(&self, result: Type) -> Type {
        Type::DirectoryContext(self.intern(Shape::DirectoryContext(result)))
    }

    pub(crate) fn context_var(&self, value: Type) -> Type {
        Type::ContextVar(self.intern(Shape::ContextValue(value)))
    }

    pub(crate) fn context_value(&self, ty: Type) -> Option<Type> {
        let (Type::ContextVar(id) | Type::ContextToken(id)) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::ContextValue(value) => Some(value),
            _ => unreachable!("context value shape"),
        }
    }

    pub(crate) fn directory_value(&self, ty: Type) -> Option<Type> {
        let Type::DirectoryContext(id) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::DirectoryContext(value) => Some(value),
            _ => unreachable!("directory context carries its enter type"),
        }
    }

    pub(crate) fn coroutine(&self, result: Type) -> Type {
        Type::Coroutine(self.intern(Shape::Awaitable(result)))
    }

    pub(crate) fn awaited(&self, ty: Type) -> Option<Type> {
        let (Type::Coroutine(id) | Type::Task(id)) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::Awaitable(result) => Some(result),
            _ => unreachable!("awaitable has an exact result type"),
        }
    }

    /// Callable types describe full positional calls; names and defaults remain
    /// properties of direct source definitions rather than part of this ABI.
    pub(crate) fn callable(&self, params: Vec<Type>, returns: Type) -> Type {
        Type::Callable(self.intern(Shape::Callable(params, returns)))
    }

    /// A subscription method accepts booleans in its first positional integer slot.
    pub(crate) fn index_callable(&self, callable: Type) -> Type {
        Type::Callable(self.intern(Shape::IndexCallable(callable)))
    }

    pub(crate) fn index_callable_signature(&self, ty: Type) -> Option<Type> {
        let Type::Callable(id) = ty else { return None };
        match self.0.borrow().shapes.get(id) {
            Some(Shape::IndexCallable(signature)) => Some(*signature),
            _ => None,
        }
    }

    /// A class value can construct an instance but owns no instance or bound receiver.
    pub(crate) fn record_constructor(&self, base: usize, params: Vec<Type>) -> Type {
        Type::Callable(self.intern(Shape::RecordConstructor(base, params)))
    }

    pub(crate) fn constructor_base(&self, ty: Type) -> Option<usize> {
        let Type::Callable(id) = ty else { return None };
        match self.0.borrow().shapes.get(id) {
            Some(Shape::RecordConstructor(base, _)) => Some(*base),
            _ => None,
        }
    }

    pub(crate) fn register_constructor(&self, function: usize, class: usize) {
        self.0
            .borrow_mut()
            .constructor_targets
            .insert(function, class);
    }

    /// Every concrete Python class has one identity across all its type[T] views.
    /// The compilation entry owns each singleton in an ordinary managed global.
    pub(crate) fn reserve_constructor_globals(
        &self,
        globals: &mut Vec<Type>,
        functions: &[crate::ir::Function],
        entry: &crate::ir::Function,
    ) {
        let mut classes = BTreeMap::new();
        for function in functions.iter().chain([entry]) {
            crate::ir::walk_expressions(&function.body, &mut |expr| {
                if let crate::ir::ExprKind::Callable {
                    function,
                    receiver: None,
                } = &expr.kind
                    && self.constructor_base(expr.ty).is_some()
                {
                    classes
                        .entry(self.0.borrow().constructor_targets[function])
                        .or_insert(expr.ty);
                }
            });
        }
        let mut storage = self.0.borrow_mut();
        for (class, ty) in classes {
            storage.constructor_slots.insert(class, globals.len());
            globals.push(ty);
        }
    }

    pub(crate) fn constructor_slot(&self, function: usize, ty: Type) -> Option<usize> {
        self.constructor_base(ty)?;
        let storage = self.0.borrow();
        Some(storage.constructor_slots[&storage.constructor_targets[&function]])
    }

    /// A module cache keeps its declaration's identity when bound to another name.
    pub(crate) fn declared_cache(&self, callable: Type, id: usize, policy: CachePolicy) -> Type {
        self.0.borrow_mut().cache_adapters.insert(id, policy);
        Type::Callable(self.intern(Shape::DeclaredCache(callable, id)))
    }

    /// Adapter identity survives storage in a parameter or return typed as Callable.
    pub(crate) fn cache_adapter(&self, function: usize) -> Option<CachePolicy> {
        self.0.borrow().cache_adapters.get(&function).copied()
    }

    pub(crate) fn cache_declaration(&self, ty: Type) -> Option<(Type, usize)> {
        let Type::Callable(id) = ty else { return None };
        match self.0.borrow().shapes.get(id) {
            Some(Shape::DeclaredCache(callable, declaration)) => Some((*callable, *declaration)),
            _ => None,
        }
    }

    /// The public positional ABI of a wrapper, including nested applied caches.
    pub(crate) fn wrapped_signature(&self, ty: Type) -> Option<Type> {
        let shapes = self.0.borrow();
        let mut current = ty;
        while let Type::Callable(id) = current {
            current = match shapes.shapes.get(id)? {
                Shape::IndexCallable(underlying) | Shape::DeclaredCache(underlying, _) => {
                    *underlying
                }
                Shape::CachedCallable(cache) => cache.callable,
                Shape::DispatchCallable(dispatcher) => dispatcher.callable,
                Shape::Callable(..) => return (current != ty).then_some(current),
                _ => return None,
            };
        }
        None
    }

    pub(crate) fn cached_callable(
        &self,
        layout: CachedFunction,
        id: usize,
        policy: CachePolicy,
    ) -> Type {
        self.0.borrow_mut().cache_adapters.insert(id, policy);
        Type::Callable(self.intern(Shape::CachedCallable(layout)))
    }

    pub(crate) fn cached_function(&self, ty: Type) -> Option<CachedFunction> {
        let Type::Callable(id) = ty else { return None };
        match self.0.borrow().shapes[id] {
            Shape::CachedCallable(layout) => Some(layout),
            _ => None,
        }
    }

    /// Only the dispatcher constructs this registry; source dictionaries cannot store callables.
    pub(crate) fn dispatch_callable(&self, callable: Type) -> Type {
        let registry = Type::Dict(self.intern(Shape::DispatchRegistry(callable)));
        let metadata = self.optional(self.tuple(vec![Type::Str, self.optional(Type::Str)]));
        let frame = self.partial_frame(vec![callable, registry, metadata]);
        Type::Callable(self.intern(Shape::DispatchCallable(DispatchFunction {
            callable,
            frame,
            registry,
            metadata,
        })))
    }

    pub(crate) fn dispatcher(&self, ty: Type) -> Option<DispatchFunction> {
        let Type::Callable(id) = ty else { return None };
        match self.0.borrow().shapes[id] {
            Shape::DispatchCallable(layout) => Some(layout),
            _ => None,
        }
    }

    pub(crate) fn is_dispatch_registry(&self, ty: Type) -> bool {
        matches!(ty, Type::Dict(id) if matches!(self.0.borrow().shapes[id], Shape::DispatchRegistry(_)))
    }

    pub(crate) fn signature(&self, ty: Type) -> Option<(Vec<Type>, Type)> {
        let Type::Callable(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::Callable(params, returns) => Some((params.clone(), *returns)),
            Shape::RecordConstructor(base, params) => Some((params.clone(), Type::Class(*base))),
            Shape::IndexCallable(callable) | Shape::DeclaredCache(callable, _) => {
                self.signature(*callable)
            }
            Shape::CachedCallable(c) => self.signature(c.callable),
            Shape::DispatchCallable(d) => self.signature(d.callable),
            _ => unreachable!("callable type identity has a callable shape"),
        }
    }

    /// A ChainMap and its concrete dictionaries share their key/value shape.
    pub(crate) fn chainmap(&self, key: Type, value: Type) -> Type {
        Type::ChainMap(self.intern(Shape::Dict(key, value)))
    }

    /// Look through the ChainMap's dictionary shape without treating its
    /// owner as a dictionary in ordinary dictionary operations.
    pub(crate) fn chainmap_entries(&self, ty: Type) -> Option<(Type, Type)> {
        let Type::ChainMap(id) = ty else { return None };
        self.mapping(Type::Dict(id))
    }

    pub(crate) fn dict(&self, key: Type, value: Type) -> Type {
        Type::Dict(self.intern(Shape::Dict(key, value)))
    }

    pub(crate) fn counter(&self, key: Type) -> Type {
        Type::Dict(self.intern(Shape::Counter(key)))
    }

    pub(crate) fn ordered_dict(&self, key: Type, value: Type) -> Type {
        Type::Dict(self.intern(Shape::OrderedDict(key, value)))
    }

    pub(crate) fn is_ordered_dict(&self, ty: Type) -> bool {
        matches!(ty, Type::Dict(id) if matches!(self.0.borrow().shapes[id], Shape::OrderedDict(..)))
    }

    pub(crate) fn is_counter(&self, ty: Type) -> bool {
        matches!(ty, Type::Dict(id) if matches!(self.0.borrow().shapes[id], Shape::Counter(_)))
    }

    /// The factory is part of the source contract; storage is still a dictionary.
    pub(crate) fn default_list_dict(&self, key: Type, value: Type) -> Type {
        debug_assert!(self.element(value).is_some());
        Type::Dict(self.intern(Shape::DefaultListDict(key, value)))
    }

    pub(crate) fn is_default_list_dict(&self, ty: Type) -> bool {
        matches!(ty, Type::Dict(id) if matches!(self.0.borrow().shapes[id], Shape::DefaultListDict(..)))
    }

    pub(crate) fn deque(&self, element: Type) -> Type {
        Type::Deque(self.intern(Shape::Deque(element)))
    }

    pub(crate) fn deque_element(&self, ty: Type) -> Option<Type> {
        let Type::Deque(id) = ty else {
            return None;
        };
        let Shape::Deque(element) = self.0.borrow().shapes[id] else {
            unreachable!("deque identity has a deque shape")
        };
        Some(element)
    }

    /// Mutable sequences with integer positions; their storage layouts remain distinct.
    pub(crate) fn indexed_element(&self, ty: Type) -> Option<Type> {
        self.element(ty).or_else(|| self.deque_element(ty))
    }

    pub(crate) fn set(&self, element: Type) -> Type {
        Type::Set(self.intern(Shape::Set(element)))
    }

    /// Kept separate from list elements so sets cannot acquire indexed operations.
    pub(crate) fn set_element(&self, ty: Type) -> Option<Type> {
        let Type::Set(id) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::Set(element) => Some(element),
            _ => unreachable!("set type identity has a set shape"),
        }
    }

    /// Element type for membership and iteration, including immutable set views.
    pub(crate) fn readable_set_element(&self, ty: Type) -> Option<Type> {
        match ty {
            Type::StringKeys | Type::FrozenStrings => Some(Type::Str),
            _ => self.set_element(ty),
        }
    }

    pub(crate) fn optional(&self, element: Type) -> Type {
        if matches!(element, Type::None | Type::Optional(_)) {
            element
        } else {
            Type::Optional(self.intern(Shape::Optional(element)))
        }
    }

    pub(crate) fn element(&self, ty: Type) -> Option<Type> {
        match ty {
            Type::ListInt => Some(Type::Int),
            Type::List(id) => match self.0.borrow().shapes[id] {
                Shape::List(element) => Some(element),
                _ => unreachable!("list type identity has a list shape"),
            },
            _ => None,
        }
    }

    pub(crate) fn forward_ref(&self, owner: Type) -> Type {
        let module = self.optional(Type::Str);
        let Type::Tuple(id) = self.tuple(vec![Type::Str, module, owner, Type::Bool, Type::Bool])
        else {
            unreachable!()
        };
        Type::ForwardRef(id)
    }

    pub(crate) fn fields(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Tuple(id) = ty else {
            return None;
        };
        match &self.0.borrow().shapes[id] {
            Shape::LenFrame(prefix) => Some(prefix.clone()),
            Shape::CallbackArgument(positional) => Some(vec![Type::Str, *positional, Type::Bool]),
            Shape::CallbackPositional => Some(Vec::new()),
            Shape::Tuple(fields)
            | Shape::NamedTuple(_, fields)
            | Shape::PartialFrame(fields)
            | Shape::DeferredCmpFrame(_, fields) => Some(fields.clone()),
            _ => unreachable!("tuple type identity has a tuple shape"),
        }
    }

    /// A runtime tuple read must agree on ownership and storage for every selectable field.
    pub(crate) fn homogeneous_tuple_element(&self, ty: Type) -> Option<Type> {
        let Type::Tuple(id) = ty else {
            return None;
        };
        let shapes = self.0.borrow();
        let (Shape::Tuple(fields) | Shape::NamedTuple(_, fields)) = &shapes.shapes[id] else {
            return None;
        };
        let widen = |t| match t {
            Type::StrLiteral(_) => Type::Str,
            other => other,
        };
        // An empty tuple raises before a result can be observed.
        let element = fields.first().copied().map_or(Type::None, widen);
        fields
            .iter()
            .all(|&field| widen(field) == element)
            .then_some(element)
    }

    /// Enumerates concrete tuple layouts for native field access and destruction.
    pub(crate) fn tuples(&self) -> Vec<(Type, Vec<Type>)> {
        self.0
            .borrow()
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(id, shape)| match shape {
                Shape::LenFrame(prefix) => Some((Type::Tuple(id), prefix.clone())),
                Shape::CallbackArgument(positional) => {
                    Some((Type::Tuple(id), vec![Type::Str, *positional, Type::Bool]))
                }
                Shape::CallbackPositional => Some((Type::Tuple(id), Vec::new())),
                Shape::Tuple(fields)
                | Shape::NamedTuple(_, fields)
                | Shape::PartialFrame(fields)
                | Shape::DeferredCmpFrame(_, fields) => Some((Type::Tuple(id), fields.clone())),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn payload(&self, ty: Type) -> Option<Type> {
        let Type::Optional(id) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::Optional(element) => Some(element),
            _ => unreachable!("optional type identity has an optional shape"),
        }
    }

    pub(crate) fn mapping(&self, ty: Type) -> Option<(Type, Type)> {
        let Type::Dict(id) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::Counter(key) => Some((key, Type::Int)),
            Shape::DispatchRegistry(target) => Some((Type::Str, target)),
            Shape::Dict(key, value)
            | Shape::OrderedDict(key, value)
            | Shape::DefaultListDict(key, value) => Some((key, value)),
            _ => unreachable!("dictionary type identity has a dictionary shape"),
        }
    }

    /// Closing a file or pipe, removing a temporary directory, or releasing a
    /// suspended frame can make the timing of the final drop observable.
    pub(crate) fn may_own_file(
        &self,
        ty: Type,
        class_fields: &[Vec<Type>],
        opaque_callables: bool,
    ) -> bool {
        let mut pending = vec![ty];
        let mut seen = HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            match ty {
                // The opaque payload may own an observable resource through a container.
                Type::Tuple(_) if self.is_callback_storage(ty) => return true,
                Type::Cursor(_)
                | Type::TextIO
                | Type::StringIO
                | Type::HandlerLock
                | Type::BinaryIO
                | Type::CsvReader
                | Type::ZipFile
                | Type::Socket
                | Type::Popen(..)
                | Type::ActivePopen
                | Type::AsyncProcess(_)
                | Type::DirectoryContext(_)
                | Type::Context
                | Type::ContextToken(_)
                | Type::GeneratorContext
                | Type::EventLoop
                | Type::Coroutine(_)
                | Type::Task(_)
                | Type::TaskGroup => return true,
                Type::Callable(_) if opaque_callables => return true,
                Type::Class(class) => {
                    pending.extend_from_slice(&class_fields[class]);
                    // A base-typed reference can own fields introduced by its concrete layout.
                    pending.extend(self.concrete_subclasses(class).iter().copied());
                }
                _ => pending.extend(self.children(ty)),
            }
        }
        false
    }

    /// Enumerates stored types for the class-field acyclicity check.
    pub(crate) fn children(&self, ty: Type) -> Vec<Type> {
        if let Type::ForwardRef(id) | Type::AnnotationValue(id) | Type::CmpKey(id, _) = ty {
            return self.fields(Type::Tuple(id)).unwrap();
        }
        if let Type::ModuleSpec(id) = ty {
            return self.module_spec_fields(id);
        }
        if let Type::VarTuple(id) = ty
            && let Shape::DisplayTuple(parts) = &self.0.borrow().shapes[id]
        {
            return parts.iter().map(|&(field, _)| field).collect();
        }
        if let Type::ChainMap(id) | Type::SimpleNamespace(id) = ty {
            return vec![Type::Dict(id)];
        }
        if let Some(element) = self
            .element(ty)
            .or_else(|| self.payload(ty))
            .or_else(|| self.set_element(ty))
            .or_else(|| self.deque_element(ty))
            .or_else(|| self.cursor_element(ty))
            .or_else(|| self.context_value(ty))
            .or_else(|| self.var_tuple_element(ty))
        {
            vec![element]
        } else if let Some((key, value)) = self.mapping(ty) {
            vec![key, value]
        } else {
            self.fields(ty)
                .or_else(|| self.members(ty))
                .unwrap_or_default()
        }
    }

    pub(crate) fn describe(&self, ty: Type, class_name: &impl Fn(usize) -> String) -> String {
        let storage = self.0.borrow();
        let mut description = TypeDescription {
            shapes: &storage.shapes,
            class_name,
            text: String::new(),
            remaining_nodes: 256,
        };
        if description.write_type(ty).is_err() {
            description.text.push_str("...");
        }
        description.text
    }
}

/// A shared type can have an exponentially larger spelling than its source.
struct TypeDescription<'a> {
    shapes: &'a [Shape],
    class_name: &'a dyn Fn(usize) -> String,
    text: String,
    remaining_nodes: usize,
}

impl fmt::Write for TypeDescription<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let remaining = MAX_DESCRIPTION_BYTES - self.text.len();
        if s.len() > remaining {
            self.text.push_str(&s[..s.floor_char_boundary(remaining)]);
            return Err(fmt::Error);
        }
        self.text.push_str(s);
        Ok(())
    }
}

impl TypeDescription<'_> {
    fn write_types(&mut self, types: &[Type], separator: &str) -> fmt::Result {
        for (i, &ty) in types.iter().enumerate() {
            if i != 0 {
                self.write_str(separator)?;
            }
            self.write_type(ty)?;
        }
        Ok(())
    }

    fn write_type(&mut self, ty: Type) -> fmt::Result {
        self.remaining_nodes = self.remaining_nodes.checked_sub(1).ok_or(fmt::Error)?;
        let id = match ty {
            Type::StrLiteral(id)
            | Type::List(id)
            | Type::Cursor(id)
            | Type::VarTuple(id)
            | Type::Tuple(id)
            | Type::Callable(id)
            | Type::ContextVar(id)
            | Type::ContextToken(id)
            | Type::Dict(id)
            | Type::ChainMap(id)
            | Type::Deque(id)
            | Type::Set(id)
            | Type::Optional(id)
            | Type::Union(id) => id,
            Type::Class(id) | Type::NamedException(id) | Type::StrEnum(id) | Type::Enum(id) => {
                return self.write_str(&(self.class_name)(id));
            }
            _ => return write!(self, "{ty}"),
        };
        let shape = &self.shapes[id];
        match shape {
            Shape::StringLiterals(values) => {
                self.write_str("Literal[")?;
                for (i, value) in values.iter().enumerate() {
                    if i != 0 {
                        self.write_str(", ")?;
                    }
                    write!(self, "{value:?}")?;
                }
            }
            Shape::LenFrame(prefix) => {
                return self.write_str(if prefix.len() == 3 {
                    "len argument"
                } else {
                    "len cache key"
                });
            }
            Shape::CallbackArgument(_) => return self.write_str("object"),
            Shape::CallbackPositional => return self.write_str("callback positional arguments"),
            Shape::ModuleSpec(_) => return self.write_str("importlib.machinery.ModuleSpec"),
            Shape::IndexCallable(callable) | Shape::DeclaredCache(callable, _) => {
                return self.write_type(*callable);
            }
            Shape::ContextValue(value) => {
                self.write_str(if matches!(ty, Type::ContextVar(_)) {
                    "ContextVar["
                } else {
                    "Token["
                })?;
                self.write_type(*value)?;
            }
            Shape::CachedCallable(c) => return self.write_type(c.callable),
            Shape::DispatchCallable(d) => return self.write_type(d.callable),
            Shape::RecordConstructor(class, _) => {
                self.write_str("type[")?;
                self.write_type(Type::Class(*class))?;
            }
            Shape::Callable(params, returns) => {
                self.write_str("Callable[[")?;
                self.write_types(params, ", ")?;
                self.write_str("], ")?;
                self.write_type(*returns)?;
            }
            Shape::VarTuple(element) => {
                self.write_str("tuple[")?;
                self.write_type(*element)?;
                self.write_str(", ...")?;
            }
            Shape::DisplayTuple(parts) => {
                self.write_str("tuple[")?;
                for (i, (ty, repeated)) in parts.iter().enumerate() {
                    if i != 0 {
                        self.write_str(", ")?;
                    }
                    if *repeated {
                        self.write_str("*tuple[")?;
                    }
                    self.write_type(*ty)?;
                    if *repeated {
                        self.write_str(", ...]")?;
                    }
                }
            }
            Shape::Cursor(element) => {
                self.write_str("Iterator[")?;
                self.write_type(*element)?;
            }
            Shape::List(element) | Shape::Set(element) => {
                self.write_str(if matches!(shape, Shape::List(_)) {
                    "list["
                } else {
                    "set["
                })?;
                self.write_type(*element)?;
            }
            Shape::Tuple(fields)
            | Shape::NamedTuple(_, fields)
            | Shape::PartialFrame(fields)
            | Shape::DeferredCmpFrame(_, fields) => {
                self.write_str("tuple[")?;
                self.write_types(fields, ", ")?;
            }
            Shape::Deque(element) => {
                self.write_str("deque[")?;
                self.write_type(*element)?;
            }
            Shape::Counter(key) => {
                self.write_str("Counter[")?;
                self.write_type(*key)?;
            }
            Shape::DispatchRegistry(target) => {
                self.write_str("dict[str, ")?;
                self.write_type(*target)?;
            }
            Shape::Dict(key, value)
            | Shape::OrderedDict(key, value)
            | Shape::DefaultListDict(key, value) => {
                self.write_str(match shape {
                    _ if matches!(ty, Type::ChainMap(_)) => "ChainMap[",
                    Shape::Dict(..) => "dict[",
                    Shape::OrderedDict(..) => "OrderedDict[",
                    _ => "defaultdict[",
                })?;
                self.write_type(*key)?;
                self.write_str(", ")?;
                self.write_type(*value)?;
            }
            Shape::DirectoryContext(_) | Shape::ProcessArgs(_) | Shape::Awaitable(_) => {
                unreachable!("handled by the nominal type spelling")
            }
            Shape::Optional(element) => {
                self.write_type(*element)?;
                return self.write_str(" | None");
            }
            Shape::Union(members) => return self.write_types(members, " | "),
        }
        self.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuple_elements_can_own_resources_through_contexts_and_suspended_frames() {
        let types = Types::default();
        for resource in [
            types.directory_context(Type::Str),
            types.coroutine(Type::Int),
            Type::AsyncProcess(1),
        ] {
            assert!(types.may_own_file(types.tuple(vec![resource]), &[], false));
        }
        assert!(!types.may_own_file(types.tuple(vec![Type::Path]), &[], false));
    }

    #[test]
    fn shared_tuple_descriptions_stop_expanding() {
        let types = Types::default();
        let pair = types.tuple(vec![Type::Int, Type::Int]);
        let name = |_| unreachable!();
        assert_eq!(types.describe(pair, &name), "tuple[int, int]");
        let mut ty = pair;
        for _ in 0..30 {
            ty = types.tuple(vec![ty, ty]);
        }
        let text = types.describe(ty, &name);
        assert!(text.starts_with("tuple[tuple[") && text.ends_with("..."));
        assert!(text.len() <= MAX_DESCRIPTION_BYTES + 3);
        let literal = types.string_literal(vec!["雪".repeat(400)]);
        let text = types.describe(literal, &name);
        assert!(text.starts_with("Literal[\"雪") && text.ends_with("..."));
        assert!(text.len() <= MAX_DESCRIPTION_BYTES + 3);
    }
}
