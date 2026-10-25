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
    source: Option<Rc<Object>>,
    pulling: Cell<bool>,
}

pub(in crate::objects) struct Fork {
    shared: Rc<Shared>,
    block: RefCell<Rc<Block>>,
    index: Cell<usize>,
}

impl Fork {
    pub(super) fn take_source(&mut self) -> Option<Rc<Object>> {
        Rc::get_mut(&mut self.shared)?.source.take()
    }

    fn copy(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            block: RefCell::new(self.block.borrow().clone()),
            index: Cell::new(self.index.get()),
        }
    }

    fn from_source(source: Rc<Object>) -> Self {
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

    pub(super) fn next(&self) -> Result<Option<OwnedWord>, Error> {
        let mut block = self.block.borrow().clone();
        let mut index = self.index.get();
        if index == BLOCK_SIZE {
            let next = block.next.get_or_init(Block::new).clone();
            self.block.replace(next.clone());
            block = next;
            index = 0;
            self.index.set(0);
        }
        let saved = block.items.borrow().get(index).cloned();
        let item = if let Some(item) = saved {
            item
        } else {
            if self.shared.pulling.get() {
                return Err(Error::new(
                    Kind::RuntimeError,
                    "cannot re-enter the tee iterator",
                ));
            }
            if index == 0 {
                block
                    .items
                    .borrow_mut()
                    .try_reserve_exact(BLOCK_SIZE)
                    .map_err(|_| Error::allocation())?;
            }
            // Do not borrow the buffer across the upstream call: reentrant
            // readers can consume cached items and must see the active pull.
            self.shared.pulling.set(true);
            let result = next_item(self.shared.source.as_ref().expect("live tee source"));
            self.shared.pulling.set(false);
            let Some(item) = result? else { return Ok(None) };
            block.items.borrow_mut().push(item.clone());
            item
        };
        self.index.set(index + 1);
        Ok(Some(item))
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
                .write(Rc::into_raw(fork));
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
        assert!(matches!(surviving.next(), Ok(None)));
    }
}
