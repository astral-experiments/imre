//! Interns concrete container types while keeping IR type identities small and exact.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::ir::Type;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Shape {
    List(Type),
    Dict(Type, Type),
    Set(Type),
    Tuple(Vec<Type>),
    Callable(Vec<Type>, Type),
    Optional(Type),
    Union(Vec<Type>),
}

#[derive(Default)]
struct Storage {
    shapes: Vec<Shape>,
    ids: HashMap<Shape, usize>,
}

/// An annotation and a matching inferred literal always receive the same identity.
/// Interior mutability lets independent function checkers share this compilation's
/// type universe without retaining references into its growable storage.
#[derive(Default)]
pub(crate) struct Types(RefCell<Storage>);

impl Types {
    /// Hash keys have immutable value semantics without user-defined protocols.
    /// Tuple fields may be nullable; mutable records and containers never enter
    /// a key, even when the surrounding tuple itself is immutable.
    pub(crate) fn hashable_key(&self, ty: Type) -> bool {
        matches!(ty, Type::Int | Type::Str | Type::Bytes)
            || matches!(ty, Type::Tuple(_)) && self.immutable_key_field(ty)
    }

    fn immutable_key_field(&self, ty: Type) -> bool {
        if let Some(payload) = self.payload(ty) {
            return self.immutable_key_field(payload);
        }
        match ty {
            Type::Int | Type::Bool | Type::Str | Type::Bytes | Type::None => true,
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
            Type::Int | Type::Bool | Type::Str | Type::Bytes => true,
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
            [a, b] if matches!((a, b), (Type::Str, Type::Bytes) | (Type::Bytes, Type::Str)) => {
                Type::Union(self.intern(Shape::Union(vec![Type::Str, Type::Bytes])))
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
            Type::Class(_)
            | Type::Callable(_)
            | Type::Iterator
            | Type::Exception
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::Path
            | Type::CompletedProcess
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
            Shape::Dict(key, value) => Some((key, value)),
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
            Type::Class(id) => class_name(id),
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
                format!(
                    "dict[{}, {}]",
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
