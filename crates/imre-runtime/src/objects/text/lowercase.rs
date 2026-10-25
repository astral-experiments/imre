//! Default Unicode 16 case conversion, as used by Python 3.14.
use std::rc::Rc;
use std::sync::LazyLock;

use regex_syntax::hir::{Class, ClassUnicode, HirKind};

use super::{Error, Result, Str, finish};

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

/// CPython titlecases just the first code point for capitalize, then lowercases
/// the rest in the context of the original string (not of the suffix alone).
fn convert(source: &str, capitalize: bool) -> Result<Str> {
    if source.is_ascii() {
        let mut result = Str::try_new(source).map_err(|_| Error::allocation())?;
        let ascii = result.as_mut_str();
        ascii.make_ascii_lowercase();
        if capitalize && let Some(first) = ascii.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        return Ok(result);
    }
    let mut result = Str::try_with_capacity(source.len()).map_err(|_| Error::allocation())?;
    for (i, ch) in source.char_indices() {
        let mapping = if capitalize && i == 0 {
            unicode_case_mapping::to_titlecase(ch)
        } else if ch == 'Σ' && is_final_sigma(&source[..i], &source[i + ch.len_utf8()..]) {
            ['ς' as u32, 0, 0]
        } else {
            let [a, b] = unicode_case_mapping::to_lowercase(ch);
            [a, b, 0]
        };
        if mapping[0] == 0 {
            result
                .try_reserve(ch.len_utf8())
                .map_err(|_| Error::allocation())?;
            result.push(ch);
        } else {
            for value in mapping.into_iter().take_while(|v| *v != 0) {
                let mapped = char::from_u32(value).unwrap();
                result
                    .try_reserve(mapped.len_utf8())
                    .map_err(|_| Error::allocation())?;
                result.push(mapped);
            }
        }
    }
    Ok(result)
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
        convert(source, false).map(|s| Rc::into_raw(Rc::new(s))),
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
        convert(source, true).map(|s| Rc::into_raw(Rc::new(s))),
        std::ptr::null(),
    )
}
