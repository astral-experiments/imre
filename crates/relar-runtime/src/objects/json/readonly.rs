//! Immutable JSON documents for literal-key reads. A view owns its document,
//! so nested values survive the source array; typed scalar results own only
//! their native string or integer. No Python object graph or mutable alias is exposed.

use std::fmt;
use std::rc::Rc;

use serde::Deserializer;
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

use super::{Key, check_depth};
use crate::objects::exceptions::string_repr;
use crate::objects::words::Words;
use crate::objects::{Object, Storage, object};
use crate::{Error, Int, Kind, StorageCell, Str, finish};

mod values;
mod writer;

#[derive(Clone)]
enum Node {
    Null,
    Bool(bool),
    Integer(Int),
    Float(f64),
    String(Rc<Str>),
    Array(Vec<usize>),
    Object(Vec<(Str, usize)>),
}

impl Node {
    fn truth(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Array(values) => !values.is_empty(),
            Self::Integer(value) => value.as_small() != Some(0),
            Self::Float(value) => *value != 0.0,
            Self::String(value) => !value.is_empty(),
            Self::Object(fields) => !fields.is_empty(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Integer(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "str",
            Self::Array(_) => "list",
            Self::Object(_) => "dict",
        }
    }

    fn lookup(&self, key: &str) -> Option<usize> {
        let Self::Object(fields) = self else {
            return None;
        };
        fields
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, index)| *index)
    }

    fn field(&self, key: &str) -> Result<usize, Error> {
        match self {
            Self::Object(_) => self
                .lookup(key)
                .ok_or_else(|| Error::new(Kind::KeyError, string_repr(key))),
            Self::Array(_) => Err(Error::new(
                Kind::TypeError,
                "list indices must be integers or slices, not str",
            )),
            Self::String(_) => Err(Error::new(
                Kind::TypeError,
                "string indices must be integers, not 'str'",
            )),
            _ => Err(Error::new(
                Kind::TypeError,
                format!("'{}' object is not subscriptable", self.kind()),
            )),
        }
    }
}

#[derive(Clone)]
enum Nodes {
    Document(Rc<Vec<Node>>),
    /// Computed scalars and missing defaults do not need a second allocation.
    Single(Node),
}

impl std::ops::Deref for Nodes {
    type Target = [Node];
    fn deref(&self) -> &[Node] {
        match self {
            Self::Document(nodes) => nodes,
            Self::Single(node) => std::slice::from_ref(node),
        }
    }
}

pub(in crate::objects) struct View {
    nodes: Nodes,
    index: usize,
}

impl View {
    fn single(node: Node) -> Self {
        Self {
            nodes: Nodes::Single(node),
            index: 0,
        }
    }

    fn handle(nodes: &Nodes, index: usize) -> *const Object {
        Self {
            nodes: nodes.clone(),
            index,
        }
        .into_handle()
    }

    fn into_handle(self) -> *const Object {
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Json(self)))))
    }

    fn get_receiver(&self) -> Result<&Node, Error> {
        let node = &self.nodes[self.index];
        if matches!(node, Node::Object(_)) {
            Ok(node)
        } else {
            Err(Error::new(
                Kind::AttributeError,
                format!("'{}' object has no attribute 'get'", node.kind()),
            ))
        }
    }

    fn get_index(&self, key: &str) -> Result<Option<usize>, Error> {
        Ok(self.get_receiver()?.lookup(key))
    }

    /// Preserve nested array elements and object lookups when concatenating
    /// values borrowed from two independently owned documents.
    fn copy_to(&self, index: usize, destination: &mut Vec<Node>) -> usize {
        let node = match &self.nodes[index] {
            Node::Array(items) => Node::Array(
                items
                    .iter()
                    .map(|&i| self.copy_to(i, destination))
                    .collect(),
            ),
            Node::Object(fields) => Node::Object(
                fields
                    .iter()
                    .map(|(key, i)| (key.clone(), self.copy_to(*i, destination)))
                    .collect(),
            ),
            scalar => scalar.clone(),
        };
        let index = destination.len();
        destination.push(node);
        index
    }

    fn add(&self, other: &Self) -> Result<Self, Error> {
        let left = &self.nodes[self.index];
        let right = &other.nodes[other.index];
        let integer = |node: &Node| match node {
            Node::Bool(v) => Int::small(i64::from(*v)),
            Node::Integer(v) => Some(v.clone()),
            _ => None,
        };
        let float = |node: &Node| match node {
            Node::Float(v) => Some(Ok(*v)),
            _ => integer(node).map(|v| crate::floats::int_to_float(&v)),
        };
        let node = if let (Some(a), Some(b)) = (integer(left), integer(right)) {
            // SAFETY: both owned integer handles are live. Addition returns an owned integer.
            Node::Integer(Int(unsafe { crate::relar_int_binary(a.0, b.0, 0) }))
        } else if let (Some(a), Some(b)) = (float(left), float(right)) {
            Node::Float(a? + b?)
        } else if let (Node::String(a), Node::String(b)) = (left, right) {
            let mut s = a.as_ref().clone();
            s.push_str(b);
            Node::String(Rc::new(s))
        } else if let (Node::Array(a), Node::Array(b)) = (left, right) {
            let mut nodes = Vec::new();
            let mut items = Vec::with_capacity(a.len() + b.len());
            items.extend(a.iter().map(|&i| self.copy_to(i, &mut nodes)));
            items.extend(b.iter().map(|&i| other.copy_to(i, &mut nodes)));
            let index = nodes.len();
            nodes.push(Node::Array(items));
            return Ok(Self {
                nodes: Nodes::Document(Rc::new(nodes)),
                index,
            });
        } else {
            let message = match left {
                Node::String(_) => {
                    format!("can only concatenate str (not \"{}\") to str", right.kind())
                }
                Node::Array(_) => format!(
                    "can only concatenate list (not \"{}\") to list",
                    right.kind()
                ),
                _ => format!(
                    "unsupported operand type(s) for +: '{}' and '{}'",
                    left.kind(),
                    right.kind()
                ),
            };
            return Err(Error::new(Kind::TypeError, message));
        };
        Ok(Self::single(node))
    }

    fn select(&self, mut path: &[u8], mode: u32, literal: Option<&Str>) -> Result<usize, Error> {
        let mut index = self.index;
        while !path.is_empty() {
            let (size, tail) = path.split_at(4);
            let (key, tail) = tail.split_at(u32::from_le_bytes(size.try_into().unwrap()) as usize);
            // SAFETY: the compiler encoded a UTF-8 string literal and its exact length.
            index = self.nodes[index].field(unsafe { std::str::from_utf8_unchecked(key) })?;
            path = tail;
        }
        self.project(index, mode, literal)
    }

    fn project(&self, index: usize, mode: u32, literal: Option<&Str>) -> Result<usize, Error> {
        let node = &self.nodes[index];
        if mode & 256 != 0 && matches!(node, Node::Null) {
            return Ok(0);
        }
        let mode = mode & !256;
        match (mode, node) {
            (0, _) | (1, Node::Object(_)) => Ok(Self::handle(&self.nodes, index) as usize),
            (2, Node::String(text)) => Ok(Rc::into_raw(Rc::clone(text)) as usize),
            (3, Node::Integer(integer)) => Ok(integer.clone().into_raw()),
            (4, _) => Ok(usize::from(
                matches!(node, Node::String(text) if Some(text.as_ref()) == literal),
            )),
            (5, Node::Array(children)) => {
                let mut result = Words::new(5);
                result.values.reserve(children.len());
                for &child in children {
                    // A JSON row keeps its actual kind until an operation (such
                    // as a subscript or .get) needs it. Unused rows do not fail.
                    result
                        .values
                        .push(Self::handle(&self.nodes, child) as usize);
                }
                Ok(Rc::into_raw(result.into_object()) as usize)
            }
            (1..=3 | 5, _) => {
                let expected = ["", "dict", "str", "int", "", "list"][mode as usize];
                Err(Error::new(
                    Kind::TypeError,
                    format!("expected {expected}, got {} from JSON", node.kind()),
                ))
            }
            _ => unreachable!("checked JSON selection mode"),
        }
    }
}

/// Raw number tokens preserve the spelling and precision which a binary64 DOM
/// would discard.
fn push_node(nodes: &mut Vec<Node>, raw: &RawValue) -> Result<usize, serde_json::Error> {
    let raw = raw.get();
    let node = match raw.as_bytes()[0] {
        b'{' => {
            Node::Object(serde_json::Deserializer::from_str(raw).deserialize_map(Fields(nodes))?)
        }
        b'[' => {
            let items: Vec<&RawValue> = serde_json::from_str(raw)?;
            Node::Array(
                items
                    .into_iter()
                    .map(|item| push_node(nodes, item))
                    .collect::<Result<_, _>>()?,
            )
        }
        b'"' => Node::String(Rc::new(Str::from(serde_json::from_str::<String>(raw)?))),
        b't' => Node::Bool(true),
        b'f' => Node::Bool(false),
        b'n' => Node::Null,
        _ if raw.contains(['.', 'e', 'E']) => {
            Node::Float(raw.parse().expect("validated JSON float"))
        }
        _ => Node::Integer(
            raw.parse::<i64>()
                .ok()
                .and_then(Int::small)
                .unwrap_or_else(|| {
                    Int::new(
                        num_bigint::BigInt::parse_bytes(raw.as_bytes(), 10)
                            .expect("validated JSON integer"),
                    )
                }),
        ),
    };
    let index = nodes.len();
    nodes.push(node);
    Ok(index)
}

struct Fields<'a>(&'a mut Vec<Node>);
impl<'de> Visitor<'de> for Fields<'_> {
    type Value = Vec<(Str, usize)>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut fields = Vec::new();
        while let Some(key) = map.next_key_seed(Key)? {
            let raw = map.next_value::<&RawValue>()?;
            let index = push_node(self.0, raw).map_err(A::Error::custom)?;
            fields.push((Str::new(key.as_ref()), index));
        }
        Ok(fields)
    }
}

fn loads(text: &str) -> Result<*const Object, Error> {
    let syntax = |error| super::syntax::decode_error(text, error);
    check_depth(text).map_err(|message| Error::new(Kind::ValueError, message))?;
    if !text
        .trim_start_matches([' ', '\n', '\r', '\t'])
        .starts_with('[')
    {
        // Syntax errors precede the array-root contract.
        serde_json::from_str::<&RawValue>(text).map_err(syntax)?;
        return Err(Error::new(
            Kind::TypeError,
            "json.loads requires an array root",
        ));
    }
    let raw: Vec<&RawValue> = serde_json::from_str(text).map_err(syntax)?;
    let mut nodes = Vec::new();
    let roots = raw
        .into_iter()
        .map(|item| push_node(&mut nodes, item))
        .collect::<Result<Vec<_>, _>>()
        .map_err(syntax)?;
    let nodes = Nodes::Document(Rc::new(nodes));
    let mut result = Words::new(5);
    result.values.extend(
        roots
            .into_iter()
            .map(|index| View::handle(&nodes, index) as usize),
    );
    Ok(Rc::into_raw(result.into_object()))
}

/// Visit each token once in an ordinary document. Serde can represent an integer
/// beyond u64 as a float, so a float callback requests exact raw-number decoding.
struct Document {
    nodes: Vec<Node>,
    exact_numbers: bool,
}

impl<'de> DeserializeSeed<'de> for &mut Document {
    type Value = usize;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        let node = d.deserialize_any(&mut *self)?;
        let index = self.nodes.len();
        self.nodes.push(node);
        Ok(index)
    }
}

impl<'de> Visitor<'de> for &mut Document {
    type Value = Node;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_bool<E>(self, value: bool) -> Result<Node, E> {
        Ok(Node::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Node, E> {
        Ok(Node::Integer(
            Int::small(value).unwrap_or_else(|| Int::new(value.into())),
        ))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Node, E> {
        Ok(Node::Integer(
            i64::try_from(value)
                .ok()
                .and_then(Int::small)
                .unwrap_or_else(|| Int::new(value.into())),
        ))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Node, E> {
        self.exact_numbers = false;
        Ok(Node::Float(value))
    }
    fn visit_str<E>(self, value: &str) -> Result<Node, E> {
        Ok(Node::String(Rc::new(Str::new(value))))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(index) = sequence.next_element_seed(&mut *self)? {
            items.push(index);
        }
        Ok(Node::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut fields = Vec::new();
        while let Some(key) = map.next_key_seed(Key)? {
            fields.push((Str::new(key.as_ref()), map.next_value_seed(&mut *self)?));
        }
        Ok(Node::Object(fields))
    }
}

fn load(text: &str) -> Result<*const Object, Error> {
    check_depth(text).map_err(|message| Error::new(Kind::ValueError, message))?;
    let syntax = |error| super::syntax::decode_error(text, error);
    let mut d = serde_json::Deserializer::from_str(text);
    d.disable_recursion_limit(); // check_depth already validated the maximum.
    let mut document = Document {
        nodes: Vec::new(),
        exact_numbers: true,
    };
    if let Ok(index) = (&mut document).deserialize(&mut d)
        && document.exact_numbers
        && d.end().is_ok()
    {
        return Ok(View {
            nodes: Nodes::Document(Rc::new(document.nodes)),
            index,
        }
        .into_handle());
    }
    // A wide integer may also make Serde report a float range error. Raw JSON
    // tokens preserve both such integers and Python's overflow-to-infinity floats.
    drop(document);
    let raw = serde_json::from_str::<&RawValue>(text).map_err(syntax)?;
    let mut nodes = Vec::new();
    let index = push_node(&mut nodes, raw).map_err(syntax)?;
    Ok(View {
        nodes: Nodes::Document(Rc::new(nodes)),
        index,
    }
    .into_handle())
}

/// # Safety
/// The caller roots the UTF-8 text read from the file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_load(text: *const Str) -> *const Object {
    // SAFETY: text remains rooted through parsing.
    finish(load(unsafe { &*text }), std::ptr::null())
}

/// # Safety
/// The caller roots a JSON view. All storage references must remain inside
/// the closure, which must not mutate or invoke user code.
unsafe fn with_view<T>(value: *const Object, run: impl FnOnce(&View) -> T) -> T {
    // SAFETY: the compiler only passes live immutable JSON owners.
    unsafe {
        object(value).0.with(|storage| match storage {
            Storage::Json(view) => run(view),
            _ => unreachable!("typed JSON owner"),
        })
    }
}

/// # Safety
/// The caller roots a JSON view throughout attribute resolution. Success returns
/// the same receiver with an independently owned reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_get_receiver(value: *const Object) -> *const Object {
    // SAFETY: the read-only borrow ends before the independently owned result escapes.
    let result = unsafe {
        with_view(value, |v| {
            v.get_receiver()?;
            Rc::increment_strong_count(value);
            Ok(value)
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the receiver, any nonzero integer default, and a UTF-8 key
/// of exactly `len` bytes. A zero default represents a fresh empty list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_get(
    value: *const Object,
    key: *const u8,
    len: usize,
    mode: u32,
    fallback: usize,
) -> usize {
    // SAFETY: the compiler emits a static string key and validated result mode.
    let result = unsafe {
        with_view(value, |view| {
            let key = std::str::from_utf8_unchecked(std::slice::from_raw_parts(key, len));
            if let Some(index) = view.get_index(key)? {
                return view.project(index, mode, None);
            }
            let default = if fallback == 0 {
                Node::Array(Vec::new())
            } else {
                Node::Integer((*Int::borrow(fallback)).clone())
            };
            View::single(default).project(0, mode, None)
        })
    };
    finish(result, 0)
}

/// # Safety
/// The compiler supplies a rooted JSON owner, two immutable UTF-8 keys with
/// their exact lengths, and two rooted nonzero integer fallback words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_add_fields(
    value: *const Object,
    a: *const u8,
    a_len: usize,
    a_default: usize,
    b: *const u8,
    b_len: usize,
    b_default: usize,
) -> *const Object {
    // SAFETY: the immutable view and key/default arguments live for the whole sum.
    let result = unsafe {
        with_view(value, |view| {
            let get = |p, len, fallback| {
                let key = std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, len));
                Ok::<_, Error>(match view.get_index(key)? {
                    Some(index) => View {
                        nodes: view.nodes.clone(),
                        index,
                    },
                    None => View::single(Node::Integer((*Int::borrow(fallback)).clone())),
                })
            };
            get(a, a_len, a_default)?.add(&get(b, b_len, b_default)?)
        })
    };
    finish(result.map(View::into_handle), std::ptr::null())
}

/// # Safety
/// Both inputs are rooted immutable JSON views; the result transfers an owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_add(a: *const Object, b: *const Object) -> *const Object {
    // SAFETY: both simultaneous storage borrows are immutable, including on alias.
    let result = unsafe { with_view(a, |left| with_view(b, |right| left.add(right))) };
    finish(result.map(View::into_handle), std::ptr::null())
}

/// # Safety
/// The caller roots a JSON view. Python bools also satisfy isinstance(value, int).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_is_int(value: *const Object) -> u32 {
    // SAFETY: only the scalar type test escapes the borrow.
    unsafe {
        with_view(value, |v| {
            u32::from(matches!(v.nodes[v.index], Node::Integer(_) | Node::Bool(_)))
        })
    }
}

/// # Safety
/// The caller roots the input string. Success transfers an ordinary list owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_loads(text: *const Str) -> *const Object {
    // SAFETY: text is a live immutable string for the entire parse.
    finish(loads(unsafe { &*text }), std::ptr::null())
}

/// Creates one separate read-only dictionary for a dataclass default factory.
#[unsafe(no_mangle)]
pub extern "C" fn relar_json_empty_object() -> *const Object {
    View::single(Node::Object(Vec::new())).into_handle()
}

/// # Safety
/// The caller roots the JSON handle and any comparison string. The compiler
/// supplies one complete immutable length-prefixed path and the exact result mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_select(
    value: *const Object,
    path: *const u8,
    len: usize,
    mode: u32,
    literal: *const Str,
) -> usize {
    // SAFETY: all borrows end before an error is published; selected owners are independent.
    let result = unsafe {
        object(value).0.with(|storage| {
            let Storage::Json(view) = storage else {
                unreachable!("typed JSON owner")
            };
            view.select(
                std::slice::from_raw_parts(path, len),
                mode,
                literal.as_ref(),
            )
        })
    };
    finish(result, 0)
}

/// # Safety
/// The caller roots a JSON view for the nonreentrant truth test.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_truth(value: *const Object) -> u32 {
    // SAFETY: only a scalar result escapes the borrowed storage.
    unsafe {
        object(value).0.with(|storage| {
            let Storage::Json(view) = storage else {
                unreachable!("typed JSON owner")
            };
            u32::from(view.nodes[view.index].truth())
        })
    }
}

/// # Safety
/// The caller roots a JSON view for the nonreentrant identity test.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_null(value: *const Object) -> u32 {
    // SAFETY: only a scalar result escapes the borrowed storage.
    unsafe {
        object(value).0.with(|storage| {
            let Storage::Json(view) = storage else {
                unreachable!("typed JSON owner")
            };
            u32::from(matches!(view.nodes[view.index], Node::Null))
        })
    }
}
