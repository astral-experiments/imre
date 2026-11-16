//! Bounded writable spans for private byte outputs selected by the native emitter.

use super::{ByteOutput, Bytes, Object, Storage, imre_byte_output_finish};
use crate::objects;

/// A non-owning caller-local cursor into the private buffer's spare capacity.
#[repr(C)]
pub struct ByteOutputCursor {
    next: *mut u8,
    remaining: usize,
    bits: usize,
}

impl ByteOutput {
    /// An optional empty-buffer reservation cannot change the following append.
    fn reserve(&mut self, capacity: usize) {
        if self.bytes.is_empty() && self.bytes.capacity() < capacity {
            let _ = self.bytes.try_reserve_exact(capacity);
        }
    }

    /// Borrow the next writable span while the generated code owns the output.
    fn window(&mut self) -> ByteOutputCursor {
        ByteOutputCursor {
            next: self.bytes.spare_capacity_mut().as_mut_ptr().cast(),
            remaining: self.bytes.capacity().min(objects::MAX_LIST_LEN) - self.bytes.len(),
            bits: 0,
        }
    }

    /// Publish the cursor's initialized bytes and deferred range check.
    ///
    /// # Safety
    /// The caller kept this private allocation fixed while the cursor was open.
    unsafe fn publish(&mut self, cursor: &ByteOutputCursor) {
        if cursor.next.is_null() {
            return;
        }
        let len = self.bytes.capacity().min(objects::MAX_LIST_LEN) - cursor.remaining;
        // SAFETY: generated code initialized all consumed spare slots.
        unsafe { self.bytes.set_len(len) };
        self.invalid |= cursor.bits & !0x1fe != 0;
    }
}

/// Reserve the next range's capacity and return its new writable span.
///
/// # Safety
/// Root the private output and range operands. The cursor belongs to this output.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_reserve_window(
    output: *const Object,
    start: usize,
    stop: usize,
    step: usize,
    per_iteration: usize,
    cursor: *mut ByteOutputCursor,
) {
    if output.is_null() {
        return;
    }
    // SAFETY: the compiler retains the same rooted operands as the range loop.
    let Some(trips) = (unsafe { crate::small_range_trips(start, stop, step) }) else {
        return;
    };
    let capacity = trips.saturating_mul(per_iteration).min(256);
    // SAFETY: only generated code accesses this private output and cursor.
    unsafe {
        objects::object(output).0.with_mut(|storage| {
            let Storage::ByteOutput(output) = storage else {
                unreachable!()
            };
            output.publish(&*cursor);
            output.reserve(capacity);
            cursor.write(output.window());
        })
    }
}

/// Grow an exhausted span, preserving the prefix if a two-word append fails.
///
/// # Safety
/// Root the private output and integer words. Count is one or two. The cursor
/// belongs to the output, which has stayed fixed since the last reservation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_grow_window(
    output: *const Object,
    first: usize,
    second: usize,
    count: usize,
    cursor: *mut ByteOutputCursor,
) -> i32 {
    // SAFETY: the caller owns this buffer and every initialized cursor slot.
    unsafe {
        objects::object(output).0.with_mut(|storage| {
            let Storage::ByteOutput(output) = storage else {
                unreachable!()
            };
            output.publish(&*cursor);
            let status = if count == 1 {
                output.grow_and_append([first])
            } else {
                output.grow_and_append([first, second])
            };
            cursor.write(output.window());
            status
        })
    }
}

/// Finish the sole conversion after publishing the final initialized span.
///
/// # Safety
/// Root the private output and its matching cursor through the conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_finish_window(
    output: *const Object,
    cursor: *const ByteOutputCursor,
) -> *const Bytes {
    // SAFETY: the compiler retains this output until conversion returns.
    unsafe {
        objects::object(output).0.with_mut(|storage| {
            let Storage::ByteOutput(output) = storage else {
                unreachable!()
            };
            output.publish(&*cursor);
        });
        imre_byte_output_finish(output)
    }
}
