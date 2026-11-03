//! Runtime collection categories for structural patterns. These are type properties,
//! independent of both an object's protocol methods and virtual subclass checks.
use super::{Context, Id, Value};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PatternFlags(u8);

impl PatternFlags {
    pub(super) const SEQUENCE: Self = Self(1 << 5);
    pub(super) const MAPPING: Self = Self(1 << 6);

    pub(super) fn marker(self) -> i64 {
        i64::from(self.0)
    }

    pub(super) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(super) fn has(self, category: Self) -> bool {
        self.0 & category.0 != 0
    }

    pub(super) fn add_marker(&mut self, marker: std::ffi::c_long) {
        self.0 |= (marker as u8) & (Self::SEQUENCE.0 | Self::MAPPING.0);
    }

    fn native(class: &str) -> Self {
        match class {
            "list" | "tuple" | "range" | "memoryview" | "array.array" | super::deque::CLASS => {
                Self::SEQUENCE
            }
            "dict" | "mappingproxy" => Self::MAPPING,
            _ => match Context::native_base(class) {
                Some("tuple") => Self::SEQUENCE,
                Some("dict") => Self::MAPPING,
                _ => Self::default(),
            },
        }
    }
}

impl Context {
    pub(super) fn class_pattern_flags(&self, class: Id) -> PatternFlags {
        match self.heap.get(class) {
            Value::Native(name) => PatternFlags::native(name),
            Value::Class { pattern_flags, .. } => *pattern_flags,
            _ => PatternFlags::default(),
        }
    }

    /// Match the actual type without invoking attribute, subclass, or length hooks.
    pub(super) fn pattern_flags(&self, subject: Id) -> PatternFlags {
        let value = self.heap.get(subject);
        if let Some(class) = value.instance_class() {
            return self.class_pattern_flags(class);
        }
        if matches!(value, Value::NamedTuple { .. }) {
            return PatternFlags::SEQUENCE;
        }
        PatternFlags::native(self.builtin_type_name(subject).unwrap_or_default())
    }
}
