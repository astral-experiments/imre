//! Python value operations for JSON stored inside ordinary dataclass fields.

use std::fmt::Write as _;

use super::{Node, View, string_repr};

impl View {
    pub(in crate::objects) fn equal(&self, right: &Self) -> bool {
        self.equal_at(self.index, right, right.index)
    }

    fn equal_at(&self, a: usize, other: &Self, b: usize) -> bool {
        let left = &self.nodes[a];
        let right = &other.nodes[b];
        if std::ptr::eq(left, right) {
            return true;
        }
        match (left, right) {
            (Node::Null, Node::Null) => true,
            (Node::Datetime(a), Node::Datetime(b)) => a == b,
            (Node::String(a), Node::String(b)) => a == b,
            (Node::Float(a), Node::Float(b)) => a == b,
            (Node::Integer(a), Node::Integer(b)) => a.0 == b.0 || a.big() == b.big(),
            (Node::Bool(a), Node::Bool(b)) => a == b,
            (Node::Float(f), Node::Integer(i)) | (Node::Integer(i), Node::Float(f)) => {
                crate::floats::compare(i, *f) == 0
            }
            (Node::Bool(b), Node::Integer(i)) | (Node::Integer(i), Node::Bool(b)) => {
                i.as_small() == Some(i64::from(*b))
            }
            (Node::Bool(b), Node::Float(f)) | (Node::Float(f), Node::Bool(b)) => {
                *f == f64::from(*b)
            }
            (Node::Array(a), Node::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(&a, &b)| self.equal_at(a, other, b))
            }
            (Node::Object(a), Node::Object(b)) => {
                // Only the last value for a repeated JSON key participates in comparison.
                a.iter().all(|(key, _)| {
                    right
                        .lookup(key)
                        .is_some_and(|b| self.equal_at(left.lookup(key).unwrap(), other, b))
                }) && b.iter().all(|(key, _)| left.lookup(key).is_some())
            }
            _ => false,
        }
    }

    pub(in crate::objects) fn write_repr(&self, output: &mut String) {
        self.write_at(self.index, output);
    }

    fn write_at(&self, i: usize, output: &mut String) {
        let node = &self.nodes[i];
        match node {
            Node::Null => output.push_str("None"),
            Node::Datetime(value) => output.push_str(value),
            Node::Bool(b) => output.push_str(if *b { "True" } else { "False" }),
            Node::Integer(i) => {
                if let Some(i) = i.as_small() {
                    write!(output, "{i}").unwrap();
                } else {
                    write!(output, "{}", i.big()).unwrap();
                }
            }
            Node::Float(f) => output.push_str(&crate::floats::render(*f)),
            Node::String(s) => output.push_str(&string_repr(s)),
            Node::Array(items) => {
                output.push('[');
                for (offset, &item) in items.iter().enumerate() {
                    if offset != 0 {
                        output.push_str(", ");
                    }
                    self.write_at(item, output);
                }
                output.push(']');
            }
            Node::Object(fields) => {
                output.push('{');
                for (offset, (name, _)) in fields.iter().enumerate() {
                    if fields[..offset].iter().any(|(old, _)| old == name) {
                        continue;
                    }
                    if offset != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&string_repr(name));
                    output.push_str(": ");
                    self.write_at(node.lookup(name).unwrap(), output);
                }
                output.push('}');
            }
        }
    }
}
