//! Immutable JSON documents for literal-key reads. A view owns its document,
//! so nested values survive the source array; typed scalar results own only
//! their native string or integer. No Python object graph or mutable alias is exposed.

mod arrays;
mod constants;
pub(in crate::objects) mod document;
mod elements;
mod keys;
mod values;
mod writer;

use std::fmt;
use std::rc::Rc;

use serde::Deserializer;
use serde::de::{Error as _, MapAccess, Visitor};
use serde_json::value::RawValue;
use smallvec::SmallVec;

use super::{Key, check_depth};
use crate::objects::words::Words;
use crate::objects::{Object, Storage, object};
use crate::{Error, Int, Kind, StorageCell, Str, finish};
use keys::{KeyMemo, ObjectFields};

#[derive(Clone)]
pub(super) enum Node {
    Null,
    Bool(bool),
    Integer(Int),
    Float(f64),
    /// The default decoder reuses a single NaN object, observable in container equality.
    ConstantNan,
    String(Rc<Str>),
    Datetime(Str),
    Array(Vec<usize>),
    Deferred(Rc<arrays::Deferred>),
    Object(ObjectFields),
}

impl Node {
    fn truth(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Datetime(_) | Self::ConstantNan => true,
            Self::Bool(value) => *value,
            Self::Array(items) => !items.is_empty(),
            Self::Deferred(array) => array.truth(),
            Self::Integer(value) => value.as_small() != Some(0),
            Self::Float(value) => *value != 0.0,
            Self::String(value) => !value.is_empty(),
            Self::Object(fields) => !fields.is_empty(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Datetime(_) => "datetime",
            Self::Bool(_) => "bool",
            Self::Integer(_) => "int",
            Self::Float(_) | Self::ConstantNan => "float",
            Self::String(_) => "str",
            Self::Array(_) | Self::Deferred(_) => "list",
            Self::Object(_) => "dict",
        }
    }

    pub(super) fn lookup(&self, key: &str) -> Option<usize> {
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
                .ok_or_else(|| Error::string_key(Str::new(key))),
            Self::Array(_) | Self::Deferred(_) => Err(Error::new(
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
pub(super) enum Nodes {
    Document(Rc<Vec<Node>>),
    /// Keep aliases of a mutable root array without enlarging every scalar view.
    Root(Rc<Vec<Node>>, Rc<Object>),
    /// Computed scalars and missing defaults do not need a second allocation.
    Single(Node),
}

impl std::ops::Deref for Nodes {
    type Target = [Node];
    fn deref(&self) -> &[Node] {
        match self {
            Self::Document(nodes) | Self::Root(nodes, _) => nodes,
            Self::Single(node) => std::slice::from_ref(node),
        }
    }
}

impl Nodes {
    pub(super) fn root_items(&self) -> Option<&Rc<Object>> {
        match self {
            Self::Root(_, items) => Some(items),
            _ => None,
        }
    }

    fn child(&self) -> Self {
        match self {
            Self::Root(nodes, _) => Self::Document(Rc::clone(nodes)),
            _ => self.clone(),
        }
    }
}

pub(in crate::objects) struct View {
    pub(super) nodes: Nodes,
    pub(super) index: usize,
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
            nodes: nodes.child(),
            index,
        }
        .into_handle()
    }

    fn into_handle(self) -> *const Object {
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Json(self)))))
    }

    fn resolved(&self, index: usize) -> (&Self, usize) {
        match &self.nodes[index] {
            Node::Deferred(array) => {
                let view = array.view();
                (view, view.index)
            }
            _ => (self, index),
        }
    }

    fn expanded(&self) -> &Self {
        self.resolved(self.index).0
    }

    /// Snapshot the current elements when an array projected to a native list
    /// is used as an immutable value again. Its aliases may have mutated it.
    fn snapshot_root(&self) -> Option<Self> {
        self.nodes.root_items()?;
        let mut nodes = Vec::new();
        let index = self.copy_to(self.index, &mut nodes);
        Some(Self {
            nodes: Nodes::Document(Rc::new(nodes)),
            index,
        })
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
        let node = match (&self.nodes[index], self.nodes.root_items()) {
            (Node::Array(_), Some(items)) if index == self.index => {
                // SAFETY: the list roots all immutable JSON views while their children are copied.
                Node::Array(unsafe {
                    items.0.with(|storage| {
                        storage
                            .sequence()
                            .values
                            .iter()
                            .map(|&word| {
                                with_view(word as *const Object, |child| {
                                    child.copy_to(child.index, destination)
                                })
                            })
                            .collect()
                    })
                })
            }
            (Node::Array(items), _) => Node::Array(
                items
                    .iter()
                    .map(|&i| self.copy_to(i, destination))
                    .collect(),
            ),
            (Node::Object(fields), _) => Node::Object(
                fields.with_entries(
                    fields
                        .iter()
                        .map(|(key, i)| (key.clone(), self.copy_to(*i, destination)))
                        .collect(),
                ),
            ),
            (scalar, _) => scalar.clone(),
        };
        let index = destination.len();
        destination.push(node);
        index
    }

    fn add(&self, other: &Self) -> Result<Self, Error> {
        if let Some(root) = self.snapshot_root() {
            return root.add(other);
        }
        if let Some(root) = other.snapshot_root() {
            return self.add(&root);
        }
        if let Node::Deferred(array) = &self.nodes[self.index] {
            return array.view().add(other);
        }
        if let Node::Deferred(array) = &other.nodes[other.index] {
            return self.add(array.view());
        }
        let left = &self.nodes[self.index];
        let right = &other.nodes[other.index];
        let integer = |node: &Node| match node {
            Node::Bool(v) => Int::small(i64::from(*v)),
            Node::Integer(v) => Some(v.clone()),
            _ => None,
        };
        let float = |node: &Node| match node {
            Node::Float(v) => Some(Ok(*v)),
            Node::ConstantNan => Some(Ok(f64::NAN)),
            _ => integer(node).map(|v| crate::floats::int_to_float(&v)),
        };
        let node = if let (Some(a), Some(b)) = (integer(left), integer(right)) {
            // SAFETY: both owned integer handles are live. Addition returns an owned integer.
            Node::Integer(Int(unsafe { crate::imre_int_binary(a.0, b.0, 0) }))
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

    fn array(&self, children: &[usize]) -> *const Object {
        if let Some(items) = self.nodes.root_items() {
            return Rc::into_raw(Rc::clone(items));
        }
        Self::children(&self.nodes, children)
    }

    fn children(nodes: &Nodes, children: &[usize]) -> *const Object {
        let mut result = Words::new(5);
        result
            .values
            .extend(children.iter().map(|&i| Self::handle(nodes, i) as usize));
        Rc::into_raw(result.into_object())
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
        let optional = mode & 256 != 0;
        let mode = mode & !256;
        match (mode, node) {
            (0, _) | (1, Node::Object(_)) if index == self.index => Ok(Self {
                nodes: self.nodes.clone(),
                index,
            }
            .into_handle()
                as usize),
            (0, _) | (1, Node::Object(_)) => Ok(Self::handle(&self.nodes, index) as usize),
            (2, Node::String(text)) => Ok(Rc::into_raw(Rc::clone(text)) as usize),
            (3, Node::Integer(integer)) => Ok(integer.clone().into_raw()),
            (4, _) => Ok(usize::from(
                matches!(node, Node::String(text) if Some(text.as_ref()) == literal),
            )),
            (5 | 6, Node::Deferred(array)) => {
                array.view().project(array.view().index, mode, literal)
            }
            (5 | 6, Node::Array(children)) => Ok(if index == self.index {
                self.array(children)
            } else {
                Self::children(&self.nodes, children)
            } as usize),
            (7, Node::Bool(value)) => Ok(usize::from(*value) + usize::from(optional)),
            (8, Node::Float(_) | Node::ConstantNan) => {
                let value = match node {
                    Node::Float(v) => *v,
                    _ => f64::NAN,
                };
                Ok(if optional {
                    Rc::into_raw(Rc::new(value)) as usize
                } else {
                    value.to_bits() as usize
                })
            }
            (1..=3 | 5..=8, _) => {
                let expected = [
                    "", "dict", "str", "int", "", "list", "list", "bool", "float",
                ][mode as usize];
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
/// would discard. Array children stay in the same immutable document arena.
#[derive(Clone, Copy)]
struct Source<'a> {
    text: &'a str,
    original: Option<(&'a str, &'a Rc<String>)>,
    shared: Option<(&'a Rc<str>, usize)>,
    keys: &'a KeyMemo,
}

impl Source<'_> {
    fn token<'a>(&'a self, raw: &'a str) -> &'a str {
        if let Some((original, _)) = self.original {
            let offset = raw.as_ptr() as usize - self.text.as_ptr() as usize;
            &original[offset..offset + raw.len()]
        } else {
            raw
        }
    }
}

fn push_node(
    nodes: &mut Vec<Node>,
    raw: &RawValue,
    source: Source<'_>,
) -> Result<usize, serde_json::Error> {
    let raw = raw.get();
    let node = match raw.as_bytes()[0] {
        b'{' => Node::Object(source.keys.fields(
            serde_json::Deserializer::from_str(raw).deserialize_map(Fields(nodes, source))?,
        )),
        b'[' => match arrays::Deferred::new(raw, source) {
            Some(array) => Node::Deferred(array),
            None => push_array(nodes, raw, source)?,
        },
        b'"' => Node::String(Rc::new(Str::from(serde_json::from_str::<String>(raw)?))),
        b't' => Node::Bool(true),
        b'f' => Node::Bool(false),
        b'n' => Node::Null,
        _ if raw.contains(['.', 'e', 'E']) => match source.token(raw) {
            "NaN" => Node::ConstantNan,
            "Infinity" => Node::Float(f64::INFINITY),
            "-Infinity" => Node::Float(f64::NEG_INFINITY),
            _ => Node::Float(raw.parse().expect("validated JSON float")),
        },
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

fn push_array(
    nodes: &mut Vec<Node>,
    raw: &str,
    source: Source<'_>,
) -> Result<Node, serde_json::Error> {
    let items: Vec<&RawValue> = serde_json::from_str(raw)?;
    Ok(Node::Array(
        items
            .into_iter()
            .map(|item| push_node(nodes, item, source))
            .collect::<Result<_, _>>()?,
    ))
}

struct Fields<'a, 'b>(&'a mut Vec<Node>, Source<'b>);
impl<'de> Visitor<'de> for Fields<'_, '_> {
    type Value = Vec<(Str, usize)>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut fields = Vec::new();
        while let Some(key) = map.next_key_seed(Key)? {
            let raw = map.next_value::<&RawValue>()?;
            let index = push_node(self.0, raw, self.1).map_err(A::Error::custom)?;
            fields.push((Str::new(key.as_ref()), index));
        }
        Ok(fields)
    }
}

/// Keep only borrowed spans until the complete object and its trailing input are valid.
/// Wider objects use whole-document validation before their fields are materialized.
struct RootSpans;

impl<'de> Visitor<'de> for RootSpans {
    type Value = SmallVec<[(&'de RawValue, &'de RawValue); 8]>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut spans = Self::Value::new();
        while let Some(key) = map.next_key()? {
            if spans.len() == spans.inline_size() {
                return Err(A::Error::custom("too many fields to index inline"));
            }
            spans.push((key, map.next_value()?));
        }
        Ok(spans)
    }
}

fn root_fields(source: Source<'_>, nodes: &mut Vec<Node>) -> Option<Vec<(Str, usize)>> {
    let mut parser = serde_json::Deserializer::from_str(source.text);
    let spans = parser.deserialize_map(RootSpans).ok()?;
    parser.end().ok()?;
    spans
        .into_iter()
        .map(|(key, raw)| {
            let key = serde_json::Deserializer::from_str(key.get())
                .deserialize_str(Key)
                .ok()?;
            Some((Str::new(key.as_ref()), push_node(nodes, raw, source).ok()?))
        })
        .collect()
}

fn build(source: Source<'_>) -> Result<*const Object, serde_json::Error> {
    let text = source.text;
    let trimmed = text.trim_start_matches([' ', '\n', '\r', '\t']);
    if !trimmed.starts_with('[') {
        let mut nodes = Vec::new();
        let index = if trimmed.starts_with('{')
            && let Some(fields) = root_fields(source, &mut nodes)
        {
            let index = nodes.len();
            nodes.push(Node::Object(source.keys.fields(fields)));
            index
        } else {
            // Preserve the original error location if a validated field cannot be decoded.
            nodes.clear();
            let raw = serde_json::from_str::<&RawValue>(text)?;
            push_node(&mut nodes, raw, source)?
        };
        return Ok(View {
            nodes: Nodes::Document(Rc::new(nodes)),
            index,
        }
        .into_handle());
    }
    let raw: Vec<&RawValue> = serde_json::from_str(text)?;
    let mut nodes = Vec::new();
    let roots = raw
        .into_iter()
        .map(|item| push_node(&mut nodes, item, source))
        .collect::<Result<Vec<_>, _>>()?;
    let index = nodes.len();
    nodes.push(Node::Array(Vec::new()));
    let nodes = Rc::new(nodes);
    let children = Nodes::Document(Rc::clone(&nodes));
    let mut result = Words::new(5);
    result.values.extend(
        roots
            .into_iter()
            .map(|index| View::handle(&children, index) as usize),
    );
    Ok(View {
        nodes: Nodes::Root(nodes, result.into_object()),
        index,
    }
    .into_handle())
}

pub(super) fn loads(
    text: &str,
    owner: impl FnOnce() -> Option<Rc<Str>>,
) -> Result<*const Object, Error> {
    check_depth(text).map_err(|message| Error::new(Kind::ValueError, message))?;
    let keys = KeyMemo::default();
    build(Source {
        text,
        original: None,
        shared: None,
        keys: &keys,
    })
    .or_else(|error| {
        if let Some(patched) = constants::normalize(text, &error)? {
            let patched = Rc::new(patched);
            build(Source {
                text: &patched,
                original: Some((text, &patched)),
                shared: None,
                keys: &keys,
            })
            .map_err(|error| super::syntax::decode_error(text, error, owner))
        } else {
            Err(super::syntax::decode_error(text, error, owner))
        }
    })
}

/// # Safety
/// The caller roots the input string. Success transfers a checked document owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_loads(text: *const Str) -> *const Object {
    // SAFETY: text is a live immutable string for the entire parse.
    let source = unsafe { &*text };
    // The str entry rejects its BOM before scanning. The binary entry already
    // consumed its encoding marker; a second one is ordinary invalid JSON.
    // SAFETY: the caller roots this immutable string while the error takes its own reference.
    let owner = || unsafe { crate::objects::acquire_rc(text as usize) };
    let result = if source.starts_with('\u{feff}') {
        Err(super::syntax::at_error(
            owner(),
            0,
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
        ))
    } else {
        loads(source, || Some(owner()))
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the JSON handle and any comparison string. The compiler
/// supplies one complete immutable length-prefixed path and the exact result mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_select(
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
pub unsafe extern "C" fn imre_json_truth(value: *const Object) -> u32 {
    // SAFETY: only a scalar result escapes the borrowed storage.
    unsafe {
        object(value).0.with(|storage| {
            let Storage::Json(view) = storage else {
                unreachable!("typed JSON owner")
            };
            u32::from(view.nodes.root_items().map_or_else(
                || view.nodes[view.index].truth(),
                |items| {
                    items
                        .0
                        .with(|storage| !storage.sequence().values.is_empty())
                },
            ))
        })
    }
}

/// # Safety
/// The caller roots a JSON view for the nonreentrant identity test.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_null(value: *const Object) -> u32 {
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
pub unsafe extern "C" fn imre_json_get_receiver(value: *const Object) -> *const Object {
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
/// Both inputs are rooted immutable JSON views; the result transfers an owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_add(a: *const Object, b: *const Object) -> *const Object {
    // SAFETY: both simultaneous storage borrows are immutable, including on alias.
    let result = unsafe { with_view(a, |left| with_view(b, |right| left.add(right))) };
    finish(result.map(View::into_handle), std::ptr::null())
}

/// # Safety
/// The compiler roots both JSON operands for the duration of the comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_equal(left: *const Object, right: *const Object) -> u32 {
    // SAFETY: both views admit simultaneous immutable borrows, including on alias.
    unsafe {
        with_view(left, |a| {
            with_view(right, |b| u32::from(a.equal_operand(b)))
        })
    }
}

/// # Safety
/// The caller roots a JSON view. Python bools also satisfy isinstance(value, int).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_is_int(value: *const Object) -> u32 {
    // SAFETY: only the scalar type test escapes the borrow.
    unsafe {
        with_view(value, |v| {
            u32::from(matches!(v.nodes[v.index], Node::Integer(_) | Node::Bool(_)))
        })
    }
}

/// # Safety
/// The compiler roots the checked receiver, static UTF-8 key, and tagged fallback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_get(
    value: *const Object,
    key: *const u8,
    len: usize,
    mode: u32,
    kind: u32,
    word: usize,
) -> usize {
    // SAFETY: the receiver and any fallback handle stay live throughout this borrow.
    let result = unsafe {
        with_view(value, |view| {
            let key = std::str::from_utf8_unchecked(std::slice::from_raw_parts(key, len));
            if let Some(index) = view.get_index(key)? {
                return view.project(index, mode, None);
            }
            let fallback = match kind {
                0 => Node::Null,
                1 => Node::Array(Vec::new()),
                2 => Node::Integer((*Int::borrow(word)).clone()),
                3 => {
                    let text = word as *const Str;
                    Rc::increment_strong_count(text);
                    Node::String(Rc::from_raw(text))
                }
                4 => Node::Bool(word != 0),
                5 => Node::Float(f64::from_bits(word as u64)),
                _ => unreachable!("checked JSON fallback type"),
            };
            View::single(fallback).project(0, mode, None)
        })
    };
    finish(result, 0)
}

/// # Safety
/// The compiler supplies a rooted JSON owner, two immutable UTF-8 keys with
/// their exact lengths, and two rooted nonzero integer fallback words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_add_fields(
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
                        nodes: view.nodes.child(),
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

/// Creates one separate read-only dictionary for a dataclass default factory.
#[unsafe(no_mangle)]
pub extern "C" fn imre_json_empty_object() -> *const Object {
    View::single(Node::Object(KeyMemo::default().fields(Vec::new()))).into_handle()
}

pub(super) fn keys(fields: &[(Str, usize)]) -> impl Iterator<Item = &Str> {
    let mut seen = std::collections::HashSet::new();
    fields
        .iter()
        .filter_map(move |(name, _)| seen.insert(name).then_some(name))
}

/// # Safety
/// The caller roots the checked document. Iteration transfers an owned list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_iter(value: *const Object) -> *const Object {
    // SAFETY: iteration only borrows the immutable document to construct a rooted sequence.
    let result = unsafe {
        with_view(value, |view| {
            let view = view.expanded();
            let mut items = Words::new(5);
            match &view.nodes[view.index] {
                Node::Array(children) => return Ok(view.array(children)),
                Node::Object(fields) => {
                    for name in keys(fields) {
                        items
                            .values
                            .try_reserve(1)
                            .map_err(|_| Error::allocation())?;
                        let key = fields.owner(name)?;
                        items
                            .values
                            .push(View::single(Node::String(key)).into_handle() as usize);
                    }
                }
                Node::String(text) => items.values.extend(text.chars().map(|ch| {
                    View::single(Node::String(Rc::new(Str::new(ch.encode_utf8(&mut [0; 4])))))
                        .into_handle() as usize
                })),
                node => {
                    return Err(Error::new(
                        Kind::TypeError,
                        format!("'{}' object is not iterable", node.kind()),
                    ));
                }
            }
            Ok(Rc::into_raw(items.into_object()))
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the checked document through the length operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_len(value: *const Object) -> usize {
    // SAFETY: no document borrow escapes the scalar result.
    let result = unsafe {
        with_view(value, |view| {
            let view = view.expanded();
            let len = match &view.nodes[view.index] {
                Node::Array(children) => view.nodes.root_items().map_or_else(
                    || children.len(),
                    |items| items.0.with(|storage| storage.sequence().values.len()),
                ),
                Node::Object(fields) => keys(fields).count(),
                Node::String(text) => text.chars().count(),
                node => {
                    return Err(Error::new(
                        Kind::TypeError,
                        format!("object of type '{}' has no len()", node.kind()),
                    ));
                }
            };
            Ok(Int::from_usize(len).into_raw())
        })
    };
    finish(result, 0)
}

/// # Safety
/// Both the checked document and the canonical integer key are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_index(value: *const Object, key: usize) -> *const Object {
    // SAFETY: child handles are acquired before their parent borrow ends.
    let result = unsafe {
        with_view(value, |view| {
            let view = view.expanded();
            let key = Int::borrow(key);
            let at = || {
                crate::decode_index(&key).map_err(|_| {
                    Error::new(
                        Kind::IndexError,
                        "cannot fit 'int' into an index-sized integer",
                    )
                })
            };
            if let Some(items) = view.nodes.root_items() {
                return items.0.with(|storage| {
                    let items = storage.sequence();
                    let index = crate::index(at()?, items.values.len())?;
                    Ok(items.kind.acquire(items.values[index]).into_word() as *const Object)
                });
            }
            match &view.nodes[view.index] {
                Node::Array(children) => {
                    let index = crate::index(at()?, children.len())?;
                    Ok(View::handle(&view.nodes, children[index]))
                }
                Node::String(text) => {
                    let index = crate::index(at()?, text.chars().count())
                        .map_err(|_| Error::new(Kind::IndexError, "string index out of range"))?;
                    Ok(View::single(Node::String(Rc::new(Str::new(
                        text.chars().nth(index).unwrap().encode_utf8(&mut [0; 4]),
                    ))))
                    .into_handle())
                }
                Node::Object(_) => Err(Error::key(super::WordKind::new(2).acquire(key.0))),
                node => Err(Error::new(
                    Kind::TypeError,
                    format!("'{}' object is not subscriptable", node.kind()),
                )),
            }
        })
    };
    finish(result, std::ptr::null())
}
