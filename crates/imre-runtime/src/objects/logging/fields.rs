//! Record attributes keep insertion order and materialize one live Python dictionary on demand.
use super::super::mappings::{dict_set, imre_dict_new};
use super::super::words::OwnedWord;
use super::super::{Error, Kind, Object, Storage, StorageCell, Str};
use super::super::{boxed_values, exceptions::Result};
use crate::{ObjectOwner, StrOwner};

pub(super) enum Fields {
    Inline(Vec<(String, OwnedWord)>),
    Dictionary(ObjectOwner),
}

impl Fields {
    pub(super) fn new<const N: usize>(fields: [(&str, OwnedWord); N]) -> Self {
        Self::Inline(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), original(value)))
                .collect(),
        )
    }

    pub(super) fn get(&self, key: &str) -> Option<OwnedWord> {
        match self {
            Self::Inline(fields) => fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone()),
            Self::Dictionary(dict) => dictionary_value(dict, &Str::from(key)),
        }
    }

    /// Reuse the caller's existing string when reading an exposed record dictionary.
    pub(super) fn get_text(&self, key: &Str) -> Option<OwnedWord> {
        match self {
            Self::Inline(_) => self.get(key.as_str()),
            Self::Dictionary(dict) => dictionary_value(dict, key),
        }
    }

    pub(super) fn missing(key: &str) -> Error {
        Error::new(
            Kind::AttributeError,
            format!("'LogRecord' object has no attribute '{key}'"),
        )
    }

    pub(super) fn field(&self, key: &str) -> Result<OwnedWord> {
        self.get(key).ok_or_else(|| Self::missing(key))
    }

    pub(super) fn insert(&mut self, key: String, value: OwnedWord) -> Result<()> {
        let value = original(value);
        match self {
            Self::Inline(fields) => {
                if let Some((_, v)) = fields.iter_mut().find(|(k, _)| *k == key) {
                    *v = value;
                } else {
                    fields.try_reserve(1).map_err(|_| Error::memory_error())?;
                    fields.push((key, value));
                }
                Ok(())
            }
            Self::Dictionary(dict) => store(dict, key, value),
        }
    }

    /// After exposure, reads and writes use this dictionary, including changes made through an alias.
    pub(super) fn dictionary(&mut self) -> Result<ObjectOwner> {
        if let Self::Inline(fields) = self {
            // SAFETY: a new dictionary transfers one strong owner. Each value is a retained view.
            let dict = unsafe { ObjectOwner::from_raw(imre_dict_new(3, 5)) };
            for (key, value) in fields.iter() {
                store(&dict, key.clone(), value.clone())?;
            }
            *self = Self::Dictionary(dict);
        }
        let Self::Dictionary(dict) = self else {
            unreachable!()
        };
        Ok(dict.clone())
    }
}

fn dictionary_value(dict: &ObjectOwner, key: &Str) -> Option<OwnedWord> {
    // SAFETY: the dictionary owns retained-value views and the lookup acquires its result.
    let held = unsafe { dict.0.with(|s| s.dictionary().logging_key(key)) }?;
    // SAFETY: held roots the view until the original value has an independent owner.
    unsafe {
        let (kind, word) = boxed_values::concrete(held.kind, held.word);
        Some(kind.acquire(word))
    }
}

/// Retain the Python value, including when it came from another record dictionary.
fn original(value: OwnedWord) -> OwnedWord {
    if value.kind.raw() == 5 {
        // SAFETY: kind five roots the source through this immutable storage inspection.
        unsafe {
            (&*(value.word as *const Object)).0.with(|s| match s {
                Storage::ObjectValue(v) => Some(v.clone()),
                _ => None,
            })
        }
        .unwrap_or(value)
    } else {
        value
    }
}

fn store(dict: &ObjectOwner, key: String, value: OwnedWord) -> Result<()> {
    let key = StrOwner::try_new(Str::new(key)).map_err(|_| Error::memory_error())?;
    let view = ObjectOwner::try_new(Object(StorageCell::new(Storage::ObjectValue(value))))
        .map_err(|_| Error::memory_error())?;
    // SAFETY: the rooted dictionary takes string keys and retained object views.
    unsafe {
        dict_set(
            ObjectOwner::as_ptr(dict),
            StrOwner::as_ptr(&key) as usize,
            ObjectOwner::as_ptr(&view) as usize,
        )
    }
}
