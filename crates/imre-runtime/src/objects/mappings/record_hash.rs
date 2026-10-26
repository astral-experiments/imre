//! Prepare pure custom hashes before borrowing or changing a native hash table.

use super::*;
use crate::objects::records::descriptor;

/// Hashes are process-local. Python gives the empty string a zero hash and
/// reserves -1 for its error sentinel.
fn text_hash(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    match HASHER.hash_one(text) as isize {
        -1 => (-2_isize) as usize,
        h => h as usize,
    }
}

fn integer(hash: usize) -> usize {
    let hash = hash as isize as i64;
    Int::small(hash)
        .unwrap_or_else(|| Int::new(hash.into()))
        .into_raw()
}

fn try_integer(hash: usize) -> Result<usize, Error> {
    Int::try_from_i64(hash as isize as i64)
        .map(Int::into_raw)
        .map_err(|_| Error::allocation())
}

/// # Safety
/// The caller roots one immutable native string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_hash(value: *const Str) -> usize {
    // SAFETY: the caller keeps the string alive for the complete read.
    integer(text_hash(unsafe { &*value }))
}

/// # Safety
/// The record and all comparison fields are initialized, rooted and immutable.
unsafe fn prepare(record: *const usize) -> Result<(), Error> {
    // SAFETY: the root pins its static layout and all recursive field owners.
    let layout = unsafe { descriptor(record) };
    if layout.has_toml_hash() {
        // SAFETY: exactly these layouts own one extra zero-initialized cache word.
        let cache = unsafe { record.add(layout.length).cast_mut() };
        // SAFETY: this layout has an allocated cache slot and only initialized object fields.
        let immutable_cached = unsafe {
            cache.read() != 0
                && !(0..layout.length).any(|i| {
                    object(*record.add(i) as *const Object)
                        .0
                        .with(|storage| matches!(storage, Storage::Document(_)))
                })
        };
        if immutable_cached {
            return Ok(());
        }
        let mut text = String::new();
        // SAFETY: projected records always have a static dataclass descriptor.
        let semantics = unsafe { &*layout.semantics };
        for index in 0..layout.length {
            // SAFETY: the checked projection lists every exact JSON field and its UTF-8 name.
            let field = unsafe { &*semantics.fields.add(index) };
            // SAFETY: the compiler emits UTF-8 field names with their byte lengths.
            let name = unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    field.name,
                    field.name_len,
                ))
            };
            // SAFETY: every projected field is an initialized JSON/document object.
            let field = unsafe { object(*record.add(index) as *const Object) };
            if index != 0 {
                text.try_reserve(1).map_err(|_| Error::allocation())?;
                text.push('\n');
            }
            // SAFETY: the rooted immutable JSON view cannot mutate while rendering.
            unsafe {
                field.0.with(|storage| match storage {
                    Storage::Json(view) => view.toml_table(name, &mut text),
                    Storage::Document(value) => value.view()?.toml_table(name, &mut text),
                    _ => unreachable!("checked TOML projection"),
                })
            }?;
        }
        // SAFETY: only this single-threaded record runtime reads or writes the private word.
        unsafe { cache.write(text_hash(&text).wrapping_add(1)) };
    } else {
        // SAFETY: the static descriptor or null outlives the rooted record.
        let semantics = unsafe { layout.semantics.as_ref() };
        for i in 0..layout.length {
            // SAFETY: descriptors and initialized field words remain rooted during recursion.
            unsafe {
                if semantics.is_some_and(|s| s.kind == 0 && (*s.fields.add(i)).flags & 1 == 0) {
                    continue;
                }
                let kind = WordKind::new(*layout.kinds.add(i));
                if kind.raw() & !NULLABLE == 6 {
                    let child = kind.read_record_field(record.add(i));
                    if child != 0 {
                        prepare(child as *const usize)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// # Safety
/// The caller roots the initialized, exact tuple or frozen record until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_prepare_record_key(record: *const usize) {
    // SAFETY: the compiler established the immutable key shape.
    crate::finish(unsafe { prepare(record) }, ());
}

/// # Safety
/// The caller roots an exact scalar struct or frozen dataclass with a validated TOML hash projection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_projected_hash(record: *const usize) -> usize {
    // SAFETY: a value struct owns only initialized scalar words and static metadata.
    unsafe {
        let layout = descriptor(record);
        if layout.is_value_struct() {
            let mut hash = 0xcbf29ce484222325_u64;
            for i in 0..layout.length {
                let kind = WordKind::new(*layout.kinds.add(i));
                let word = kind.read_record_field(record.add(i));
                let key = if kind.raw() == 7 {
                    crate::objects::structs::float_key(word)
                } else {
                    word
                };
                hash = (hash ^ key as u64).wrapping_mul(0x100000001b3);
            }
            return crate::finish(try_integer((hash & i64::MAX as u64) as usize), 0);
        }
    }
    // SAFETY: preparation initializes the extra word before the successful read.
    crate::finish(
        unsafe {
            prepare(record).and_then(|()| {
                try_integer((*record.add(descriptor(record).length)).wrapping_sub(1))
            })
        },
        0,
    )
}
