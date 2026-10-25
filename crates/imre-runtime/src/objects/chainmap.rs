//! A ChainMap owns its public list of dictionaries; the list and each
//! dictionary can be shared and mutated through independent references.

use super::words::WordKind;
use super::*;

impl Storage {
    fn chainmap(&self) -> &crate::ObjectOwner {
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
    let maps = unsafe { crate::ObjectOwner::acquire_word(maps as usize) };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::ChainMap(maps),
    ))))
}

/// # Safety
/// `value` is a rooted ChainMap. The result owns a reference to its actual list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_maps(value: *const Object) -> *const Object {
    // SAFETY: the receiver stays alive until the list is retained.
    crate::ObjectOwner::into_raw(unsafe {
        object(value)
            .0
            .with(|s| crate::ObjectOwner::clone(s.chainmap()))
    })
}

/// # Safety
/// Both inputs are rooted and the list elements have the ChainMap's dictionary shape.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_set_maps(value: *const Object, maps: *const Object) {
    // SAFETY: acquire the new list before replacing the old one, including self-assignment.
    let maps = unsafe { crate::ObjectOwner::acquire_word(maps as usize) };
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
        with_layers(value, |maps| {
            maps.iter().any(|&word| {
                object(word as *const Object)
                    .0
                    .with(|d| !d.dictionary().is_empty())
            })
        })
    })
}

/// Visit the current list with all its dictionaries rooted. These sealed
/// mappings hash native keys and do not run a user callback while borrowed.
unsafe fn with_layers<T>(value: *const Object, f: impl FnOnce(&[usize]) -> T) -> T {
    // SAFETY: the caller roots the ChainMap and its list for the whole visit.
    unsafe {
        object(value)
            .0
            .with(|s| s.chainmap().0.with(|maps| f(&maps.sequence().values)))
    }
}

/// # Safety
/// `value` is a rooted ChainMap and `key` has the declared dictionary key kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_contains(value: *const Object, key: usize) -> u32 {
    // SAFETY: each pointer belongs to the live dictionary list. Containment
    // never invokes a missing-key policy or inserts a default.
    crate::finish(
        unsafe { with_layers(value, |maps| contains(maps, key)) }.map(u32::from),
        0,
    )
}

/// Probe all layers without invoking a missing-key policy.
unsafe fn contains(maps: &[usize], key: usize) -> Result<bool, Error> {
    for &map in maps {
        // SAFETY: the visit roots each dictionary and the key.
        if unsafe { mappings::contains_key(map as *const Object, key) }? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Subscribe to each layer in order, including its Counter/defaultdict policy.
unsafe fn lookup(maps: &[usize], key: usize) -> Result<Option<OwnedWord>, Error> {
    for &map in maps {
        // SAFETY: the visit roots this dictionary until its result is acquired.
        if let Some(value) = unsafe { mappings::subscript(map as *const Object, key) }? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// # Safety
/// The ChainMap and key are rooted and `kind` describes the incoming key.
/// Success transfers a value owner; failure leaves a pending Python exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_getitem(
    value: *const Object,
    key: usize,
    kind: u32,
) -> usize {
    // SAFETY: the layer list roots each dictionary until its result is acquired.
    let result = unsafe {
        with_layers(value, |maps| {
            lookup(maps, key)?.ok_or_else(|| Error::key(WordKind::new(kind).acquire(key)))
        })
    };
    match result {
        Ok(value) => value.into_word(),
        Err(error) => error.returning(0),
    }
}

/// # Safety
/// All inputs are rooted with exact kinds; `out` is an independent writable
/// word. A hit transfers its value owner. A miss leaves the slot untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_try_get(
    value: *const Object,
    key: usize,
    out: *mut usize,
) -> u32 {
    // SAFETY: containment precedes subscription, as it does for ChainMap.get.
    // A missing factory in an earlier layer can still win over a later hit.
    let result = unsafe {
        with_layers(value, |maps| {
            if contains(maps, key)? {
                lookup(maps, key)
            } else {
                Ok(None)
            }
        })
    };
    match result {
        Ok(Some(value)) => {
            // SAFETY: a hit initialized an independent owned word.
            unsafe { out.write(value.into_word()) };
            1
        }
        Ok(None) => 0,
        Err(error) => error.returning(0),
    }
}

/// # Safety
/// The ChainMap, key, and new value are rooted and have the declared shape.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chainmap_setitem(value: *const Object, key: usize, word: usize) {
    // SAFETY: only the first dictionary is modified. An explicitly empty
    // public maps list raises the same IndexError as indexing that list.
    let result = unsafe {
        with_layers(value, |maps| {
            let first = maps.first().ok_or_else(Error::index)?;
            mappings::dict_set(*first as *const Object, key, word)
        })
    };
    crate::finish(result, ());
}
