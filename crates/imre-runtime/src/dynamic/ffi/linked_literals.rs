use super::{Context, Id, NONE, finish, load_linked_literal};
use crate::dynamic::value::ConstantSlot;

/// Borrow a linked constant, or return the None handle for a cache miss.
///
/// # Safety
/// The active context owns the slot's immutable root throughout the calling frame.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_cached_handle(ctx: *const Context, slot: *const u8) -> Id {
    // SAFETY: generated code uses its live context and address-significant constant slot.
    unsafe { &*ctx }
        .heap
        .constant_at(&ConstantSlot::Linked(slot.addr()))
        .unwrap_or(NONE)
}

/// Return the source unit's immutable Python literal, decoding its linked bytes on a miss.
///
/// # Safety
/// The caller supplies a live context, a distinct linked slot, and `len` immutable bytes.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_linked_string(
    ctx: *mut Context,
    slot: *const u8,
    kind: i32,
    data: *const u8,
    len: usize,
) -> Id {
    // SAFETY: the caller passes the active frame's context.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let key = ConstantSlot::Linked(slot.addr());
    if let Some(value) = ctx.heap.constant_at(&key) {
        // The active code already roots this constant across operand cleanup and collection.
        return value;
    }
    // SAFETY: the emitter places the exact length of immutable data in this code unit.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let value = ctx.linked_constant(|ctx| load_linked_literal(ctx, kind, bytes));
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    // The ordinary literal loader has already rooted the result in this frame.
    match ctx.heap.cached_constant(key, |_| Ok(value)) {
        Ok(value) => value,
        Err(error) => finish(ctx, Err(error)),
    }
}
