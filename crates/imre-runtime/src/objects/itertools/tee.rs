//! Forks root their current block. Only the leading fork pulls from the source;
//! lagging forks read the saved owners, including after the leader is dropped.

use super::*;
use std::cell::OnceCell;

const BLOCK_SIZE: usize = 57;

struct Block {
    items: RefCell<Vec<OwnedWord>>,
    next: OnceCell<Rc<Block>>,
}

impl Block {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            items: RefCell::default(),
            next: OnceCell::new(),
        })
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        // A long lag can leave hundreds of thousands of uniquely owned blocks.
        // Unlink them iteratively and stop at the first block another fork roots.
        self.items.get_mut().clear();
        let mut next = self.next.take();
        while let Some(block) = next.and_then(|block| Rc::try_unwrap(block).ok()) {
            let mut block = block;
            next = block.next.take();
        }
    }
}

struct Shared {
    source: Option<crate::ObjectOwner>,
    pulling: Cell<bool>,
}

pub(in crate::objects) struct Fork {
    shared: Rc<Shared>,
    block: RefCell<Rc<Block>>,
    index: Cell<usize>,
}

impl Fork {
    pub(super) fn take_source(&mut self) -> Option<crate::ObjectOwner> {
        Rc::get_mut(&mut self.shared)?.source.take()
    }

    fn copy(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            block: RefCell::new(self.block.borrow().clone()),
            index: Cell::new(self.index.get()),
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
                pulling: Cell::new(false),
            }),
            block: RefCell::new(Block::new()),
            index: Cell::new(0),
        })
    }

    pub(super) fn advance(&self, result: Option<ItemResult>) -> Advance {
        let mut block = self.block.borrow().clone();
        let mut index = self.index.get();
        if index == BLOCK_SIZE {
            let next = block.next.get_or_init(Block::new).clone();
            self.block.replace(next.clone());
            block = next;
            index = 0;
            self.index.set(0);
        }
        let item = if let Some(result) = result {
            self.shared.pulling.set(false);
            match result {
                Ok(Some(item)) => {
                    block.items.borrow_mut().push(item.clone());
                    item
                }
                other => return Advance::Return(other),
            }
        } else if let Some(item) = block.items.borrow().get(index).cloned() {
            item
        } else {
            if self.shared.pulling.get() {
                return Advance::Return(Err(Error::new(
                    Kind::RuntimeError,
                    "cannot re-enter the tee iterator",
                )));
            }
            if index == 0
                && block
                    .items
                    .borrow_mut()
                    .try_reserve_exact(BLOCK_SIZE)
                    .is_err()
            {
                return Advance::Return(Err(Error::allocation()));
            }
            self.shared.pulling.set(true);
            return Advance::Pull(crate::ObjectOwner::clone(
                self.shared.source.as_ref().expect("live tee source"),
            ));
        };
        self.index.set(index + 1);
        Advance::Return(Ok(Some(item)))
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
