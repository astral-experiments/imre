//! Record attributes keep insertion order and materialize one live Python dictionary on demand.
use super::super::exceptions::{Result, value_word};
use super::super::mappings::{dict_set, imre_dict_new};
use super::super::words::OwnedWord;
use super::super::{Error, Kind, Object, Storage, StorageCell, Str};
use std::rc::Rc;

pub(super) enum Fields {
    Inline(Vec<(String, OwnedWord)>),
    Dictionary(Rc<Object>),
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
            Self::Dictionary(dict) => {
                // SAFETY: the dictionary owns retained-value views and this lookup acquires its result.
                let held = unsafe { dict.0.with(|s| s.dictionary().logging_key(&Str::from(key))) }?;
                // SAFETY: held roots the view until the original field has its own owner.
                unsafe {
                    let (kind, word) = value_word(held.word as *const Object);
                    Some(kind.acquire(word))
                }
            }
        }
    }

    pub(super) fn field(&self, key: &str) -> Result<OwnedWord> {
        self.get(key).ok_or_else(|| {
            Error::new(
                Kind::AttributeError,
                format!("'LogRecord' object has no attribute '{key}'"),
            )
        })
    }

    pub(super) fn insert(&mut self, key: String, value: OwnedWord) -> Result<()> {
        let value = original(value);
        match self {
            Self::Inline(fields) => {
                if let Some((_, v)) = fields.iter_mut().find(|(k, _)| *k == key) {
                    *v = value;
                } else {
                    fields.push((key, value));
                }
                Ok(())
            }
            Self::Dictionary(dict) => store(dict, key, value),
        }
    }

    /// After exposure, reads and writes use this dictionary, including changes made through an alias.
    pub(super) fn dictionary(&mut self) -> Result<Rc<Object>> {
        if let Self::Inline(fields) = self {
            // SAFETY: a new dictionary transfers one strong owner. Each value is a retained view.
            let dict = unsafe { Rc::from_raw(imre_dict_new(3, 5)) };
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

/// Retain the Python value, including when it came from another record dictionary.
fn original(value: OwnedWord) -> OwnedWord {
    if value.kind.raw() == 5 {
        // SAFETY: kind five roots the source through this immutable storage inspection.
        unsafe {
            (&*(value.word as *const Object)).0.with(|s| match s {
                Storage::ValueView(v) => Some(v.clone()),
                _ => None,
            })
        }
        .unwrap_or(value)
    } else {
        value
    }
}

fn store(dict: &Rc<Object>, key: String, value: OwnedWord) -> Result<()> {
    let key = Rc::new(Str::new(key));
    let view = Rc::new(Object(StorageCell::new(Storage::ValueView(value))));
    // SAFETY: the rooted dictionary takes string keys and retained object views.
    unsafe {
        dict_set(
            Rc::as_ptr(dict),
            Rc::as_ptr(&key) as usize,
            Rc::as_ptr(&view) as usize,
        )
    }
}
