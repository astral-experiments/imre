//! Immutable JSON documents for literal-key reads. A view owns its document,
//! so nested values survive the source array; typed scalar results own only
//! their native string or integer. No Python object graph or mutable alias is exposed.

use std::fmt;
use std::rc::Rc;

use serde::Deserializer;
use serde::de::{Error as _, MapAccess, Visitor};
use serde_json::value::RawValue;

use super::{Key, check_depth};
use crate::objects::exceptions::string_repr;
use crate::objects::words::Words;
use crate::objects::{Object, Storage, object};
use crate::{Error, Int, Kind, StorageCell, Str, finish};

enum Node {
    Null,
    Bool(bool),
    Integer(Int),
    Float(f64),
    String(Rc<Str>),
    Array(bool),
    Object(Vec<(Str, usize)>),
}

impl Node {
    fn truth(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) | Self::Array(value) => *value,
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

    fn field(&self, key: &str) -> Result<usize, Error> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, index)| *index)
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

pub(in crate::objects) struct View {
    nodes: Rc<Vec<Node>>,
    index: usize,
}

impl View {
    fn handle(nodes: &Rc<Vec<Node>>, index: usize) -> *const Object {
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Json(Self {
            nodes: Rc::clone(nodes),
            index,
        })))))
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
            (1..=3, _) => {
                let expected = ["", "dict", "str", "int"][mode as usize];
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
/// would discard. Nested arrays have no supported element-access operation.
fn push_node(nodes: &mut Vec<Node>, raw: &RawValue) -> Result<usize, serde_json::Error> {
    let raw = raw.get();
    let node = match raw.as_bytes()[0] {
        b'{' => {
            Node::Object(serde_json::Deserializer::from_str(raw).deserialize_map(Fields(nodes))?)
        }
        b'[' => Node::Array(!raw[1..].trim_start().starts_with(']')),
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
    let nodes = Rc::new(nodes);
    let mut result = Words::new(5);
    result.values.extend(
        roots
            .into_iter()
            .map(|index| View::handle(&nodes, index) as usize),
    );
    Ok(Rc::into_raw(result.into_object()))
}

/// # Safety
/// The caller roots the input string. Success transfers an ordinary list owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_loads(text: *const Str) -> *const Object {
    // SAFETY: text is a live immutable string for the entire parse.
    finish(loads(unsafe { &*text }), std::ptr::null())
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
