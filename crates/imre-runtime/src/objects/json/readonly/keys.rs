//! Python exposes the identity of decoded dictionary keys. Every object in one
//! input shares owners, including objects reached later through a deferred span.
use crate::StrOwner;
use crate::shared::Shared;
use allocator_api2::alloc::AllocError;
use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::ops::Deref;

use super::{Error, Str};

type Owners = Shared<RefCell<HashSet<StrOwner>>>;

#[derive(Default)]
pub(super) struct KeyMemo(OnceCell<Owners>);

impl KeyMemo {
    fn owners(&self) -> Result<&Owners, AllocError> {
        if let Some(owners) = self.0.get() {
            return Ok(owners);
        }
        let fresh = Shared::try_new(RefCell::new(HashSet::new()))?;
        Ok(self.0.get_or_init(|| fresh))
    }

    pub(super) fn shared(&self) -> Result<Self, AllocError> {
        Ok(Self(OnceCell::from(self.owners()?.clone())))
    }

    pub(super) fn fields(&self, entries: Vec<(Str, usize)>) -> Result<ObjectFields, AllocError> {
        Ok(ObjectFields {
            entries,
            owners: self.owners()?.clone(),
        })
    }
}

#[derive(Clone)]
pub(in crate::objects::json) struct ObjectFields {
    entries: Vec<(Str, usize)>,
    owners: Owners,
}

impl Deref for ObjectFields {
    type Target = [(Str, usize)];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl ObjectFields {
    /// A later occurrence replaces this value in the decoded dictionary.
    pub(super) fn is_shadowed(&self, index: usize) -> bool {
        let key = &self.entries[index].0;
        self.entries[index + 1..]
            .iter()
            .any(|(name, _)| name == key)
    }

    pub(super) fn with_entries(&self, entries: Vec<(Str, usize)>) -> Self {
        Self {
            entries,
            owners: self.owners.clone(),
        }
    }

    /// Field lookups can borrow their spelling; materialize an owner when a
    /// key escapes through iteration or conversion to a Python dictionary.
    pub(super) fn owner(&self, key: &Str) -> Result<StrOwner, Error> {
        let mut owners = self.owners.borrow_mut();
        if let Some(owner) = owners.get(key) {
            return Ok(owner.clone());
        }
        owners.try_reserve(1).map_err(|_| Error::allocation())?;
        let owner = StrOwner::new(Str::try_new(key.as_str()).map_err(|_| Error::allocation())?);
        owners.insert(owner.clone());
        Ok(owner)
    }
}
