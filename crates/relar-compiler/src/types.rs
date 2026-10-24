//! Interns concrete container types while keeping IR type identities small and exact.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};

use crate::ir::Type;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Shape {
    StringLiterals(Vec<String>),
    List(Type),
    Dict(Type, Type),
    DefaultListDict(Type, Type),
    Counter(Type),
    Set(Type),
    Tuple(Vec<Type>),
    Callable(Vec<Type>, Type),
    Awaitable(Type),
    DirectoryContext(Type),
    Optional(Type),
    Union(Vec<Type>),
}

#[derive(Clone)]
pub(crate) struct NamedException {
    pub id: u32,
    pub name: String,
    pub base: u32,
    pub ancestors: Vec<u32>,
}

#[derive(Default)]
struct Storage {
    shapes: Vec<Shape>,
    exceptions: Vec<NamedException>,
    ids: HashMap<Shape, usize>,
    hashable_records: HashSet<usize>,
    custom_truth: HashSet<usize>,
    fallible_keys: HashSet<usize>,
}

/// An annotation and a matching inferred literal always receive the same identity.
/// Interior mutability lets independent function checkers share this compilation's
/// type universe without retaining references into its growable storage.
#[derive(Default)]
pub(crate) struct Types(RefCell<Storage>);

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

    pub(crate) fn set_custom_truth(&self, records: HashSet<usize>) {
        self.0.borrow_mut().custom_truth = records;
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
                | Type::Path
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
    /// remain invariant; only str and bytes may form a nonnullable sum today.
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
            [record @ Type::Class(id), Type::Exception]
            | [Type::Exception, record @ Type::Class(id)]
                if !self.0.borrow().custom_truth.contains(id) =>
            {
                Type::Union(self.intern(Shape::Union(vec![*record, Type::Exception])))
            }
            _ => return None,
        };
        Some(if nullable { self.optional(ty) } else { ty })
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
            Type::Class(_)
            | Type::Callable(_)
            | Type::Enum(_)
            | Type::Iterator
            | Type::Exception
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::Path
            | Type::CompletedProcess
            | Type::EventLoop
            | Type::Coroutine(_)
            | Type::Task(_)
            | Type::TaskGroup
            | Type::Document(_)
            | Type::GeneratorContext
            | Type::DirectoryContext(_)
            | Type::Semaphore
            | Type::AsyncProcess(_)
            | Type::Logger
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

    pub(crate) fn signature(&self, ty: Type) -> Option<(Vec<Type>, Type)> {
        let Type::Callable(id) = ty else { return None };
        match &self.0.borrow().shapes[id] {
            Shape::Callable(params, returns) => Some((params.clone(), *returns)),
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
            Shape::Tuple(fields) => Some(fields.clone()),
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
                Shape::Tuple(fields) => Some((Type::Tuple(id), fields.clone())),
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
            Shape::Dict(key, value) | Shape::DefaultListDict(key, value) => Some((key, value)),
            Shape::Counter(key) => Some((key, Type::Int)),
            _ => unreachable!("dictionary type identity has a dictionary shape"),
        }
    }

    /// Enumerates stored types for the class-field acyclicity check.
    pub(crate) fn children(&self, ty: Type) -> Vec<Type> {
        if let Some(element) = self
            .element(ty)
            .or_else(|| self.payload(ty))
            .or_else(|| self.set_element(ty))
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
        match ty {
            Type::StrLiteral(_) => {
                let values = self.literal_strings(ty).unwrap();
                let choices = values.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>();
                format!("Literal[{}]", choices.join(", "))
            }
            Type::Class(id) | Type::StrEnum(id) | Type::Enum(id) => class_name(id),
            Type::Callable(_) => {
                let (params, returns) = self.signature(ty).unwrap();
                format!(
                    "Callable[[{}], {}]",
                    params
                        .iter()
                        .map(|&ty| self.describe(ty, class_name))
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.describe(returns, class_name)
                )
            }
            Type::List(_) => format!(
                "list[{}]",
                self.describe(self.element(ty).unwrap(), class_name)
            ),
            Type::Tuple(_) => format!(
                "tuple[{}]",
                self.fields(ty)
                    .unwrap()
                    .into_iter()
                    .map(|ty| self.describe(ty, class_name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Dict(_) => {
                let (key, value) = self.mapping(ty).unwrap();
                if self.is_counter(ty) {
                    return format!("Counter[{}]", self.describe(key, class_name));
                }
                let name = if self.is_default_list_dict(ty) {
                    "defaultdict"
                } else {
                    "dict"
                };
                format!(
                    "{name}[{}, {}]",
                    self.describe(key, class_name),
                    self.describe(value, class_name)
                )
            }
            Type::Set(_) => format!(
                "set[{}]",
                self.describe(self.set_element(ty).unwrap(), class_name)
            ),
            Type::Optional(_) => format!(
                "{} | None",
                self.describe(self.payload(ty).unwrap(), class_name)
            ),
            Type::Union(_) => self
                .members(ty)
                .unwrap()
                .into_iter()
                .map(|ty| self.describe(ty, class_name))
                .collect::<Vec<_>>()
                .join(" | "),
            _ => ty.to_string(),
        }
    }
}
