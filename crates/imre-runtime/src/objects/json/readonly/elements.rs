//! Decode materialized arrays of records without reparsing each field as raw JSON.
use crate::StrOwner;
use std::cell::Cell;
use std::fmt;

use serde::Deserializer;
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};

use super::{Int, Key, KeyMemo, Node, Str, allocation_error, push};

/// Nested arrays retain their lazy source spans. Floating-point callbacks also
/// fall back because Serde can report an out-of-range integer as a float.
pub(super) fn parse(
    text: &str,
    nodes: &mut Vec<Node>,
    keys: &KeyMemo,
    refused: &Cell<bool>,
) -> Option<Node> {
    let start = nodes.len();
    let value = serde_json::Deserializer::from_str(text).deserialize_seq(&mut Elements {
        nodes,
        entered: false,
        keys,
        refused,
    });
    if let Ok(array) = value {
        Some(array)
    } else {
        nodes.truncate(start);
        None
    }
}

struct Elements<'a> {
    nodes: &'a mut Vec<Node>,
    entered: bool,
    keys: &'a KeyMemo,
    refused: &'a Cell<bool>,
}

impl<'de> DeserializeSeed<'de> for &mut Elements<'_> {
    type Value = usize;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        let node = d.deserialize_any(&mut *self)?;
        let index = self.nodes.len();
        push(self.nodes, node, self.refused)?;
        Ok(index)
    }
}

impl<'de> Visitor<'de> for &mut Elements<'_> {
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
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Node, E> {
        let value = Str::try_new(value).map_err(|_| allocation_error::<E>(self.refused))?;
        Ok(Node::String(
            StrOwner::try_new(value).map_err(|_| allocation_error::<E>(self.refused))?,
        ))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Node, A::Error> {
        if std::mem::replace(&mut self.entered, true) {
            return Err(A::Error::custom("nested array requires its own raw span"));
        }
        let mut items = Vec::new();
        while let Some(index) = sequence.next_element_seed(&mut *self)? {
            push(&mut items, index, self.refused)?;
        }
        Ok(Node::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut fields = Vec::new();
        while let Some(key) = map.next_key_seed(Key)? {
            let key = Str::try_new(key.as_ref())
                .map_err(|_| allocation_error::<A::Error>(self.refused))?;
            let value = map.next_value_seed(&mut *self)?;
            push(&mut fields, (key, value), self.refused)?;
        }
        Ok(Node::Object(self.keys.fields(fields)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_integer_rows_do_not_confuse_string_brackets_with_nested_arrays() {
        let mut nodes = vec![Node::Null];
        let keys = KeyMemo::default();
        let row = parse(
            r#"[{"v":18446744073709551615,"name":"[row]"}]"#,
            &mut nodes,
            &keys,
            &Cell::new(false),
        );
        assert!(matches!(row, Some(Node::Array(items)) if items.len() == 1));
        assert!(matches!(&nodes[1], Node::Integer(n) if n.big().as_ref() == &u64::MAX.into()));
        let before = nodes.len();
        for raw in ["[0,[1]]", "[0,0.5]", "[0,18446744073709551616]", "[0,NaN]"] {
            assert!(parse(raw, &mut nodes, &keys, &Cell::new(false)).is_none());
            assert_eq!(nodes.len(), before);
        }
    }
}
