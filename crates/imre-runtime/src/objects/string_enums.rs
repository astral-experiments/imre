//! Borrowed compiler-emitted member metadata for recursive value representations.

use crate::StrOwner;
use std::cell::Cell;
use std::ptr;

use super::{Str, exceptions::string_repr};

// Class name, member name and member value each point to a pooled string root.
type Member = [*const *const Str; 3];
thread_local! { static PLAIN: Cell<*const u8> = const { Cell::new(ptr::null()) }; }
thread_local! { static MEMBERS: Cell<(*const Member, usize)> = const { Cell::new((ptr::null(), 0)) }; }

// Optional parallel rows share the canonical tags and lifetime of MEMBERS.
#[repr(C)]
pub struct HttpMember {
    code: usize,
    phrase: *const *const Str,
    description: *const *const Str,
}

thread_local! { static HTTP: Cell<*const HttpMember> = const { Cell::new(ptr::null()) }; }

/// Installs optional HTTP fields for the current enum table.
///
/// # Safety
/// A non-null table must have a row per registered member and live until cleared.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_http_enum_table(rows: *const HttpMember) {
    HTTP.set(rows);
}

// None is an ordinary enum; Some(0) is HTTPMethod; HTTPStatus stores its tagged int.
fn http_code(tag: usize) -> Option<usize> {
    let rows = HTTP.get();
    if rows.is_null() {
        return None;
    }
    assert!((1..=MEMBERS.get().1).contains(&tag));
    // SAFETY: the registered parallel table has the same index range as MEMBERS.
    let row = unsafe { &*rows.add(tag - 1) };
    (!row.description.is_null()).then_some(row.code)
}

/// Installs one module's read-only member table after its string pool is initialized.
///
/// # Safety
/// Every entry and its three pooled string owners must live and remain initialized
/// on the calling thread until this function clears the table with null and zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_enum_table(members: *const Member, length: usize) {
    MEMBERS.set((members, length));
    PLAIN.set(ptr::null());
}

/// Register whether each canonical member uses Enum or StrEnum for its display.
///
/// # Safety
/// The member table has the same lifetime as in `imre_str_enum_table`; `plain`
/// names a static byte for each member in that same order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_enum_display_table(
    members: *const Member,
    length: usize,
    plain: *const u8,
) {
    // SAFETY: both tables have length entries and remain live together.
    unsafe { imre_str_enum_table(members, length) };
    PLAIN.set(plain);
}

/// Format a retained member without losing its nominal identity or representation.
pub(super) fn write_display(out: &mut impl std::fmt::Write, tag: usize) -> std::fmt::Result {
    with_member(tag, |[class, name, value]| {
        let plain = PLAIN.get();
        // SAFETY: with_member checked this tag against the registered table length.
        if !plain.is_null() && unsafe { *plain.add(tag - 1) } != 0 {
            write!(out, "{class}.{name}")
        } else {
            out.write_str(value.as_str())
        }
    })
}

pub(super) fn class_name(tag: usize) -> String {
    with_member(tag, |[class, _, _]| class.to_string())
}

/// A StrEnum has string indexing and length; an ordinary Enum has neither.
pub(super) fn string_value(tag: usize) -> Option<crate::StrOwner> {
    with_member(tag, |[_, _, value]| {
        let plain = PLAIN.get();
        // SAFETY: with_member proved this index and roots the registered enum value.
        (http_code(tag).unwrap_or(0) == 0
            && (plain.is_null() || unsafe { *plain.add(tag - 1) } == 0))
            .then(|| unsafe { crate::StrOwner::acquire_word(std::ptr::from_ref(value) as usize) })
    })
}

fn with_member<R>(tag: usize, f: impl FnOnce([&Str; 3]) -> R) -> R {
    let (members, length) = MEMBERS.get();
    assert!((1..=length).contains(&tag), "invalid string enum tag");
    // SAFETY: the registered table and its three initialized string roots remain
    // alive through this nonreentrant call. The one-based index was checked above.
    f(unsafe { (*members.add(tag - 1)).map(|root| &**root) })
}

/// Selects the canonical member of an ordinary string-valued Enum by value.
///
/// # Safety
/// The caller roots the text and supplies a nonempty range in the installed member table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_enum_lookup(text: *const Str, first: usize, count: usize) -> usize {
    // SAFETY: the caller roots the immutable lookup value.
    let text = unsafe { &*text };
    for tag in first..first + count {
        if http_code(tag).unwrap_or(0) == 0 && with_member(tag, |[_, _, value]| value == text) {
            return tag;
        }
    }
    with_member(first, |[class, _, _]| {
        super::exceptions::Error::new(
            super::exceptions::Kind::ValueError,
            format!("{} is not a valid {class}", string_repr(text)),
        )
        .raise();
    });
    0
}

/// Return a class's canonical tag for a Python integer, or zero for a missing dictionary key.
/// Status codes are immediate Python integers, so a heap integer cannot compare equal.
#[unsafe(no_mangle)]
pub extern "C" fn imre_enum_find_int(word: usize, first: usize, count: usize) -> usize {
    (first..first + count)
        .find(|&tag| http_code(tag) == Some(word))
        .unwrap_or(0)
}

/// Looks up a rooted Python integer without equating its decimal text to a string.
///
/// # Safety
/// The caller roots the canonical integer and supplies the class's member range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_enum_lookup_int(word: usize, first: usize, count: usize) -> usize {
    let tag = imre_enum_find_int(word, first, count);
    if tag != 0 {
        return tag;
    }
    // SAFETY: the caller roots word, and int_str returns one owned string reference.
    let text = unsafe { StrOwner::from_raw(crate::imre_int_str(word)) };
    with_member(first, |[class, _, _]| {
        super::exceptions::Error::new(
            super::exceptions::Kind::ValueError,
            format!("{text} is not a valid {class}"),
        )
        .raise();
    });
    0
}

/// Formats an ordinary Enum using its canonical member name.
#[unsafe(no_mangle)]
pub extern "C" fn imre_enum_string(tag: usize) -> *const Str {
    with_member(tag, |[class, name, _]| {
        StrOwner::into_raw(StrOwner::new(Str::new(format!("{class}.{name}"))))
    })
}

pub(super) fn compare(left: usize, right: usize) -> std::cmp::Ordering {
    with_member(left, |a| with_member(right, |b| a[2].cmp(b[2])))
}

pub(super) fn write_repr(
    output: &mut super::values::ReprWriter<'_>,
    tag: usize,
) -> std::fmt::Result {
    use std::fmt::Write as _;
    with_member(tag, |[class, name, value]| {
        output.write_char('<')?;
        output.write_str(class)?;
        output.write_char('.')?;
        output.write_str(name)?;
        match http_code(tag) {
            Some(0) => {}
            Some(_) => {
                output.write_str(": ")?;
                output.write_str(value)?;
            }
            None => {
                output.write_str(": ")?;
                super::exceptions::write_quoted_repr(value, output)?;
            }
        }
        output.write_char('>')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::values::imre_value_repr;
    use crate::objects::words::{NULLABLE, WordKind};

    #[test]
    fn scalar_tags_own_no_roots_and_render_the_canonical_member() {
        let texts = ["Status", "FIRST", "z\n", "LAST", "a"].map(|s| StrOwner::new(Str::new(s)));
        let roots = texts.each_ref().map(StrOwner::as_ptr);
        let members = [
            [&roots[0], &roots[1], &roots[2]],
            [&roots[0], &roots[3], &roots[4]],
        ]
        .map(|fields| fields.map(std::ptr::from_ref));
        // SAFETY: the table, its root slots and their Rc owners live until explicit teardown.
        unsafe {
            imre_str_enum_table(members.as_ptr(), members.len());
            let kind = WordKind::new(10);
            drop(kind.acquire(1));
            assert!(texts.iter().all(|s| StrOwner::strong_count(s) == 1));
            assert!(compare(1, 2).is_gt());
            assert_eq!(
                *StrOwner::from_raw(imre_value_repr(1, 10)),
                "<Status.FIRST: 'z\\n'>"
            );
            assert_eq!(
                *StrOwner::from_raw(imre_value_repr(2, 10 | NULLABLE)),
                "<Status.LAST: 'a'>"
            );
            assert_eq!(
                *StrOwner::from_raw(imre_value_repr(0, 10 | NULLABLE)),
                "None"
            );
            imre_str_enum_table(ptr::null(), 0);
        }
    }
}
