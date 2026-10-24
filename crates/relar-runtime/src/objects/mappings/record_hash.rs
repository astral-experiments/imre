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

/// # Safety
/// The caller roots one immutable native string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_string_hash(value: *const Str) -> usize {
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
        // SAFETY: allocation initializes the private cache word to zero.
        if unsafe { cache.read() } != 0 {
            return Ok(());
        }
        let mut text = String::new();
        // SAFETY: projected records always carry a static dataclass descriptor.
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
            // SAFETY: every projected field is an initialized immutable JSON object.
            let field = unsafe { object(*record.add(index) as *const Object) };
            if index != 0 {
                text.push('\n');
            }
            // SAFETY: the rooted immutable JSON view cannot mutate while rendering.
            unsafe {
                field.0.with(|storage| {
                    let Storage::Json(view) = storage else {
                        unreachable!("checked JSON projection")
                    };
                    view.toml_table(name, &mut text)
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
pub unsafe extern "C" fn relar_prepare_record_key(record: *const usize) {
    // SAFETY: the compiler established the immutable key shape.
    crate::finish(unsafe { prepare(record) }, ());
}

/// # Safety
/// The caller roots an exact frozen dataclass with a validated TOML hash projection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_projected_hash(record: *const usize) -> usize {
    // SAFETY: preparation initializes the extra word before the successful read.
    crate::finish(
        unsafe {
            prepare(record)
                .map(|()| integer((*record.add(descriptor(record).length)).wrapping_sub(1)))
        },
        0,
    )
}
