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

fn lowercase(source: &str) -> Result<Str> {
    if source.is_ascii() {
        let mut result = Str::try_new(source).map_err(|_| Error::allocation())?;
        result.as_mut_str().make_ascii_lowercase();
        return Ok(result);
    }
    let mut result = Str::try_with_capacity(source.len()).map_err(|_| Error::allocation())?;
    for (i, ch) in source.char_indices() {
        let mapping = if ch == 'Σ' && is_final_sigma(&source[..i], &source[i + ch.len_utf8()..]) {
            ['ς' as u32, 0]
        } else {
            unicode_case_mapping::to_lowercase(ch)
        };
        let first = if mapping[0] == 0 {
            ch
        } else {
            char::from_u32(mapping[0]).unwrap()
        };
        let second = (mapping[1] != 0).then(|| char::from_u32(mapping[1]).unwrap());
        result
            .try_reserve(first.len_utf8() + second.map_or(0, char::len_utf8))
            .map_err(|_| Error::allocation())?;
        result.push(first);
        if let Some(second) = second {
            result.push(second);
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
        lowercase(source).map(|s| Rc::into_raw(Rc::new(s))),
        std::ptr::null(),
    )
}
