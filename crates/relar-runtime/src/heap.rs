//! Select one allocator for the process, including when native code is sanitized.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, Ordering};

use tikv_jemallocator::Jemalloc;

const UNSELECTED: u8 = 0;
const SYSTEM: u8 = 1;
const JEMALLOC: u8 = 2;
static BACKEND: AtomicU8 = AtomicU8::new(UNSELECTED);

pub(super) fn select_jemalloc() {
    // An earlier runtime allocation can already have selected the system heap.
    // All later reallocations and frees must stay with that original allocator.
    let _ = BACKEND.compare_exchange(UNSELECTED, JEMALLOC, Ordering::Relaxed, Ordering::Relaxed);
}

fn backend() -> &'static (dyn GlobalAlloc + Sync) {
    // Publishing an allocator choice shares no other state. Both backends are
    // static, and every allocation sees the same choice once it has been set.
    let use_jemalloc = match BACKEND.load(Ordering::Relaxed) {
        JEMALLOC => true,
        SYSTEM => false,
        _ => {
            BACKEND.compare_exchange(UNSELECTED, SYSTEM, Ordering::Relaxed, Ordering::Relaxed)
                == Err(JEMALLOC)
        }
    };
    if use_jemalloc { &Jemalloc } else { &System }
}

struct Heap;

#[global_allocator]
static HEAP: Heap = Heap;

// SAFETY: the backend never changes after the first selection or allocation.
// Selection uses only atomic operations and does not allocate.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's layout to the selected allocator.
        unsafe { backend().alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's layout to the selected allocator.
        unsafe { backend().alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the selected allocator owns this allocation and its layout.
        unsafe { backend().dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the original allocator receives its pointer and layout.
        unsafe { backend().realloc(ptr, layout, new_size) }
    }
}
