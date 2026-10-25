use super::{Object, Storage, StorageCell};

fn plain() -> crate::ObjectOwner {
    crate::ObjectOwner::new(Object(StorageCell::new(Storage::Plain)))
}

// The native object runtime and its module registry are thread-confined.
// Keep one owner in the registry; every returned handle owns a separate reference.
thread_local! { static SENTINEL: crate::ObjectOwner = plain(); }
const STR_METHOD_NAMES: [&str; 5] = ["lower", "strip", "lstrip", "rstrip", "isalnum"];
thread_local! {
    static STR_METHODS: [crate::ObjectOwner; STR_METHOD_NAMES.len()] = std::array::from_fn(|index|
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::StrDescriptor(index as u32)))));
}

pub(super) fn descriptor_name(index: u32) -> &'static str {
    STR_METHOD_NAMES[index as usize]
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_plain_object_new() -> *const Object {
    crate::ObjectOwner::into_raw(plain())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_inspect_sentinel() -> *const Object {
    SENTINEL.with(|value| crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_inspect_str_descriptor(index: u32) -> *const Object {
    STR_METHODS.with(|values| {
        let descriptor = values
            .get(index as usize)
            .unwrap_or_else(|| super::fail("SystemError", "invalid str descriptor"));
        crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(descriptor))
    })
}
