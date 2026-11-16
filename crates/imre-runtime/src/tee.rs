//! Shared cursor transitions for checked and boxed itertools.tee owners.

use crate::{Error, Kind};

const BLOCK_SIZE: usize = 57;

#[derive(Clone)]
pub(crate) struct Position<L> {
    pub(crate) block: L,
    index: usize,
}

#[derive(Clone)]
pub(crate) struct Block<T, L> {
    pub(crate) items: Vec<T>,
    pub(crate) following: Option<L>,
    running: bool,
}

impl<T, L> Block<T, L> {
    pub(crate) fn new() -> Self {
        Self {
            items: Vec::new(),
            following: None,
            running: false,
        }
    }
}

/// Checked iterators own reference-counted links. Boxed iterators use traced heap IDs.
pub(crate) trait Store {
    type Link: Clone;
    type Item: Clone;
    fn allocate(&mut self) -> Result<Self::Link, Error>;
    fn with_block<R>(
        &mut self,
        link: &Self::Link,
        f: impl FnOnce(&mut Block<Self::Item, Self::Link>) -> R,
    ) -> R;
}

impl<L: Clone> Position<L> {
    pub(crate) fn new(block: L) -> Self {
        Self { block, index: 0 }
    }

    /// Return a buffered item, or reserve the frontier for one source callback.
    pub(crate) fn prepare<S: Store<Link = L>>(
        &mut self,
        store: &mut S,
    ) -> Result<Option<S::Item>, Error> {
        if self.index == BLOCK_SIZE {
            let next = store.with_block(&self.block, |b| b.following.clone());
            self.block = if let Some(next) = next {
                next
            } else {
                let next = store.allocate()?;
                store.with_block(&self.block, |b| b.following = Some(next.clone()));
                next
            };
            self.index = 0;
        }
        let item = store.with_block(&self.block, |b| {
            if let Some(item) = b.items.get(self.index) {
                return Ok(Some(item.clone()));
            }
            if b.running {
                return Err(Error::new(
                    Kind::RuntimeError,
                    "cannot re-enter the tee iterator",
                ));
            }
            if self.index == 0 {
                b.items
                    .try_reserve_exact(BLOCK_SIZE)
                    .map_err(|_| Error::memory_error())?;
            }
            b.running = true;
            Ok(None)
        })?;
        if item.is_some() {
            self.index += 1;
        }
        Ok(item)
    }

    /// Clear the reentrancy guard on every return. Only a yielded item advances the fork.
    pub(crate) fn finish<S: Store<Link = L>>(&mut self, store: &mut S, item: Option<&S::Item>) {
        store.with_block(&self.block, |b| {
            b.running = false;
            if let Some(item) = item {
                b.items.push(item.clone());
                self.index += 1;
            }
        });
    }
}
