//! Default Unicode 16 case conversion, as used by Python 3.14.
use crate::StrOwner;
use std::sync::LazyLock;

use compact_str::ReserveError;
use regex_syntax::hir::{Class, ClassUnicode, HirKind};

use super::{Error, Str, finish};

/// Read a binary property from the runtime's pinned Unicode 16 regex tables.
fn property(pattern: &str) -> ClassUnicode {
    let parsed = regex_syntax::Parser::new()
        .parse(pattern)
        .expect("known Unicode property");
    let HirKind::Class(Class::Unicode(class)) = parsed.into_kind() else {
        unreachable!("expected a Unicode character class");
    };
    class
}

fn contains(class: &ClassUnicode, ch: char) -> bool {
    let ranges = class.ranges();
    ranges
        .get(ranges.partition_point(|r| r.end() < ch))
        .is_some_and(|r| r.start() <= ch)
}

/// U+03A3 is final after a cased character unless another cased character follows.
/// Both searches ignore Case_Ignorable code points (even when also Cased).
fn is_final_sigma(before: &str, after: &str) -> bool {
    static CASED: LazyLock<ClassUnicode> = LazyLock::new(|| property(r"\p{Cased}"));
    static IGNORABLE: LazyLock<ClassUnicode> = LazyLock::new(|| property(r"\p{Case_Ignorable}"));
    let significant = |c: &char| !contains(&IGNORABLE, *c);
    let is_cased = |c| contains(&CASED, c);
    before.chars().rev().find(significant).is_some_and(is_cased)
        && !after.chars().find(significant).is_some_and(is_cased)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Lower,
    Upper,
    Capitalize,
}

fn case_chars(source: &str, case: Case) -> impl Iterator<Item = char> {
    source.char_indices().flat_map(move |(i, ch)| {
        let mapping = if case == Case::Upper {
            unicode_case_mapping::to_uppercase(ch)
        } else if case == Case::Capitalize && i == 0 {
            unicode_case_mapping::to_titlecase(ch)
        } else if ch == 'Σ' && is_final_sigma(&source[..i], &source[i + ch.len_utf8()..]) {
            ['ς' as u32, 0, 0]
        } else {
            let [a, b] = unicode_case_mapping::to_lowercase(ch);
            [a, b, 0]
        };
        let chars = if mapping[0] == 0 {
            [Some(ch), None, None]
        } else {
            mapping.map(|value| (value != 0).then(|| char::from_u32(value).unwrap()))
        };
        chars.into_iter().flatten()
    })
}

/// Yield the same Unicode mapping as str.lower() without storing the result.
pub(in crate::objects) fn lowercase_chars(source: &str) -> impl Iterator<Item = char> {
    case_chars(source, Case::Lower)
}

/// CPython titlecases just the first code point for capitalize, then lowercases
/// the rest in the context of the original string (not of the suffix alone).
fn convert(source: &str, case: Case) -> Result<Str, ReserveError> {
    if source.is_ascii() {
        let mut result = Str::try_new(source)?;
        let ascii = result.as_mut_str();
        if case == Case::Upper {
            ascii.make_ascii_uppercase();
        } else {
            ascii.make_ascii_lowercase();
            if case == Case::Capitalize
                && let Some(first) = ascii.get_mut(..1)
            {
                first.make_ascii_uppercase();
            }
        }
        return Ok(result);
    }
    let mut result = Str::try_with_capacity(source.len())?;
    for ch in case_chars(source, case) {
        result.try_reserve(ch.len_utf8())?;
        result.push(ch);
    }
    Ok(result)
}

/// Lowercase a full string with the runtime's pinned Unicode data.
pub(in crate::objects) fn lowercase(source: &str) -> Result<Str, ReserveError> {
    convert(source, Case::Lower)
}

/// Lowercase an exact native string, setting a pending MemoryError on allocation failure.
///
/// # Safety
/// The caller roots the immutable input string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_lower(value: *const Str) -> *const Str {
    // SAFETY: the caller roots the string.
    let source = unsafe { &*value };
    finish(
        lowercase(source)
            .map(|s| StrOwner::into_raw(StrOwner::new(s)))
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
}

/// Uppercase a native string with the runtime's pinned Unicode data.
///
/// # Safety
/// The caller roots the immutable input string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_upper(value: *const Str) -> *const Str {
    // SAFETY: the caller roots the string throughout conversion.
    let source = unsafe { &*value };
    finish(
        convert(source, Case::Upper)
            .map(|s| StrOwner::into_raw(StrOwner::new(s)))
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
}

/// Titlecase the first character and lowercase the rest of a native string.
///
/// # Safety
/// The caller roots the immutable input string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_capitalize(value: *const Str) -> *const Str {
    // SAFETY: the caller roots the string.
    let source = unsafe { &*value };
    finish(
        convert(source, Case::Capitalize)
            .map(|s| StrOwner::into_raw(StrOwner::new(s)))
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
}
