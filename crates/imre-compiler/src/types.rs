//! Interns concrete container types while keeping IR type identities small and exact.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write};

use crate::ir::Type;
use crate::modules::DefinitionId;
use std::rc::Rc;

const MAX_DESCRIPTION_BYTES: usize = 1024;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Shape {
    StringLiterals(Vec<String>),
    Cursor(Type),
    List(Type),
    Dict(Type, Type),
    DefaultListDict(Type, Type),
    Counter(Type),
    Set(Type),
    Tuple(Vec<Type>),
    NamedTuple(TupleOrigin, Vec<Type>),
    PartialFrame(Vec<Type>),
    Callable(Vec<Type>, Type),
    DeclaredCache(Type, usize),
    CachedCallable(CachedFunction),
    Awaitable(Type),
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct CachePolicy {
    /// Negative represents an unbounded cache; negative Python sizes normalize to zero.
    pub capacity: i64,
    pub typed: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TupleOrigin {
    Declaration(DefinitionId),
    StructSequence(&'static str, &'static [&'static str]),
}

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

#[derive(Default)]
struct Storage {
    shapes: Vec<Shape>,
    named_tuples: HashMap<DefinitionId, Rc<NamedTuple>>,
    cache_adapters: HashMap<usize, CachePolicy>,
    exceptions: Vec<NamedException>,
    ids: HashMap<Shape, usize>,
    hashable_records: HashSet<usize>,
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

#[cfg(any(debug_assertions, test, feature = "verify-ir"))]
impl Types {
    /// Checks an identity without using the unchecked shape accessors below.
    pub(crate) fn verify_type(&self, ty: Type, classes: usize) -> Result<(), String> {
        let storage = self.0.borrow();
        let valid = match ty {
            Type::StrLiteral(id) => {
                matches!(storage.shapes.get(id), Some(Shape::StringLiterals(_)))
            }
            Type::SimpleNamespace(id) => {
                matches!(storage.shapes.get(id), Some(Shape::Dict(Type::Str, _)))
            }
            Type::Cursor(id) => matches!(storage.shapes.get(id), Some(Shape::Cursor(_))),
            Type::List(id) => matches!(storage.shapes.get(id), Some(Shape::List(_))),
            Type::CacheState(id) => matches!(storage.shapes.get(id), Some(Shape::Dict(..))),
            Type::Dict(id) => matches!(
                storage.shapes.get(id),
                Some(Shape::Dict(..) | Shape::DefaultListDict(..) | Shape::Counter(_))
            ),
            Type::Set(id) => matches!(storage.shapes.get(id), Some(Shape::Set(_))),
            Type::Tuple(id) => matches!(
                storage.shapes.get(id),
                Some(Shape::Tuple(_) | Shape::NamedTuple(..) | Shape::PartialFrame(_))
            ),
            Type::Callable(id) => matches!(
                storage.shapes.get(id),
                Some(Shape::Callable(..) | Shape::DeclaredCache(..) | Shape::CachedCallable(_))
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
            | Type::StringKeys
            | Type::Iterator
            | Type::ByteOutput
            | Type::Exception
            | Type::ExceptionField
            | Type::ImportException
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::BinaryIO
            | Type::Path
            | Type::PurePath(_)
            | Type::CompletedProcess(_, None)
            | Type::EventLoop
            | Type::TaskGroup
            | Type::GeneratorContext
            | Type::Semaphore
            | Type::Document(_)
            | Type::LogFormatter
            | Type::LogRecord
            | Type::Logger
            | Type::TimedFileHandler
            | Type::ClockTime
            | Type::StrDescriptor(_)
            | Type::PlainObject
            | Type::PathInfo
            | Type::Sha256
            | Type::ZipFile
            | Type::ZipInfo
            | Type::CsvReader
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::JsonRoot
            | Type::JsonValue
            | Type::JsonObject
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
                Shape::Cursor(ty)
                | Shape::List(ty)
                | Shape::Set(ty)
                | Shape::Counter(ty)
                | Shape::Awaitable(ty)
                | Shape::DirectoryContext(ty)
                | Shape::ProcessArgs(ty)
                | Shape::Optional(ty) => vec![*ty],
                Shape::Dict(key, value) | Shape::DefaultListDict(key, value) => vec![*key, *value],
                Shape::Tuple(fields)
                | Shape::NamedTuple(_, fields)
                | Shape::PartialFrame(fields)
                | Shape::Union(fields) => fields.clone(),
                Shape::DeclaredCache(callable, _) => vec![*callable],
                Shape::CachedCallable(c) => vec![c.callable, c.frame, c.table],
                Shape::Callable(params, returns) => {
                    params.iter().copied().chain([*returns]).collect()
                }
            };
            for child in children {
                self.verify_type(child, classes)?;
                if let Type::StrLiteral(child_id)
                | Type::Cursor(child_id)
                | Type::List(child_id)
                | Type::CacheState(child_id)
                | Type::Dict(child_id)
                | Type::Set(child_id)
                | Type::Tuple(child_id)
                | Type::Callable(child_id)
                | Type::Optional(child_id)
                | Type::Coroutine(child_id)
                | Type::Task(child_id)
                | Type::SimpleNamespace(child_id)
                | Type::DirectoryContext(child_id)
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
                | Type::Path
                | Type::PurePath(_)
        ) || matches!(ty, Type::Tuple(_) | Type::Class(_)) && self.immutable_key_field(ty)
    }

    pub(crate) fn immutable_key_field(&self, ty: Type) -> bool {
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
            | Type::None => true,
            Type::Class(id) => self.0.borrow().hashable_records.contains(&id),
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
        if left == right || self.payload(left) == Some(right) {
            Some(left)
        } else if self.payload(right) == Some(left) {
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
                if matches!(other, Type::Bytes | Type::Path | Type::Enum(_))
                    || self.element(*other) == Some(Type::Str) =>
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
            Type::Tuple(_) => Some(!self.fields(ty).unwrap().is_empty()),
            Type::Class(id) if self.0.borrow().custom_truth.contains(&id) => None,
            Type::SimpleNamespace(_) => Some(true),
            Type::Class(_)
            | Type::Callable(_)
            | Type::Enum(_)
            | Type::Iterator
            | Type::Cursor(_)
            | Type::CacheState(_)
            | Type::ByteOutput
            | Type::Exception
            | Type::ImportException
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::Path
            | Type::PurePath(_)
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
            | Type::AsyncProcess(_)
            | Type::LogFormatter
            | Type::LogRecord
            | Type::Logger
            | Type::TimedFileHandler
            | Type::ClockTime
            | Type::StrDescriptor(_)
            | Type::PlainObject
            | Type::PathInfo
            | Type::Sha256
            | Type::ZipFile
            | Type::ZipInfo
            | Type::CsvReader
            | Type::RegexPattern
            | Type::RegexMatch
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

    pub(crate) fn tuple(&self, elements: Vec<Type>) -> Type {
        Type::Tuple(self.intern(Shape::Tuple(elements)))
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

    pub(crate) fn namedtuple_layout(&self, definition: DefinitionId, fields: Vec<Type>) -> Type {
        Type::Tuple(self.intern(Shape::NamedTuple(
            TupleOrigin::Declaration(definition),
            fields,
        )))
    }

    /// A native struct sequence shares the tuple descriptor but has no Python declaration.
    pub(crate) fn struct_sequence(
        &self,
        name: &'static str,
        names: &'static [&'static str],
        fields: Vec<Type>,
    ) -> Type {
        assert_eq!(names.len(), fields.len());
        Type::Tuple(self.intern(Shape::NamedTuple(
            TupleOrigin::StructSequence(name, names),
            fields,
        )))
    }

    pub(crate) fn tuple_metadata(&self, ty: Type) -> Option<Rc<NamedTuple>> {
        let Type::Tuple(id) = ty else { return None };
        let storage = self.0.borrow();
        match &storage.shapes[id] {
            Shape::NamedTuple(TupleOrigin::Declaration(id), _) => {
                storage.named_tuples.get(id).cloned()
            }
            Shape::NamedTuple(TupleOrigin::StructSequence(name, fields), _) => {
                Some(Rc::new(NamedTuple {
                    name: name.to_string(),
                    fields: fields.iter().map(|v| v.to_string()).collect(),
                }))
            }
            _ => None,
        }
    }

    /// Slicing a struct sequence materializes a plain tuple of its visible fields.
    pub(crate) fn struct_sequence_fields(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Tuple(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::NamedTuple(TupleOrigin::StructSequence(..), fields) => Some(fields.clone()),
            _ => None,
        }
    }

    pub(crate) fn namedtuple_definition(&self, ty: Type) -> Option<DefinitionId> {
        let Type::Tuple(id) = ty else {
            return None;
        };
        match self.0.borrow().shapes[id] {
            Shape::NamedTuple(TupleOrigin::Declaration(definition), _) => Some(definition),
            _ => None,
        }
    }

    /// Immutable private storage for the callable and arguments retained by partial.
    pub(crate) fn partial_frame(&self, fields: Vec<Type>) -> Type {
        Type::Tuple(self.intern(Shape::PartialFrame(fields)))
    }

    pub(crate) fn is_partial_frame(&self, ty: Type) -> bool {
        matches!(ty, Type::Tuple(id) if matches!(self.0.borrow().shapes[id], Shape::PartialFrame(_)))
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
    pub(crate) fn cache_signature(&self, ty: Type) -> Option<Type> {
        let shapes = self.0.borrow();
        let mut current = ty;
        while let Type::Callable(id) = current {
            current = match shapes.shapes.get(id)? {
                Shape::DeclaredCache(underlying, _) => *underlying,
                Shape::CachedCallable(cache) => cache.callable,
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

    pub(crate) fn signature(&self, ty: Type) -> Option<(Vec<Type>, Type)> {
        let Type::Callable(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::Callable(params, returns) => Some((params.clone(), *returns)),
            Shape::DeclaredCache(callable, _) => self.signature(*callable),
            Shape::CachedCallable(c) => self.signature(c.callable),
            _ => unreachable!("callable type identity has a callable shape"),
        }
    }

    pub(crate) fn dict(&self, key: Type, value: Type) -> Type {
        Type::Dict(self.intern(Shape::Dict(key, value)))
    }

    pub(crate) fn counter(&self, key: Type) -> Type {
        Type::Dict(self.intern(Shape::Counter(key)))
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

    pub(crate) fn fields(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Tuple(id) = ty else {
            return None;
        };
        match &self.0.borrow().shapes[id] {
            Shape::Tuple(fields) | Shape::NamedTuple(_, fields) | Shape::PartialFrame(fields) => {
                Some(fields.clone())
            }
            _ => unreachable!("tuple type identity has a tuple shape"),
        }
    }

    /// Enumerates concrete tuple layouts for native field access and destruction.
    pub(crate) fn tuples(&self) -> Vec<(Type, Vec<Type>)> {
        self.0
            .borrow()
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(id, shape)| match shape {
                Shape::Tuple(fields)
                | Shape::NamedTuple(_, fields)
                | Shape::PartialFrame(fields) => Some((Type::Tuple(id), fields.clone())),
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
            Shape::Dict(key, value) | Shape::DefaultListDict(key, value) => Some((key, value)),
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
                Type::Cursor(_)
                | Type::TextIO
                | Type::BinaryIO
                | Type::CsvReader
                | Type::ZipFile
                | Type::Popen(..)
                | Type::AsyncProcess(_)
                | Type::DirectoryContext(_)
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
        if let Type::SimpleNamespace(id) = ty {
            return vec![Type::Dict(id)];
        }
        if let Some(element) = self
            .element(ty)
            .or_else(|| self.payload(ty))
            .or_else(|| self.set_element(ty))
            .or_else(|| self.cursor_element(ty))
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
            | Type::Tuple(id)
            | Type::Callable(id)
            | Type::Dict(id)
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
            Shape::DeclaredCache(callable, _) => return self.write_type(*callable),
            Shape::CachedCallable(c) => return self.write_type(c.callable),
            Shape::Callable(params, returns) => {
                self.write_str("Callable[[")?;
                self.write_types(params, ", ")?;
                self.write_str("], ")?;
                self.write_type(*returns)?;
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
            Shape::Tuple(fields) | Shape::NamedTuple(_, fields) | Shape::PartialFrame(fields) => {
                self.write_str("tuple[")?;
                self.write_types(fields, ", ")?;
            }
            Shape::Counter(key) => {
                self.write_str("Counter[")?;
                self.write_type(*key)?;
            }
            Shape::Dict(key, value) | Shape::DefaultListDict(key, value) => {
                self.write_str(if matches!(shape, Shape::Dict(..)) {
                    "dict["
                } else {
                    "defaultdict["
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
