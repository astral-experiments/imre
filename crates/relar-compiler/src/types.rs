//! Interns concrete container types while keeping IR type identities small and exact.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write};

use crate::ir::Type;

const MAX_DESCRIPTION_BYTES: usize = 1024;

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
            Type::List(id) => matches!(storage.shapes.get(id), Some(Shape::List(_))),
            Type::Dict(id) => matches!(
                storage.shapes.get(id),
                Some(Shape::Dict(..) | Shape::DefaultListDict(..) | Shape::Counter(_))
            ),
            Type::Set(id) => matches!(storage.shapes.get(id), Some(Shape::Set(_))),
            Type::Tuple(id) => matches!(storage.shapes.get(id), Some(Shape::Tuple(_))),
            Type::Callable(id) => matches!(storage.shapes.get(id), Some(Shape::Callable(..))),
            Type::Optional(id) => matches!(storage.shapes.get(id), Some(Shape::Optional(_))),
            Type::Union(id) => matches!(storage.shapes.get(id), Some(Shape::Union(_))),
            Type::Class(id) | Type::StrEnum(id) | Type::Enum(id) => id < classes,
            Type::Coroutine(id) | Type::Task(id) => {
                matches!(storage.shapes.get(id), Some(Shape::Awaitable(_)))
            }
            Type::DirectoryContext(id) => {
                matches!(storage.shapes.get(id), Some(Shape::DirectoryContext(_)))
            }
            Type::AsyncProcess(bits) => bits <= 3,
            Type::Int
            | Type::Float
            | Type::Bool
            | Type::Str
            | Type::Bytes
            | Type::ListInt
            | Type::StringKeys
            | Type::Iterator
            | Type::ByteOutput
            | Type::Exception
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::BinaryIO
            | Type::Path
            | Type::CompletedProcess
            | Type::EventLoop
            | Type::TaskGroup
            | Type::GeneratorContext
            | Type::Semaphore
            | Type::Document(_)
            | Type::Logger
            | Type::RegexPattern
            | Type::RegexMatch
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
                Shape::List(ty)
                | Shape::Set(ty)
                | Shape::Counter(ty)
                | Shape::Awaitable(ty)
                | Shape::DirectoryContext(ty)
                | Shape::Optional(ty) => vec![*ty],
                Shape::Dict(key, value) | Shape::DefaultListDict(key, value) => vec![*key, *value],
                Shape::Tuple(fields) | Shape::Union(fields) => fields.clone(),
                Shape::Callable(params, returns) => {
                    params.iter().copied().chain([*returns]).collect()
                }
            };
            for child in children {
                self.verify_type(child, classes)?;
                if let Type::StrLiteral(child_id)
                | Type::List(child_id)
                | Type::Dict(child_id)
                | Type::Set(child_id)
                | Type::Tuple(child_id)
                | Type::Callable(child_id)
                | Type::Optional(child_id)
                | Type::Coroutine(child_id)
                | Type::Task(child_id)
                | Type::DirectoryContext(child_id)
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

    pub(crate) fn set_custom_truth(&self, records: HashSet<usize>) {
        self.0.borrow_mut().custom_truth = records;
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
            | Type::ByteOutput
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
                Type::TextIO
                | Type::BinaryIO
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
                    if let Some(subclasses) = self.0.borrow().concrete_subclasses.get(&class) {
                        pending.extend_from_slice(subclasses);
                    }
                }
                _ => pending.extend(self.children(ty)),
            }
        }
        false
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
            | Type::Tuple(id)
            | Type::Callable(id)
            | Type::Dict(id)
            | Type::Set(id)
            | Type::Optional(id)
            | Type::Union(id) => id,
            Type::Class(id) | Type::StrEnum(id) | Type::Enum(id) => {
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
            Shape::Callable(params, returns) => {
                self.write_str("Callable[[")?;
                self.write_types(params, ", ")?;
                self.write_str("], ")?;
                self.write_type(*returns)?;
            }
            Shape::List(element) | Shape::Set(element) => {
                self.write_str(if matches!(shape, Shape::List(_)) {
                    "list["
                } else {
                    "set["
                })?;
                self.write_type(*element)?;
            }
            Shape::Tuple(fields) => {
                self.write_str("tuple[")?;
                self.write_types(fields, ", ")?;
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
            Shape::Counter(key) => {
                self.write_str("Counter[")?;
                self.write_type(*key)?;
            }
            Shape::DirectoryContext(_) | Shape::Awaitable(_) => {
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
