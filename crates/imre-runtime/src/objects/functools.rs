//! Native identities for the functools placeholder and its class.

use super::{Object, Rc, Storage, StorageCell};

thread_local! {
    static PLACEHOLDERS: [Rc<Object>; 2] = std::array::from_fn(|index|
        Rc::new(Object(StorageCell::new(Storage::FunctoolsPlaceholder { class: index != 0 }))));
}

/// Each load owns a reference to the same value or class in this interpreter.
#[unsafe(no_mangle)]
pub extern "C" fn imre_functools_placeholder(kind: u32) -> *const Object {
    PLACEHOLDERS.with(|objects| {
        let value = objects
            .get(kind as usize)
            .unwrap_or_else(|| super::fail("SystemError", "invalid Placeholder kind"));
        Rc::into_raw(Rc::clone(value))
    })
}
