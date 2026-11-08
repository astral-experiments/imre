//! Forks root their current block. Only the leading fork pulls from the source;
//! lagging forks read the saved owners, including after the leader is dropped.

use super::*;
use crate::tee::{Block as Data, Position, Store};

type BlockOwner = crate::shared::Shared<Block>;

struct Block(RefCell<Data<OwnedWord, BlockOwner>>);

impl Block {
    fn new() -> Self {
        Self(RefCell::new(Data::new()))
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        // Unlink uniquely owned blocks iteratively when a long-lagging fork dies.
        let block = self.0.get_mut();
        block.items.clear();
        let mut next = block.following.take();
        while let Some(mut block) = next.and_then(|block| BlockOwner::try_unwrap(block).ok()) {
            next = block.0.get_mut().following.take();
        }
    }
}

struct CheckedBlocks;

impl Store for CheckedBlocks {
    type Link = BlockOwner;
    type Item = OwnedWord;

    fn allocate(&mut self) -> Result<BlockOwner, Error> {
        BlockOwner::try_new(Block::new()).map_err(|_| Error::memory_error())
    }

    fn with_block<R>(
        &mut self,
        link: &BlockOwner,
        f: impl FnOnce(&mut Data<OwnedWord, BlockOwner>) -> R,
    ) -> R {
        f(&mut link.0.borrow_mut())
    }
}

struct Shared {
    source: Option<crate::ObjectOwner>,
}

pub(in crate::objects) struct Fork {
    shared: Rc<Shared>,
    position: RefCell<Position<BlockOwner>>,
}

impl Fork {
    pub(super) fn take_source(&mut self) -> Option<crate::ObjectOwner> {
        Rc::get_mut(&mut self.shared)?.source.take()
    }

    fn copy(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            position: RefCell::new(self.position.borrow().clone()),
        }
    }

    fn from_source(source: crate::ObjectOwner) -> Self {
        // SAFETY: snapshot an existing fork without pulling from or changing it.
        let fork = unsafe {
            source.0.with(|storage| {
                if let Storage::IteratorAdapter(adapter) = storage
                    && let Adapter::Tee(fork) = adapter.as_ref()
                {
                    Some(fork.copy())
                } else {
                    None
                }
            })
        };
        fork.unwrap_or_else(|| Self {
            shared: Rc::new(Shared {
                source: Some(source),
            }),
            position: RefCell::new(Position::new(BlockOwner::new(Block::new()))),
        })
    }

    pub(super) fn advance(&self, result: Option<ItemResult>) -> Advance {
        if let Some(result) = result {
            self.position.borrow_mut().finish(
                &mut CheckedBlocks,
                result.as_ref().ok().and_then(Option::as_ref),
            );
            return Advance::Return(result);
        }
        // Publishing a rollover may release a whole block of item owners. Let
        // those owners drop after replace() has released the cursor borrow.
        let mut position = self.position.borrow().clone();
        let prepared = position.prepare(&mut CheckedBlocks);
        self.position.replace(position);
        match prepared {
            Ok(Some(item)) => Advance::Return(Ok(Some(item))),
            Ok(None) => Advance::Pull(crate::ObjectOwner::clone(
                self.shared.source.as_ref().expect("live tee source"),
            )),
            Err(error) => Advance::Return(Err(error)),
        }
    }
}

/// # Safety
/// The caller roots the selected source. The immutable tuple descriptor has one
/// Object field per requested fork and outlives all uses of the returned tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tee(
    mode: u32,
    source: usize,
    layout: *const records::RecordDescriptor,
) -> *mut usize {
    // SAFETY: new records start all-zero and own each initialized pointer field.
    unsafe {
        let tuple = records::imre_record_try_new(layout);
        if tuple.is_null() || (*layout).length == 0 {
            return tuple;
        }
        let first = Fork::from_source(sources::acquire(mode, source));
        for i in 0..(*layout).length {
            let fork = adapter(Adapter::Tee(first.copy()));
            tuple
                .cast::<*const Object>()
                .add(i)
                .write(crate::ObjectOwner::into_raw(fork));
        }
        tuple
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_mixed_forks_keeps_the_other_fork_alive() {
        let cycle = |source| {
            adapter(Adapter::Cycle(Cycle {
                source: RefCell::new(source),
                saved: RefCell::default(),
                index: Cell::new(0),
            }))
        };
        let surviving = Fork::from_source(cycle(None));
        let mut cursor = adapter(Adapter::Tee(surviving.copy()));
        for _ in 0..16_384 {
            cursor = adapter(Adapter::Tee(Fork::from_source(cycle(Some(cursor)))));
        }
        drop(cursor);
        assert!(matches!(
            next_item(&adapter(Adapter::Tee(surviving))),
            Ok(None)
        ));
    }
}
