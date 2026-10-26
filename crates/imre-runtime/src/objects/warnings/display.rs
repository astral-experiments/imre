//! Default warning display uses the public line cache before writing to the
//! destination. The writer catches OSError only; formatting failures propagate.
use super::super::{Kind, Object, Str, linecache, object, python_whitespace, records};
use crate::StrOwner;

/// # Safety
/// All strings, the tagged int and cache layout are live. A null line requests
/// lookup; a non-null line bypasses the cache, even when it is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_warning_format(
    message: *const Str,
    category: *const Str,
    filename: *const Str,
    number: *const Str,
    index: usize,
    supplied: *const Str,
    root: *const Str,
    newline: *const Str,
    environment: u32,
    layout: *const records::RecordDescriptor,
) -> *const Str {
    // SAFETY: caller roots the parameters; each linecache result owns its data.
    unsafe {
        let filename = &*filename;
        let mut text = format!(
            "{}:{}: {}: {}\n",
            filename.as_str(),
            (&*number).as_str(),
            (&*category).as_str(),
            (&*message).as_str()
        );
        // Unlike a supplied line, an omitted one consults the shared cache. The
        // upstream formatter suppresses source-lookup errors without losing the header.
        let cached = if supplied.is_null() {
            linecache::cached_lines(
                filename,
                None,
                (&*root).as_str(),
                &*newline,
                environment != 0,
                false,
                layout,
            )
            .ok()
            .and_then(|ls| linecache::line_at(&ls, index).ok().flatten())
        } else {
            None
        };
        if let Some(line) = supplied
            .as_ref()
            .or(cached.as_deref())
            .filter(|s| !s.is_empty())
        {
            text.push_str("  ");
            text.push_str(line.trim_matches(python_whitespace));
            text.push('\n');
        }
        StrOwner::into_raw(StrOwner::new(Str::new(text)))
    }
}

/// # Safety
/// The text is rooted and a non-null file is a native text stream (including
/// closed streams and StringIO). Null chooses the selected standard stream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_warning_write(file: *const Object, text: *const Str, stdout: u32) {
    // SAFETY: exact file and immutable text live until the synchronous write finishes.
    unsafe {
        let text = (&*text).as_str();
        let result = if file.is_null() {
            crate::sys::write_standard_stream(stdout == 0, text).map_err(super::super::errno_error)
        } else {
            object(file).0.with_mut(|s| s.file_mut().write(text))
        };
        if let Err(e) = result
            && !e.is_kind(Kind::OSError)
            && !e.is_kind(Kind::UnsupportedOperation)
        {
            e.raise();
        }
    }
}
