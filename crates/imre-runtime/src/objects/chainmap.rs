//! A ChainMap owns its public list of dictionaries; the list and each
//! dictionary can be shared and mutated through independent references.

use super::*;

impl Storage {
    fn chainmap(&self) -> &Rc<Object> {
        let Self::ChainMap(maps) = self else {
            unreachable!("expected a ChainMap")
        };
        maps
    }
}

/// # Safety
/// `maps` is a rooted list of dictionaries with the ChainMap's concrete shape.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_new(maps: *const Object) -> *const Object {
    // SAFETY: the caller keeps the input list alive until its owner is acquired.
    let maps = unsafe { acquire_rc(maps as usize) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ChainMap(maps)))))
}

/// # Safety
/// `value` is a rooted ChainMap. The result owns a reference to its actual list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_maps(value: *const Object) -> *const Object {
    // SAFETY: the receiver stays alive until the list is retained.
    Rc::into_raw(unsafe { object(value).0.with(|s| Rc::clone(s.chainmap())) })
}

/// # Safety
/// Both inputs are rooted and the list elements have the ChainMap's dictionary shape.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_set_maps(value: *const Object, maps: *const Object) {
    // SAFETY: acquire the new list before replacing the old one, including self-assignment.
    let maps = unsafe { acquire_rc(maps as usize) };
    // SAFETY: the receiver is rooted and no borrowed list reference escapes.
    let old = unsafe {
        object(value).0.with_mut(|s| {
            let Storage::ChainMap(old) = s else {
                unreachable!("expected a ChainMap")
            };
            std::mem::replace(old, maps)
        })
    };
    drop(old);
}

/// # Safety
/// `value` is a rooted ChainMap whose list holds only rooted native dictionaries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_truth(value: *const Object) -> u32 {
    // SAFETY: no user code runs during a dictionary length check. Each enclosing
    // owner roots the next allocation while its borrowed storage is inspected.
    u32::from(unsafe {
        object(value).0.with(|s| {
            s.chainmap().0.with(|maps| {
                maps.sequence().values.iter().any(|&word| {
                    object(word as *const Object)
                        .0
                        .with(|d| !d.dictionary().is_empty())
                })
            })
        })
    })
}
