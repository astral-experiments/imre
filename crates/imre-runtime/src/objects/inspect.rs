use super::{Object, Rc, Storage, StorageCell};

fn plain() -> Rc<Object> {
    Rc::new(Object(StorageCell::new(Storage::Plain)))
}

// The native object runtime and its module registry are thread-confined.
// Keep one owner in the registry; every returned handle owns a separate reference.
thread_local! { static SENTINEL: Rc<Object> = plain(); }
thread_local! {
    static STR_METHODS: [Rc<Object>; 4] = std::array::from_fn(|index|
        Rc::new(Object(StorageCell::new(Storage::StrDescriptor(index as u32)))));
}

pub(super) fn descriptor_name(index: u32) -> &'static str {
    ["lower", "strip", "lstrip", "rstrip"][index as usize]
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_plain_object_new() -> *const Object {
    Rc::into_raw(plain())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_inspect_sentinel() -> *const Object {
    SENTINEL.with(|value| Rc::into_raw(Rc::clone(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_inspect_str_descriptor(index: u32) -> *const Object {
    STR_METHODS.with(|values| {
        let descriptor = values
            .get(index as usize)
            .unwrap_or_else(|| super::fail("SystemError", "invalid str descriptor"));
        Rc::into_raw(Rc::clone(descriptor))
    })
}
