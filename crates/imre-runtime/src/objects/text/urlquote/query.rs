//! URL-encoded forms keep the order and byte/string layout of each pair.
use super::*;
use crate::objects::{
    Object, StorageCell, bytes,
    codecs::encoded_bytes,
    exceptions::string_repr,
    records::{RecordDescriptor, allocate_record},
    words::{OwnedWord, WordKind, Words},
};
use crate::{Int, ObjectOwner};

fn component(input: &[u8], bytes: bool, encoding: &str, errors: &str) -> Result<OwnedWord> {
    let mut input = Cow::Borrowed(input);
    if input.contains(&b'+') {
        let mut spaced = Vec::new();
        spaced
            .try_reserve_exact(input.len())
            .map_err(|_| Error::allocation())?;
        spaced.extend(input.iter().map(|&b| if b == b'+' { b' ' } else { b }));
        input = Cow::Owned(spaced);
    }
    let (word, kind) = if bytes {
        (encoded_bytes(unquoted(&input)?)? as usize, 8)
    } else {
        let text = if input.contains(&b'%') {
            check_option(encoding)?;
            check_option(errors)?;
            crate::checked_owned_str(decode_text(&input, Codec::lookup(encoding)?, errors)?.into())?
        } else {
            // SAFETY: A text component has only been split on whole scalars and ASCII delimiters.
            let raw = unsafe { std::str::from_utf8_unchecked(&input) };
            StrOwner::into_raw(crate::objects::text::try_owned_string(raw)?)
        };
        (text as usize, 3)
    };
    // SAFETY: The allocation above has transferred one owner of the selected kind.
    Ok(unsafe { OwnedWord::from_owned(word, WordKind::new(kind)) })
}

/// # Safety
/// The caller roots all non-null inputs and supplies a permanent descriptor for
/// the two str or bytes fields. A zero max word denotes None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_parse_qsl(
    query: *const c_void,
    is_bytes: i32,
    keep: i32,
    strict: i32,
    encoding: *const Str,
    errors: *const Str,
    max: usize,
    separator: *const c_void,
    separator_bytes: i32,
    pair_layout: *const RecordDescriptor,
) -> *const Object {
    let bytes = is_bytes != 0;
    let result = (|| {
        let mut pairs = Words::new(6);
        let sep = if separator.is_null() {
            &[][..]
        } else {
            // SAFETY: Non-null separators carry their rooted str/bytes layout.
            unsafe { url_data(separator, separator_bytes != 0) }
        };
        if sep.is_empty() {
            return Err(Error::new(
                Kind::ValueError,
                "Separator must be of type string or bytes.",
            ));
        }
        if !query.is_null() {
            if bytes != (separator_bytes != 0) && !sep.is_ascii() {
                if bytes {
                    // SAFETY: A mixed separator for bytes input is a rooted str.
                    let text = unsafe { &*separator.cast::<Str>() };
                    Codec::Ascii.encode(text, Some("strict"), || {
                        // SAFETY: The encode error retains the same rooted str separator.
                        Ok(unsafe { StrOwner::acquire_word(separator as usize) })
                    })?;
                } else {
                    Codec::Ascii.decode(sep, Some("strict"))?;
                }
            }
            // SAFETY: The query and optional scalar strings are all rooted for the call.
            let (source, encoding, errors) = unsafe {
                (
                    url_data(query, bytes),
                    encoding.as_ref().map_or("utf-8", Str::as_str),
                    errors.as_ref().map_or("replace", Str::as_str),
                )
            };
            if !source.is_empty() {
                let finder = memchr::memmem::Finder::new(sep);
                if max != 0 {
                    let count = Int::from_usize(1 + finder.find_iter(source).count());
                    // SAFETY: Nonzero max borrows a live arbitrary precision integer.
                    if unsafe { Int::borrow(max) }.cmp(&count).is_lt() {
                        return Err(Error::new(
                            Kind::ValueError,
                            "Max number of fields exceeded",
                        ));
                    }
                }
                let mut start = 0;
                for end in finder
                    .find_iter(source)
                    .chain(std::iter::once(source.len()))
                {
                    let field = &source[start..end];
                    start = end.saturating_add(sep.len());
                    let equals = memchr::memchr(b'=', field);
                    if equals.is_none() && strict != 0 {
                        let repr = if bytes {
                            bytes::repr(field)?
                        } else {
                            // SAFETY: A str query was split only on a valid text separator.
                            string_repr(unsafe { std::str::from_utf8_unchecked(field) })
                        };
                        return Err(Error::new(
                            Kind::ValueError,
                            format!("bad query field: {}", repr.as_str()),
                        ));
                    }
                    let (name, value) =
                        equals.map_or((field, &[][..]), |i| (&field[..i], &field[i + 1..]));
                    if field.is_empty() || (value.is_empty() && keep == 0) {
                        continue;
                    }
                    pairs
                        .values
                        .try_reserve(1)
                        .map_err(|_| Error::allocation())?;
                    let key = component(name, bytes, encoding, errors)?;
                    let value = component(value, bytes, encoding, errors)?;
                    // SAFETY: The permanent tuple descriptor matches both initialized owners.
                    unsafe {
                        let pair = allocate_record(pair_layout);
                        if pair.is_null() {
                            return Err(Error::allocation());
                        }
                        key.initialize_record_field(pair);
                        value.initialize_record_field(pair.add(1));
                        pairs.values.push(pair as usize);
                    }
                }
            }
        }
        ObjectOwner::try_new(Object(StorageCell::new(pairs.into_storage())))
            .map(ObjectOwner::into_raw)
            .map_err(|_| Error::allocation())
    })();
    finish(result, std::ptr::null())
}
