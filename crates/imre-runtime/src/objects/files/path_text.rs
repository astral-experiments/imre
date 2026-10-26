//! Whole-file text conversion uses the same descriptor, buffering, and close path as binary I/O.

use super::super::codecs::{Codec, option_error};
use super::super::{Storage, StorageCell};
use super::{Error, Int, Kind, Object, Result, Str, finish, object, with_path_file};
use crate::{ObjectOwner, StrOwner};

pub(super) fn output_newlines(text: &str, replacement: &str) -> Result<String> {
    let extra = if replacement.len() == 2 {
        text.bytes().filter(|&byte| byte == b'\n').count()
    } else {
        0
    };
    let mut result = String::new();
    let len = text
        .len()
        .checked_add(extra)
        .ok_or_else(Error::allocation)?;
    result
        .try_reserve_exact(len)
        .map_err(|_| Error::allocation())?;
    for part in text.split_inclusive('\n') {
        if let Some(line) = part.strip_suffix('\n') {
            result.push_str(line);
            result.push_str(replacement);
        } else {
            result.push_str(part);
        }
    }
    Ok(result)
}

fn options(encoding: Option<&str>, newline: Option<&str>) -> Result<Codec> {
    if let Some(value) = newline
        && !matches!(value, "" | "\n" | "\r" | "\r\n")
    {
        return Err(option_error(
            Kind::ValueError,
            "illegal newline value: ",
            value,
            "",
        ));
    }
    Codec::lookup(crate::sys::file_encoding(encoding))
}

// An optional native string is an owned pointer word, or zero for None.
unsafe fn string_option<'a>(word: usize) -> Option<&'a str> {
    // SAFETY: the caller roots each non-null string until its I/O operation returns.
    unsafe { (word as *const Str).as_ref() }.map(Str::as_str)
}

/// Validate string arguments before opening, then let TextIOWrapper validation
/// see the actual descriptor. A bad codec can therefore leave a truncated file.
fn open_text_output(
    path: &StrOwner,
    mode: &str,
    encoding: Option<&str>,
    errors: Option<StrOwner>,
    newline: Option<&str>,
) -> Result<*const Object> {
    if [encoding, errors.as_deref().map(Str::as_str), newline]
        .into_iter()
        .flatten()
        .any(|option| option.contains('\0'))
    {
        return Err(Error::new(Kind::ValueError, "embedded null character"));
    }
    let mode = match mode {
        "w" | "wt" | "tw" => 1,
        "a" | "at" | "ta" => 2,
        "x" | "xt" | "tx" => 3,
        _ => unreachable!("compiler checked the text output mode"),
    };
    let spelling = crate::sys::file_encoding(encoding);
    let policy = errors.as_deref().map_or("strict", Str::as_str);
    let mut state =
        super::open_state_with_encoding(path, mode, false, false, Some((spelling, policy)))?;
    let codec = options(Some(spelling), newline)?;
    let Some(super::Stream::Write(writer)) = state.stream.as_mut() else {
        unreachable!()
    };
    writer.set_encoding(codec, errors, newline);
    let state = super::BoxedFile::try_new(state).map_err(|_| Error::allocation())?;
    ObjectOwner::try_new(Object(StorageCell::new(Storage::File(state))))
        .map(ObjectOwner::into_raw)
        .map_err(|_| Error::allocation())
}

/// # Safety
/// Root the native path object, literal mode and each nonzero string until open returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_open_text_output(
    path: *const Object,
    mode: *const Str,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> *const Object {
    // SAFETY: the caller roots every operand for this synchronous operation.
    let (path, mode, encoding, errors, newline) = unsafe {
        (
            object(path),
            &*mode,
            string_option(encoding),
            (errors != 0).then(|| StrOwner::acquire_word(errors)),
            string_option(newline),
        )
    };
    // SAFETY: opening a file cannot re-enter or mutate the rooted path.
    let result = unsafe {
        path.0
            .with(|s| open_text_output(s.path_text(), mode, encoding, errors, newline))
    };
    finish(result, std::ptr::null())
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
        Ok(Str::from(codec.decode_stream(
            &data,
            errors,
            newline.is_none(),
        )?))
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
        result.map(|text| StrOwner::into_raw(StrOwner::new(text))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root the path, text and each nonzero optional string through the write and close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_text_options(
    path: *const Object,
    text_handle: *const Str,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> usize {
    // SAFETY: the path, data and options remain immutably rooted for synchronous I/O.
    let (path, text, encoding, errors, newline) = unsafe {
        (
            object(path),
            &*text_handle,
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
                    converted = output_newlines(text, replacement)?;
                    &converted
                } else {
                    text.as_str()
                };
                file.write_bytes(&codec.encode(value, errors, || {
                    if value == text.as_str() {
                        // The string borrow covers the value. Its original allocation
                        // handle also covers the reference count that we increment.
                        Ok(StrOwner::acquire_word(text_handle as usize))
                    } else {
                        StrOwner::try_new(Str::try_new(value).map_err(|_| Error::allocation())?)
                            .map_err(|_| Error::allocation())
                    }
                })?)?;
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
