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

#[repr(C)]
pub struct CoroutineTarget {
    adapter: *const (),
    // Negative for leaf functions. A partial retains its target in this frame slot.
    slot: isize,
    marker: *const u8,
}

/// # Safety
/// The caller roots a two-word callback. The immutable table is terminated by
/// a null adapter and describes only frames emitted by this executable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_inspect_coroutine_fn(
    mut function: *const *const (),
    targets: *const CoroutineTarget,
) -> bool {
    loop {
        // SAFETY: each partial owns its target and the original callback is rooted.
        let adapter = unsafe { *function };
        let mut entry = targets;
        loop {
            // SAFETY: a sentinel follows the last entry.
            let target = unsafe { &*entry };
            if target.adapter.is_null() {
                return false;
            }
            if target.adapter == adapter {
                if target.slot < 0 {
                    // SAFETY: non-null markers are bytes in the generated program.
                    return target.slot == -1
                        || unsafe { target.marker.as_ref() }.is_some_and(|&mark| mark != 0);
                }
                // SAFETY: the second word owns the compiler-described capture frame.
                function = unsafe {
                    *(*function.add(1))
                        .cast::<*const *const ()>()
                        .add(target.slot as usize)
                };
                break;
            }
            // SAFETY: the current entry was not the sentinel.
            entry = unsafe { entry.add(1) };
        }
    }
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
