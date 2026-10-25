//! Python value operations for JSON stored inside ordinary dataclass fields.

use std::fmt::Write as _;

use super::{Node, View, deep_values};

impl View {
    /// Containers use identity before comparing their elements. Direct equality
    /// of a floating point NaN is false even for the same operand.
    pub(super) fn equal_operand(&self, right: &Self) -> bool {
        let is_nan = |view: &Self| {
            matches!(&view.nodes[view.index], Node::ConstantNan)
                || matches!(&view.nodes[view.index], Node::Float(f) if f.is_nan())
        };
        !is_nan(self) && !is_nan(right) && self.equal(right)
    }

    pub(in crate::objects) fn equal(&self, right: &Self) -> bool {
        let left_root = self.snapshot_root();
        let right_root = right.snapshot_root();
        let left = left_root.as_ref().unwrap_or(self);
        let right = right_root.as_ref().unwrap_or(right);
        if left.nodes.deep() || right.nodes.deep() {
            deep_values::equal(left, right)
        } else {
            left.equal_at(left.index, right, right.index)
        }
    }

    pub(super) fn equal_at(&self, a: usize, other: &Self, b: usize) -> bool {
        if let Node::Deferred(array) = &self.nodes[a] {
            let v = array.view();
            return v.equal_at(v.index, other, b);
        }
        if let Node::Deferred(array) = &other.nodes[b] {
            let v = array.view();
            return self.equal_at(a, v, v.index);
        }
        let left = &self.nodes[a];
        let right = &other.nodes[b];
        if std::ptr::eq(left, right) {
            return true;
        }
        match (left, right) {
            (Node::Null, Node::Null) | (Node::ConstantNan, Node::ConstantNan) => true,
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
                a.iter().enumerate().all(|(i, (key, value))| {
                    a.is_shadowed(i)
                        || right
                            .lookup(key)
                            .is_some_and(|b| self.equal_at(*value, other, b))
                }) && b.iter().all(|(key, _)| left.lookup(key).is_some())
            }
            _ => false,
        }
    }

    pub(in crate::objects) fn write_repr(
        &self,
        output: &mut crate::objects::values::ReprWriter<'_>,
    ) -> std::fmt::Result {
        if let Some(items) = self.nodes.root_items() {
            // SAFETY: the root owns this ordinary list through the entire repr traversal.
            unsafe {
                crate::objects::values::try_write_repr(
                    output,
                    std::rc::Rc::as_ptr(items) as usize,
                    crate::objects::words::WordKind::new(5),
                )
            }
        } else if self.nodes.deep() {
            deep_values::write_repr(self, output)
        } else {
            self.write_at(self.index, output)
        }
    }

    pub(super) fn write_at(
        &self,
        i: usize,
        output: &mut crate::objects::values::ReprWriter<'_>,
    ) -> std::fmt::Result {
        let node = &self.nodes[i];
        match node {
            Node::Null => output.write_str("None")?,
            Node::Datetime(value) => output.write_str(value)?,
            Node::Bool(b) => output.write_str(if *b { "True" } else { "False" })?,
            Node::Integer(i) => {
                if let Some(i) = i.as_small() {
                    write!(output, "{i}")?;
                } else {
                    write!(output, "{}", i.big())?;
                }
            }
            Node::Float(f) => output.write_str(&crate::floats::render(*f))?,
            Node::ConstantNan => output.write_str("nan")?,
            Node::String(s) => crate::objects::exceptions::write_quoted_repr(s, output)?,
            Node::Deferred(array) => array.view().write_repr(output)?,
            Node::Array(items) => {
                output.write_char('[')?;
                for (offset, &item) in items.iter().enumerate() {
                    if offset != 0 {
                        output.write_str(", ")?;
                    }
                    self.write_at(item, output)?;
                }
                output.write_char(']')?;
            }
            Node::Object(fields) => {
                output.write_char('{')?;
                for (offset, (name, _)) in fields.iter().enumerate() {
                    if fields[..offset].iter().any(|(old, _)| old == name) {
                        continue;
                    }
                    if offset != 0 {
                        output.write_str(", ")?;
                    }
                    crate::objects::exceptions::write_quoted_repr(name, output)?;
                    output.write_str(": ")?;
                    self.write_at(node.lookup(name).unwrap(), output)?;
                }
                output.write_char('}')?;
            }
        }
        Ok(())
    }
}
