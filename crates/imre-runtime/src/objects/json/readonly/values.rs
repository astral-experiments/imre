//! Python value operations for JSON stored inside ordinary dataclass fields.

use std::fmt::Write as _;

use toml_datetime::{Datetime, Offset};

use super::{Error, Node, View, deep_values};
use crate::objects::exceptions::push_string_repr;

impl View {
    /// Containers use identity before comparing their elements. Direct equality
    /// of a floating point NaN is false even for the same operand.
    pub(super) fn equal_operand(&self, right: &Self) -> Result<bool, Error> {
        let is_nan = |view: &Self| {
            matches!(&view.nodes[view.index], Node::ConstantNan)
                || matches!(&view.nodes[view.index], Node::Float(f) if f.is_nan())
        };
        if is_nan(self) || is_nan(right) {
            Ok(false)
        } else {
            self.equal(right)
        }
    }

    pub(in crate::objects) fn equal(&self, right: &Self) -> Result<bool, Error> {
        if let (Some(left), Some(right)) = (self.nodes.root_items(), right.nodes.root_items()) {
            // Preserve Python list identity shortcuts for the root and shared elements.
            // SAFETY: both objects are rooted lists with JsonValue element storage.
            return unsafe {
                crate::objects::values::try_equal(
                    crate::ObjectOwner::as_ptr(left) as usize,
                    crate::ObjectOwner::as_ptr(right) as usize,
                    crate::objects::words::WordKind::new(5),
                )
            };
        }
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

    pub(super) fn equal_at(&self, a: usize, other: &Self, b: usize) -> Result<bool, Error> {
        if let Node::Deferred(array) = &self.nodes[a] {
            let v = array.try_view()?;
            return v.equal_at(v.index, other, b);
        }
        if let Node::Deferred(array) = &other.nodes[b] {
            let v = array.try_view()?;
            return self.equal_at(a, v, v.index);
        }
        let left = &self.nodes[a];
        let right = &other.nodes[b];
        if std::ptr::eq(left, right) {
            return Ok(true);
        }
        Ok(match (left, right) {
            (Node::Null, Node::Null) | (Node::ConstantNan, Node::ConstantNan) => true,
            (Node::Datetime(a), Node::Datetime(b)) => super::super::toml::datetimes_equal(a, b),
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
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (&i, &j) in a.iter().zip(b) {
                    if !self.equal_at(i, other, j)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Node::Object(a), Node::Object(b)) => {
                for (i, (key, value)) in a.iter().enumerate() {
                    if a.is_shadowed(i) {
                        continue;
                    }
                    let Some(target) = right.lookup(key) else {
                        return Ok(false);
                    };
                    if !self.equal_at(*value, other, target)? {
                        return Ok(false);
                    }
                }
                b.iter().all(|(key, _)| left.lookup(key).is_some())
            }
            _ => false,
        })
    }

    pub(in crate::objects) fn write_repr(&self, output: &mut String) {
        self.try_write_repr(output).unwrap_or_else(Error::raise)
    }

    pub(in crate::objects) fn try_write_repr(&self, output: &mut String) -> Result<(), Error> {
        if let Some(items) = self.nodes.root_items() {
            // SAFETY: the root owns this ordinary list through the entire repr traversal.
            unsafe {
                crate::objects::values::write_repr(
                    output,
                    crate::ObjectOwner::as_ptr(items) as usize,
                    crate::objects::words::WordKind::new(5),
                )
            };
        } else if self.nodes.deep() {
            deep_values::write_repr(self, output)?;
        } else {
            self.write_at(self.index, output)?;
        }
        Ok(())
    }

    pub(super) fn write_at(&self, i: usize, output: &mut String) -> Result<(), Error> {
        let node = &self.nodes[i];
        match node {
            Node::Null => output.push_str("None"),
            Node::Datetime(value) => write_datetime_repr(value, output),
            Node::Bool(b) => output.push_str(if *b { "True" } else { "False" }),
            Node::Integer(i) => {
                if let Some(i) = i.as_small() {
                    write!(output, "{i}").unwrap();
                } else {
                    write!(output, "{}", i.big()).unwrap();
                }
            }
            Node::Float(f) => output.push_str(&crate::floats::render(*f)),
            Node::ConstantNan => output.push_str("nan"),
            Node::String(s) => push_string_repr(s, output),
            Node::Deferred(array) => array.try_view()?.try_write_repr(output)?,
            Node::Array(items) => {
                output.push('[');
                for (offset, &item) in items.iter().enumerate() {
                    if offset != 0 {
                        output.push_str(", ");
                    }
                    self.write_at(item, output)?;
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
                    push_string_repr(name, output);
                    output.push_str(": ");
                    self.write_at(node.lookup(name).unwrap(), output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }
}

fn write_datetime_repr(text: &str, output: &mut String) {
    let value: Datetime = text.parse().expect("validated TOML datetime");
    match (value.date, value.time) {
        (Some(date), time) => {
            let kind = if time.is_some() { "datetime" } else { "date" };
            write!(
                output,
                "datetime.{kind}({}, {}, {}",
                date.year, date.month, date.day
            )
            .unwrap();
            if time.is_some() {
                output.push_str(", ");
            }
        }
        (None, Some(_)) => output.push_str("datetime.time("),
        (None, None) => unreachable!("TOML datetimes contain a date or time"),
    }
    if let Some(time) = value.time {
        write!(output, "{}, {}", time.hour, time.minute).unwrap();
        let seconds = time.second.unwrap_or(0);
        let micros = time.nanosecond.unwrap_or(0) / 1000;
        if seconds != 0 || micros != 0 {
            write!(output, ", {seconds}").unwrap();
        }
        if micros != 0 {
            write!(output, ", {micros}").unwrap();
        }
    }
    if let Some(offset) = value.offset {
        output.push_str(", tzinfo=datetime.timezone");
        let minutes = match offset {
            Offset::Z => 0,
            Offset::Custom { minutes } => i32::from(minutes),
        };
        if minutes == 0 {
            output.push_str(".utc");
        } else {
            output.push_str("(datetime.timedelta(");
            let seconds = minutes * 60;
            if seconds < 0 {
                output.push_str("days=-1, ");
            }
            write!(output, "seconds={}))", seconds.rem_euclid(86_400)).unwrap();
        }
    }
    output.push(')');
}

#[cfg(test)]
mod tests {
    use super::write_datetime_repr;

    #[test]
    fn toml_datetimes_use_python_constructor_representations() {
        for (text, expected) in [
            ("0001-05-27", "datetime.date(1, 5, 27)"),
            ("07:32:00", "datetime.time(7, 32)"),
            ("07:32:00.000042", "datetime.time(7, 32, 0, 42)"),
            (
                "1979-05-27 07:32:03+00:00",
                "datetime.datetime(1979, 5, 27, 7, 32, 3, tzinfo=datetime.timezone.utc)",
            ),
            (
                "1979-05-27 07:32:00-00:30",
                "datetime.datetime(1979, 5, 27, 7, 32, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=84600)))",
            ),
        ] {
            let mut output = String::new();
            write_datetime_repr(text, &mut output);
            assert_eq!(output, expected);
        }
    }
}
