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
    Optional(Type),
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
            self.fields(ty).unwrap_or_default()
        }
    }

    pub(crate) fn describe(&self, ty: Type, class_name: &impl Fn(usize) -> String) -> String {
        match ty {
            Type::Class(id) => class_name(id),
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
            _ => ty.to_string(),
        }
    }
}
