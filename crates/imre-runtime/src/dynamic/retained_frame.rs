//! Materialize live native values only when a registered activation unwinds.

use super::value::Id;
use super::{Context, Value};

/// Transfer a fresh integer list into the ordinary function scope exposed by traceback frames.
///
/// # Safety
/// The generated root owns the active scope. The UTF-8 name is live through this call. A null
/// pointer denotes an unbound slot. Otherwise the list is a live private local on its error
/// exit, with no observable aliases or subsequent uses apart from releasing its owner.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_native_frame_int_list(
    ctx: *mut Context,
    scope: Id,
    name: *const u8,
    len: usize,
    list: *const crate::List,
) {
    // SAFETY: the native caller keeps the list alive until its error cleanup.
    let Some(list) = (unsafe { list.as_ref() }) else {
        return;
    };
    // SAFETY: the caller transfers the contents of this private list. No Python call occurs
    // while accessing its storage. Its now-empty native owner is released by normal cleanup.
    let words = unsafe { list.0.with_mut(std::mem::take) };
    // SAFETY: the registered activation and its static UTF-8 name are live.
    let (ctx, name) = unsafe {
        (
            &mut *ctx,
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)),
        )
    };
    let items = words
        .into_iter()
        .map(|word| ctx.alloc(Value::Int(word)))
        .collect();
    let object = ctx.alloc(Value::List(items));
    // No temporary-root checkpoint or call to finish: either the registered or native
    // exception may already be pending. The ordinary frame keeps its existing scope.
    ctx.bind_local(scope, name, object);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::{Error, ffi};

    #[test]
    fn unbound_and_live_list_slots_preserve_the_original_exception() {
        let mut ctx = ffi::new_context(
            serde_json::json!({"paths": []}),
            std::iter::empty::<String>(),
        );
        let globals = ctx.scope("__main__", None);
        let scope = ctx.scope("__main__", Some(globals));
        ctx.pending = Some(Error::new("KeyboardInterrupt", "original"));
        let key = b"digits";
        // SAFETY: null represents a slot whose assignment has not completed.
        unsafe {
            imre_dyn_native_frame_int_list(
                &mut ctx,
                scope,
                key.as_ptr(),
                key.len(),
                std::ptr::null(),
            )
        };
        assert_eq!(ctx.scope_name(scope, "digits"), None);
        let list = crate::List::new(vec![
            crate::Int::small(3).unwrap(),
            crate::Int::small(2).unwrap(),
        ]);
        // SAFETY: the list is private and dies after this frame publication.
        unsafe { imre_dyn_native_frame_int_list(&mut ctx, scope, key.as_ptr(), key.len(), &list) };
        let value = ctx.scope_name(scope, "digits").unwrap();
        let Value::List(items) = ctx.heap.get(value) else {
            panic!("expected a Python list")
        };
        let digits: Vec<_> = items
            .iter()
            .map(|&id| {
                let Value::Int(word) = ctx.heap.get(id) else {
                    panic!("expected an int")
                };
                word.as_small().unwrap()
            })
            .collect();
        assert_eq!(digits, [3, 2]);
        let pending = ctx.pending.as_ref().unwrap();
        assert_eq!(pending.kind, "KeyboardInterrupt");
        assert_eq!(pending.message, "original");
    }
}
