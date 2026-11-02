//! Runtime collection categories for structural patterns. These are type properties,
//! independent of both an object's protocol methods and virtual subclass checks.
use super::{Context, Error, Id, Result, Value};

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

    /// ABC initialization consumes only the class's own marker, after abstract methods.
    pub(super) fn init_pattern_flags(&mut self, class: Id, scope: Id) -> Result<()> {
        const NAME: &str = "__abc_tpflags__";
        let Some(marker) = self.namespace_binding(self.scope_data(scope), NAME) else {
            return Ok(());
        };
        self.heap.keep(marker);
        let key = self.heap.code_name(NAME);
        self.remove_binding(scope, NAME, key)?;
        let Value::Int(value) = self.heap.get(marker) else {
            return Ok(());
        };
        let value = value
            .to_i64()
            .and_then(|n| std::ffi::c_long::try_from(n).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C long")
            })?;
        let mask = PatternFlags::SEQUENCE.0 | PatternFlags::MAPPING.0;
        let bits = value as u8 & mask;
        if bits == mask {
            return Err(Error::new(
                "TypeError",
                "__abc_tpflags__ cannot be both Py_TPFLAGS_SEQUENCE and Py_TPFLAGS_MAPPING",
            ));
        }
        let Value::Class { pattern_flags, .. } = self.heap.get_mut(class) else {
            unreachable!()
        };
        pattern_flags.0 |= bits;
        Ok(())
    }
}
