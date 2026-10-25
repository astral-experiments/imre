//! Whole-file text conversion uses the same descriptor, buffering, and close path as binary I/O.

use super::super::codecs::Codec;
use super::{Error, Int, Kind, Object, Rc, Result, Str, finish, object, with_path_file};

fn options(encoding: Option<&str>, newline: Option<&str>) -> Result<Codec> {
    if let Some(value) = newline
        && !matches!(value, "" | "\n" | "\r" | "\r\n")
    {
        return Err(Error::new(
            Kind::ValueError,
            format!("illegal newline value: {value}"),
        ));
    }
    match encoding {
        None | Some("locale") => Ok(Codec::Utf8 { signature: false }),
        Some(name) => Codec::lookup(name),
    }
}

// An optional native string is an owned pointer word, or zero for None.
unsafe fn string_option<'a>(word: usize) -> Option<&'a str> {
    // SAFETY: the caller roots each non-null string until its I/O operation returns.
    unsafe { (word as *const Str).as_ref() }.map(Str::as_str)
}

pub(super) fn read(
    path: &str,
    encoding: Option<&str>,
    errors: Option<&str>,
    newline: Option<&str>,
) -> Result<Str> {
    with_path_file(path, 0, true, |file| {
        let codec = options(encoding, newline)?;
        let data = file.input()?.read_bytes(None)?;
        let text = codec.decode(&data, errors)?;
        let text = if newline.is_none() && text.contains('\r') {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text
        };
        Ok(Str::from(text))
    })
}

/// # Safety
/// Root the path and each nonzero optional string through the read and close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_read_text_options(
    path: *const Object,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> *const Str {
    // SAFETY: the path and options remain immutably rooted for synchronous I/O.
    let (path, encoding, errors, newline) = unsafe {
        (
            object(path),
            string_option(encoding),
            string_option(errors),
            string_option(newline),
        )
    };
    // SAFETY: no language callback can mutate the rooted path during file I/O.
    let result = unsafe {
        path.0
            .with(|storage| read(storage.path(), encoding, errors, newline))
    };
    finish(
        result.map(|text| Rc::into_raw(Rc::new(text))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root the path, text and each nonzero optional string through the write and close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_text_options(
    path: *const Object,
    text: *const Str,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> usize {
    // SAFETY: the path, data and options remain immutably rooted for synchronous I/O.
    let (path, text, encoding, errors, newline) = unsafe {
        (
            object(path),
            &*text,
            string_option(encoding),
            string_option(errors),
            string_option(newline),
        )
    };
    // SAFETY: no language callback can mutate the rooted path during file I/O.
    let result = unsafe {
        path.0.with(|storage| {
            with_path_file(storage.path(), 1, true, |file| {
                let codec = options(encoding, newline)?;
                let converted;
                let value = if let Some(replacement @ ("\r" | "\r\n")) = newline {
                    converted = text.replace('\n', replacement);
                    &converted
                } else {
                    text.as_str()
                };
                file.write_bytes(&codec.encode(value, errors)?)?;
                Ok(Int::from_usize(text.chars().count()).into_raw())
            })
        })
    };
    finish(result, 0)
}

/// # Safety
/// The diagnostic string stays rooted until the pending TypeError owns its message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_type_error(message: *const Str) -> usize {
    // SAFETY: the compiler supplies an immutable rooted string.
    finish(
        Err(Error::new(Kind::TypeError, unsafe { &*message }.as_str())),
        0,
    )
}

/// # Safety
/// The data is a rooted optional str/bytes union; tag zero is str and tag one is bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_bytes_union(path: *const Object, data: usize) -> usize {
    if data & 7 == 1 {
        // SAFETY: removing the tag restores the rooted bytes pointer.
        return unsafe {
            super::super::paths::imre_path_write_bytes(path, (data & !7) as *const _)
        };
    }
    let actual = if data == 0 { "NoneType" } else { "str" };
    finish(
        Err(Error::new(
            Kind::TypeError,
            format!("memoryview: a bytes-like object is required, not '{actual}'"),
        )),
        0,
    )
}

/// # Safety
/// The path, optional str/bytes word, and text options stay rooted through this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_text_union(
    path: *const Object,
    data: usize,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> usize {
    if data != 0 && data & 7 == 0 {
        // SAFETY: the nonzero untagged word is the rooted string.
        return unsafe {
            imre_path_write_text_options(path, data as *const Str, encoding, errors, newline)
        };
    }
    let actual = if data == 0 { "NoneType" } else { "bytes" };
    finish(
        Err(Error::new(
            Kind::TypeError,
            format!("data must be str, not {actual}"),
        )),
        0,
    )
}
