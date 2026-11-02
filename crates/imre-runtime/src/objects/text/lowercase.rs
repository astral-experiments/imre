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

static CASED: LazyLock<ClassUnicode> = LazyLock::new(|| property(r"\p{Cased}"));
static LOWERCASE: LazyLock<ClassUnicode> = LazyLock::new(|| property(r"\p{Lowercase}"));
static UPPERCASE: LazyLock<ClassUnicode> = LazyLock::new(|| property(r"\p{Uppercase}"));

/// True if at least one character is cased and every cased character is lowercase.
pub(crate) fn python_islower(chars: impl Iterator<Item = char>) -> bool {
    let mut seen_lower = false;
    for ch in chars {
        if ch.is_ascii() {
            if ch.is_ascii_uppercase() {
                return false;
            }
            seen_lower |= ch.is_ascii_lowercase();
        } else if contains(&LOWERCASE, ch) {
            seen_lower = true;
        } else if contains(&CASED, ch) {
            return false;
        }
    }
    seen_lower
}

/// Each cased run starts with an uppercase or titlecase character, followed by lowercase.
pub(crate) fn python_istitle(chars: impl Iterator<Item = char>) -> bool {
    let mut seen_cased = false;
    let mut previous_cased = false;
    for ch in chars {
        let cased = contains(&CASED, ch);
        if cased {
            // Cased is the union of Lowercase, Uppercase, and Titlecase_Letter.
            if contains(&LOWERCASE, ch) != previous_cased {
                return false;
            }
            seen_cased = true;
        }
        previous_cased = cased;
    }
    seen_cased
}

/// U+03A3 is final after a cased character unless another cased character follows.
/// Both searches ignore Case_Ignorable code points (even when also Cased).
fn is_final_sigma(before: impl Iterator<Item = u32>, after: impl Iterator<Item = u32>) -> bool {
    static IGNORABLE: LazyLock<ClassUnicode> = LazyLock::new(|| property(r"\p{Case_Ignorable}"));
    fn next_is_cased(mut points: impl Iterator<Item = u32>) -> bool {
        points
            .find(|&p| char::from_u32(p).is_none_or(|c| !contains(&IGNORABLE, c)))
            .and_then(char::from_u32)
            .is_some_and(|c| contains(&CASED, c))
    }
    next_is_cased(before) && !next_is_cased(after)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Case {
    Lower,
    Upper,
    Capitalize,
    Title,
    Swap,
}

/// Apply a full mapping to an original scalar. Only title consults its immediate
/// predecessor; lowercasing sigma looks beyond case-ignorable characters.
fn mapped_char(
    ch: char,
    case: Case,
    first: bool,
    previous_cased: &mut bool,
    final_sigma: impl FnOnce() -> bool,
) -> [Option<char>; 3] {
    let title = if case == Case::Title {
        let previous = *previous_cased;
        *previous_cased = contains(&CASED, ch);
        !previous
    } else {
        case == Case::Capitalize && first
    };
    let mapping = if title {
        unicode_case_mapping::to_titlecase(ch)
    } else if case == Case::Upper || (case == Case::Swap && contains(&LOWERCASE, ch)) {
        unicode_case_mapping::to_uppercase(ch)
    } else if case == Case::Swap && !contains(&UPPERCASE, ch) {
        // Titlecase digraphs are cased but neither lowercase nor uppercase.
        [0; 3]
    } else if ch == 'Σ' && final_sigma() {
        ['ς' as u32, 0, 0]
    } else {
        let [a, b] = unicode_case_mapping::to_lowercase(ch);
        [a, b, 0]
    };
    if mapping[0] == 0 {
        [Some(ch), None, None]
    } else {
        mapping.map(|value| (value != 0).then(|| char::from_u32(value).unwrap()))
    }
}

fn case_chars(source: &str, case: Case) -> impl Iterator<Item = char> {
    let mut previous_cased = false;
    source.char_indices().flat_map(move |(i, ch)| {
        mapped_char(ch, case, i == 0, &mut previous_cased, || {
            is_final_sigma(
                source[..i].chars().rev().map(u32::from),
                source[i + ch.len_utf8()..].chars().map(u32::from),
            )
        })
        .into_iter()
        .flatten()
    })
}

/// Map a scalar string with the same Unicode data used by native string operations.
pub(crate) fn case_map(source: &str, case: Case) -> super::Result<String> {
    let mut result = String::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| Error::allocation())?;
    if case == Case::Lower && source.is_ascii() {
        result.push_str(source);
        result.make_ascii_lowercase();
        return Ok(result);
    }
    for ch in case_chars(source, case) {
        result
            .try_reserve(ch.len_utf8())
            .map_err(|_| Error::allocation())?;
        result.push(ch);
    }
    Ok(result)
}

/// Preserve surrogates as uncased boundaries when mapping Python code points.
pub(crate) fn case_map_points(points: &[u32], case: Case) -> super::Result<Vec<u32>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(points.len())
        .map_err(|_| Error::allocation())?;
    let mut previous_cased = false;
    for (i, &point) in points.iter().enumerate() {
        if let Some(ch) = char::from_u32(point) {
            let chars = mapped_char(ch, case, i == 0, &mut previous_cased, || {
                is_final_sigma(
                    points[..i].iter().rev().copied(),
                    points[i + 1..].iter().copied(),
                )
            });
            for mapped in chars.into_iter().flatten() {
                result.try_reserve(1).map_err(|_| Error::allocation())?;
                result.push(u32::from(mapped));
            }
        } else {
            previous_cased = false;
            result.try_reserve(1).map_err(|_| Error::allocation())?;
            result.push(point);
        }
    }
    Ok(result)
}

/// Return an exact str. CPython reuses the empty string and makes a new owner for any nonempty result.
///
/// # Safety
/// The caller roots the immutable string storage for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_case_map(source: *const Str, title: u32) -> *const Str {
    // SAFETY: the caller keeps the immutable Str alive for this borrow.
    let source = unsafe { &*source };
    let result = if source.is_empty() {
        super::try_owned_string("")
    } else {
        let case = if title != 0 { Case::Title } else { Case::Swap };
        case_map(source, case)
            .and_then(|value| StrOwner::try_new(Str::from(value)).map_err(|_| Error::allocation()))
    };
    finish(result.map(StrOwner::into_raw), std::ptr::null())
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

/// Titlecase the first character and lowercase the rest in their original context.
pub(crate) fn capitalize(source: &str) -> super::Result<Str> {
    convert(source, Case::Capitalize).map_err(|_| Error::allocation())
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
        capitalize(source).map(|s| StrOwner::into_raw(StrOwner::new(s))),
        std::ptr::null(),
    )
}
