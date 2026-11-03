//! String-valued GNU langinfo items, decoded in their owning locale category.

use super::{
    CStr, CString, OsStr, OsStrExt, decode_locale, with_ctype_locale, with_process_locale,
};

#[repr(C)]
pub(crate) struct Item {
    name: *const libc::c_char,
    pub(crate) value: libc::c_int,
    category: libc::c_int,
}

pub(crate) fn items() -> &'static [Item] {
    unsafe extern "C" {
        fn imre_langinfo_items(count: *mut usize) -> *const Item;
    }
    let mut count = 0;
    // SAFETY: the C table and all its names are immutable static storage.
    unsafe {
        let items = imre_langinfo_items(&mut count);
        std::slice::from_raw_parts(items, count)
    }
}

impl Item {
    pub(crate) fn name(&self) -> &'static str {
        // SAFETY: each name in the static table is a stringified C identifier.
        unsafe { CStr::from_ptr(self.name) }.to_str().unwrap()
    }

    pub(crate) fn read(&self) -> crate::Result<String> {
        with_process_locale(|| {
            // SAFETY: only string-valued items in the target's C table reach libc.
            // Copy its storage while holding the locale lock, before locale queries.
            let ptr = unsafe { libc::nl_langinfo(self.value) };
            if ptr.is_null() {
                return Ok(String::new());
            }
            // SAFETY: a non-null value for a string item is NUL-terminated and the
            // locale lock keeps its storage valid while it is read.
            let first = unsafe { CStr::from_ptr(ptr) };
            let multiple = matches!(self.value, libc::ALT_DIGITS | libc::ERA);
            if first.is_empty()
                || self.category == libc::LC_CTYPE
                || (!multiple && first.to_bytes().is_ascii())
            {
                return decode_locale(first);
            }
            let raw = if multiple {
                // glibc separates these entries with NUL, whereas POSIX and Python
                // separate them with semicolons. ALT_DIGITS contains at most 100.
                let limit = if self.value == libc::ALT_DIGITS {
                    100
                } else {
                    usize::MAX
                };
                let mut bytes = Vec::new();
                let mut cursor = ptr;
                for _ in 0..limit {
                    // SAFETY: glibc's list ends with an empty entry, or at the limit.
                    let part = unsafe { CStr::from_ptr(cursor) };
                    if part.is_empty() {
                        break;
                    }
                    if !bytes.is_empty() {
                        bytes.push(b';');
                    }
                    bytes.extend_from_slice(part.to_bytes());
                    // SAFETY: part spans this entry through its NUL, so the new pointer
                    // stays within the glibc list or one byte past its last entry.
                    cursor = unsafe { cursor.add(part.to_bytes_with_nul().len()) };
                }
                CString::new(bytes).unwrap()
            } else {
                first.to_owned()
            };
            // SAFETY: querying the category while locked returns a borrowed C string.
            let name = unsafe { libc::setlocale(self.category, std::ptr::null()) };
            if !name.is_null() {
                // SAFETY: setlocale returned a non-null, NUL-terminated name. The locale
                // lock protects it until with_ctype_locale copies its bytes.
                let name = unsafe { CStr::from_ptr(name) };
                if let Some(result) =
                    with_ctype_locale(OsStr::from_bytes(name.to_bytes()), || decode_locale(&raw))
                {
                    return result;
                }
            }
            decode_locale(&raw)
        })
    }
}
