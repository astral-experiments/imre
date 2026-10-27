use super::{Int, OwnedWord, WordKind};

/// The numeric state of a count cursor, shared by typed and runtime-loaded Python.
#[derive(Clone)]
pub(crate) enum CountValue {
    Int(Int),
    Float(f64),
}

impl CountValue {
    /// Compute the successor before yielding the old value, preserving it if conversion fails.
    pub(crate) fn successor(&self, step: &Self) -> crate::Result<Self> {
        match (self, step) {
            (Self::Int(a), Self::Int(b)) => crate::integer_binary(a, b, 0).map(Self::Int),
            (Self::Float(a), Self::Float(b)) => Ok(Self::Float(a + b)),
            (Self::Int(a), Self::Float(b)) => Ok(Self::Float(crate::floats::int_to_float(a)? + b)),
            (Self::Float(a), Self::Int(b)) => Ok(Self::Float(a + crate::floats::int_to_float(b)?)),
        }
    }

    pub(super) fn into_word(self) -> OwnedWord {
        let (word, kind) = match self {
            Self::Int(n) => (n.into_raw(), 2),
            Self::Float(f) => (f.to_bits() as usize, 7),
        };
        // SAFETY: each numeric variant supplies its matching owned representation.
        unsafe { OwnedWord::from_owned(word, WordKind::new(kind)) }
    }
}

impl std::fmt::Display for CountValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Int(n) => n.fmt(f),
            Self::Float(n) => crate::floats::render(*n).fmt(f),
        }
    }
}
