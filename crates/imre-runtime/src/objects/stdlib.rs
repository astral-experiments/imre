//! Scalar sorting, callback-based sorting, and UTF-8 builders.

mod comparator;

use crate::StrOwner;
use std::cmp::Ordering;

use smallvec::SmallVec;

use super::{Bytes, Error, Kind, List, Object, Storage, StorageCell, Str, Words, fail, object};

/// Sorts original indices instead of moving key owners. Reversing comparison,
/// rather than the finished sequence, preserves the order of equal keys.
fn ordered_indices(
    length: usize,
    reverse: bool,
    compare: impl Fn(usize, usize) -> Ordering,
) -> Option<SmallVec<[usize; 4]>> {
    let mut indices = SmallVec::new();
    indices.try_reserve_exact(length).ok()?;
    indices.extend(0..length);
    indices.sort_unstable_by(|&left, &right| {
        let order = compare(left, right);
        if reverse { order.reverse() } else { order }
    });
    // Restore encounter order within equal-key runs, including reverse sorts.
    for equal in indices.chunk_by_mut(|&left, &right| compare(left, right).is_eq()) {
        equal.sort_unstable();
    }
    Some(indices)
}

/// Tuple comparison may run out of traversal space. Merge indices in scratch
/// storage so an error can leave the caller's list and all its owners untouched.
unsafe fn tuple_key_order(words: &[usize], reverse: bool) -> Option<SmallVec<[usize; 4]>> {
    let mut order = SmallVec::<[usize; 4]>::new();
    order.try_reserve_exact(words.len()).ok()?;
    order.extend(0..words.len());
    let mut scratch = SmallVec::<[usize; 4]>::new();
    scratch.try_reserve_exact(words.len()).ok()?;
    scratch.resize(words.len(), 0);
    let mut width = 1;
    while width < words.len() {
        for (input, output) in order.chunks(2 * width).zip(scratch.chunks_mut(2 * width)) {
            let (mut left, mut right) = input.split_at(width.min(input.len()));
            let mut slots = output.iter_mut();
            while let (Some(&a), Some(&b)) = (left.first(), right.first()) {
                // SAFETY: the key list roots all tuples of this exact ordered shape.
                let cmp = unsafe {
                    super::values::tuple_order(words[a] as *const usize, words[b] as *const usize)
                }?;
                let first = if (if reverse { cmp.reverse() } else { cmp }).is_le() {
                    &mut left
                } else {
                    &mut right
                };
                *slots.next().unwrap() = first[0];
                *first = &first[1..];
            }
            slots
                .into_slice()
                .copy_from_slice(if left.is_empty() { right } else { left });
        }
        std::mem::swap(&mut order, &mut scratch);
        width *= 2;
    }
    Some(order)
}

/// Walks each cycle with owner-preserving swaps, marking visited indices in the permutation.
fn permute<T>(values: &mut [T], order: &mut [usize]) {
    permute_with(values.len(), order, |a, b| values.swap(a, b));
}

fn permute_with(len: usize, order: &mut [usize], mut swap: impl FnMut(usize, usize)) {
    if len != order.len() {
        fail("SystemError", "sort key count differs from value count");
    }
    for start in 0..len {
        let mut current = start;
        while order[current] != current {
            let next = std::mem::replace(&mut order[current], current);
            if next == start {
                break;
            }
            swap(current, next);
            current = next;
        }
    }
}

/// Long path-like strings often share the same leading bytes. For a large
/// stable sort, find that prefix once before comparing the remaining bytes.
///
pub(super) enum StringSort {
    Stable { reverse: bool },
    Distinct,
}

pub(super) fn stable_sort<T: Copy>(values: &mut [T], compare: impl FnMut(&T, &T) -> Ordering) {
    // Glidesort needs at least 48 scratch elements to avoid internal allocation.
    // A stack buffer also lets a large sort finish when the heap is full.
    let mut local = [std::mem::MaybeUninit::uninit(); 64];
    let mut heap = Vec::<T>::new();
    let scratch = if values.len() <= 128 || heap.try_reserve_exact(values.len()).is_err() {
        local.as_mut_slice()
    } else {
        heap.spare_capacity_mut()
    };
    glidesort::sort_with_buffer_by(values, scratch, compare);
}

/// # Safety
/// Each word is an immutable string rooted for the entire sort.
pub(super) unsafe fn sort_strings(values: &mut [usize], mode: StringSort) {
    // SAFETY: the list keeps these owners live, including while they move.
    let bytes = |word: usize| unsafe { (&*(word as *const Str)).as_bytes() };
    let skip = shared_prefix(values.iter().map(|&word| bytes(word)));
    let compare = |&left: &usize, &right: &usize| bytes(left)[skip..].cmp(&bytes(right)[skip..]);
    match mode {
        StringSort::Stable { reverse } => {
            stable_sort(values, |left, right| {
                if reverse {
                    compare(right, left)
                } else {
                    compare(left, right)
                }
            });
        }
        StringSort::Distinct => sort_distinct_words(values, compare),
    }
}

/// Merge a handful of ascending runs with a fallible scratch buffer. Random
/// input and a failed reservation both use the allocation-free general sort.
fn sort_distinct_words(
    values: &mut [usize],
    compare: impl Fn(&usize, &usize) -> std::cmp::Ordering,
) {
    let mut breaks = smallvec::SmallVec::<[usize; 8]>::new();
    for index in 1..values.len() {
        if compare(&values[index - 1], &values[index]).is_gt() {
            if breaks.len() == breaks.capacity() {
                values.sort_unstable_by(compare);
                return;
            }
            breaks.push(index);
        }
    }
    let mut scratch = Vec::new();
    let ends = breaks
        .iter()
        .copied()
        .skip(1)
        .chain(std::iter::once(values.len()));
    for (&split, end) in breaks.iter().zip(ends) {
        if end - split <= 8 {
            for index in split..end {
                let word = values[index];
                let dest = values[..index].partition_point(|other| compare(other, &word).is_lt());
                values[dest..=index].rotate_right(1);
            }
            continue;
        }
        scratch.clear();
        if scratch.try_reserve_exact(split).is_err() {
            values.sort_unstable_by(compare);
            return;
        }
        scratch.extend_from_slice(&values[..split]);
        let (mut left, mut right) = (0, split);
        for dest in 0..end {
            if left == split {
                // The unconsumed right suffix already starts at its final position.
                break;
            }
            if right < end && compare(&values[right], &scratch[left]).is_lt() {
                values[dest] = values[right];
                right += 1;
            } else {
                values[dest] = scratch[left];
                left += 1;
            }
        }
    }
}

/// Returns a byte prefix shared by every key only when its removal pays for a
/// large sort. UTF-8 byte order after an identical prefix is unchanged.
pub(super) fn shared_prefix<'a>(mut keys: impl ExactSizeIterator<Item = &'a [u8]>) -> usize {
    if keys.len() < 64 {
        return 0;
    }
    let mut prefix = keys.next().unwrap();
    for candidate in keys {
        if !candidate.starts_with(prefix) {
            let end = prefix
                .iter()
                .zip(candidate)
                .take_while(|(a, b)| a == b)
                .count();
            if end < 16 {
                return 0;
            }
            prefix = &prefix[..end];
        }
    }
    if prefix.len() < 16 { 0 } else { prefix.len() }
}

/// # Safety
/// Both words are rooted canonical integers for this comparison.
pub(super) unsafe fn compare_int(left: usize, right: usize) -> Ordering {
    if left & right & 1 != 0 {
        (left as isize).cmp(&(right as isize))
    } else {
        // SAFETY: the caller keeps both arbitrary-precision integers alive.
        unsafe { crate::imre_int_compare(left, right) }.cmp(&0)
    }
}

/// # Safety
/// `keys` is a rooted list with the exact nonnullable scalar element kind.
/// No mutation or callback overlaps the short immutable storage access.
unsafe fn key_order(keys: *const (), kind: u32, reverse: bool) -> Option<SmallVec<[usize; 4]>> {
    if kind == 2 {
        // SAFETY: integer lists have their separate exact runtime allocation.
        return unsafe {
            (&*keys.cast::<List>()).0.with(|values| {
                ordered_indices(values.len(), reverse, |left, right| {
                    compare_int(values[left].0, values[right].0)
                })
            })
        };
    }
    // SAFETY: only the owned permutation escapes, never a reference to key storage.
    unsafe {
        object(keys.cast()).0.with(|storage| {
            let values = storage.sequence().ok()?;
            values.check_kind(kind);
            if kind == 6 {
                return tuple_key_order(&values.values, reverse);
            }
            ordered_indices(values.values.len(), reverse, |left, right| {
                let (left, right) = (values.values[left], values.values[right]);
                match kind {
                    1 => left.cmp(&right),
                    10 => super::string_enums::compare(left, right),
                    3 => (&*(left as *const Str)).cmp(&*(right as *const Str)),
                    8 => (&*(left as *const Bytes)).cmp(&*(right as *const Bytes)),
                    _ => fail("SystemError", "unsupported sort key representation"),
                }
            })
        })
    }
}

/// Transfers a list's elements to a new owner, leaving its existing aliases
/// observing an empty list while Python key callbacks execute.
///
/// # Safety
/// The caller roots the exact list and supplies its element representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sort_detach(list: *const (), kind: u32) -> *const () {
    if kind == 2 {
        let Ok(saved) = crate::try_new_list(crate::sorting_empty_ints()) else {
            return super::Error::allocation().returning(std::ptr::null());
        };
        // SAFETY: the new owner is private and no callback runs during the swap.
        unsafe {
            (&*list.cast::<List>()).0.with_mut(|values| {
                saved
                    .0
                    .with_mut(|temporary| std::mem::swap(values, temporary))
            });
        }
        crate::ListOwner::into_raw(saved).cast()
    } else {
        // SAFETY: the descriptor and cached length travel with their owners.
        let saved = unsafe {
            object(list.cast()).0.with_mut(|storage| {
                let words = if storage.sequence_storage().rows().is_some() {
                    debug_assert_eq!(kind, 6);
                    storage.sequence_storage_mut()
                } else {
                    let words = storage.sequence_mut()?;
                    words.check_kind(kind);
                    words
                };
                let owner = crate::ObjectOwner::try_new(Object(StorageCell::new(
                    Storage::Sequence(Words::sorting_empty(kind)),
                )))
                .map_err(|_| super::Error::allocation())?;
                owner
                    .0
                    .with_mut(|temporary| std::mem::swap(words, temporary.sequence_storage_mut()));
                Ok(crate::ObjectOwner::into_raw(owner).cast())
            })
        };
        crate::finish(saved, std::ptr::null())
    }
}

/// Extending from an iterator without a length hint modifies the temporary
/// list even when it yields nothing. An ordinary destination needs no mutation.
///
/// # Safety
/// The argument is a rooted list with the supplied element representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_extend_unhinted(list: *const (), kind: u32) {
    if kind == 2 {
        // SAFETY: only an unallocated empty marker is replaced.
        unsafe {
            (&*list.cast::<List>()).0.with_mut(|values| {
                if crate::is_sorting_empty_ints(values) {
                    *values = Vec::new();
                }
            })
        };
    } else {
        // SAFETY: the real length stays unchanged at zero.
        unsafe {
            object(list.cast()).0.with_mut(|storage| {
                let words = storage.sequence_storage_mut();
                if words.is_sorting_empty() {
                    words.update_header();
                }
            })
        };
    }
}

/// Restores the saved elements and discards any additions made by key callbacks.
/// A mutation raises ValueError after a completed sort; a failed key keeps its
/// own exception and the original element order.
///
/// # Safety
/// Both same-kind lists are independently rooted and distinct. The private
/// saved list came from the matching detach and cannot be observed elsewhere.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sort_restore(
    list: *const (),
    kind: u32,
    saved: *const (),
    completed: u32,
) {
    let modified = if kind == 2 {
        // SAFETY: the caller proves distinct handles and no reentrant access.
        unsafe {
            (&*list.cast::<List>()).0.with_mut(|current| {
                let modified = !crate::is_sorting_empty_ints(current);
                (&*saved.cast::<List>())
                    .0
                    .with_mut(|saved| std::mem::swap(current, saved));
                modified
            })
        }
    } else {
        // SAFETY: the swap transfers all owners and headers without releasing
        // any value during access. Normal scope cleanup later drops the additions.
        unsafe {
            object(list.cast()).0.with_mut(|storage| {
                let current = storage.sequence_storage_mut();
                let modified = !current.is_sorting_empty();
                object(saved.cast())
                    .0
                    .with_mut(|storage| std::mem::swap(current, storage.sequence_storage_mut()));
                modified
            })
        }
    };
    if modified && completed != 0 {
        Error::new(Kind::ValueError, "list modified during sort").raise();
    }
}

/// Reorders materialized values by already evaluated scalar keys.
/// Key callbacks and their errors are handled by ordinary generated code before
/// this operation. No comparison calls user code or changes reference counts.
///
/// # Safety
/// Both pointers are rooted exact lists of equal length with the supplied kinds;
/// Kind 2 denotes List, all others a Sequence Object. Keys are ordered scalars or
/// immutable tuples whose fields have a compiler-proved total order.
/// Values own each element independently and cannot be accessed reentrantly.
/// The same list may supply both pointers: key reading ends before any mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sort_by_keys(
    values: *const (),
    element_kind: u32,
    keys: *const (),
    key_kind: u32,
    reverse: u32,
) {
    if values == keys && key_kind == 3 {
        // Without a key function, the string list itself owns every comparison
        // operand. A stable sort can move those owners directly.
        // SAFETY: the exclusive list access roots both strings across comparison.
        unsafe {
            object(values.cast()).0.with_mut(|storage| {
                let values = match storage.sequence_mut() {
                    Ok(words) => words,
                    Err(error) => return error.raise(),
                };
                values.check_kind(key_kind);
                sort_strings(
                    &mut values.values,
                    StringSort::Stable {
                        reverse: reverse != 0,
                    },
                );
            });
        }
        return;
    }
    // SAFETY: both exact list owners remain rooted throughout this operation.
    let Some(mut order) = (unsafe { key_order(keys, key_kind, reverse != 0) }) else {
        super::exceptions::sequence_allocation_failed();
        return;
    };
    // SAFETY: all immutable key storage borrows have ended.
    unsafe { apply_order(values, element_kind, &mut order) };
}

/// Move element owners through a checked permutation without running user code.
///
/// # Safety
/// The list is rooted with this exact kind; order covers each element once.
unsafe fn apply_order(values: *const (), element_kind: u32, order: &mut [usize]) {
    if element_kind == 2 {
        // SAFETY: all immutable key access has ended; swaps transfer existing owners.
        unsafe {
            (&*values.cast::<List>())
                .0
                .with_mut(|values| permute(values, order))
        };
    } else {
        // SAFETY: no storage reference escapes and swaps never drop an owner.
        unsafe {
            object(values.cast()).0.with_mut(|storage| {
                if let Some(rows) = storage.sequence_storage_mut().rows_mut() {
                    match rows.reorder_shared(order) {
                        Ok(false) => permute_with(rows.len(), order, |a, b| rows.swap(a, b)),
                        Ok(true) => {}
                        Err(error) => error.raise(),
                    }
                } else {
                    let values = match storage.sequence_mut() {
                        Ok(words) => words,
                        Err(error) => return error.raise(),
                    };
                    values.check_kind(element_kind);
                    permute(&mut values.values, order);
                }
            })
        };
    }
}

fn string_buffer(length: Option<usize>) -> Option<Str> {
    let buffer = length.and_then(|length| Str::try_with_capacity(length).ok());
    if buffer.is_none() {
        super::exceptions::sequence_allocation_failed();
    }
    buffer
}

/// Joins an exact string sequence, retaining a single item without copying it.
///
/// # Safety
/// The separator and exact list[str] are rooted. No callback or mutation occurs
/// during the call; only an independent string owner escapes the storage borrow.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_join(separator: *const Str, values: *const Object) -> *const Str {
    // SAFETY: immutable string contents remain rooted by the caller and list.
    unsafe {
        let separator = &*separator;
        object(values).0.with(|storage| {
            if let Some(joined) = storage
                .sequence_storage()
                .plan()
                .and_then(|plan| plan.join_ranges(separator))
            {
                return joined;
            }
            let values = match storage.sequence() {
                Ok(values) => values,
                Err(error) => return error.returning(std::ptr::null()),
            };
            values.check_kind(3);
            if let [word] = values.values.as_slice() {
                let value = *word as *const Str;
                StrOwner::increment_strong_count(value);
                return value;
            }
            let length = separator
                .len()
                .checked_mul(values.values.len().saturating_sub(1))
                .and_then(|separators| {
                    values.values.iter().try_fold(separators, |length, &word| {
                        length.checked_add((&*(word as *const Str)).len())
                    })
                });
            let Some(mut result) = string_buffer(length) else {
                return std::ptr::null();
            };
            for (index, &word) in values.values.iter().enumerate() {
                if index != 0 {
                    result.push_str(separator);
                }
                result.push_str(&*(word as *const Str));
            }
            StrOwner::into_raw(StrOwner::new(result))
        })
    }
}

/// Parse a decimal field or item index, including Python's Unicode decimal digits.
pub(super) fn format_index(name: &str) -> crate::Result<Option<usize>> {
    let mut index = 0isize;
    // A decimal prefix can overflow before a nondigit makes this a named field.
    for ch in name.chars() {
        let Some(digit) = crate::integer_text::decimal_digit(ch) else {
            return Ok(None);
        };
        index = index
            .checked_mul(10)
            .and_then(|value| value.checked_add(isize::from(digit)))
            .ok_or_else(|| {
                Error::new(Kind::ValueError, "Too many decimal digits in format string")
            })?;
    }
    Ok(Some(index as usize))
}

/// Both keyword-only formatters have an empty tuple of positional arguments.
pub(super) fn check_keyword_field(name: &str) -> crate::Result<()> {
    if let Some(index) = format_index(name)? {
        return Err(Error::new(
            Kind::IndexError,
            compact_str::format_compact!(
                "Replacement index {index} out of range for positional args tuple"
            ),
        ));
    }
    Ok(())
}

enum FormatValue<'a> {
    Text(&'a str),
    Integer(std::mem::ManuallyDrop<crate::Int>),
}

/// Substitutes plain named fields in encounter order. A missing field can raise
/// before a malformed brace later in the same template.
fn format_named<'a>(
    template: &str,
    field: impl Fn(&str) -> Option<FormatValue<'a>>,
) -> crate::Result<Str> {
    // A long template can consist entirely of fields with empty replacements.
    let mut result = Str::default();
    let mut rest = template;
    while let Some(at) = memchr::memchr2(b'{', b'}', rest.as_bytes()) {
        result.push_str(&rest[..at]);
        let brace = rest.as_bytes()[at];
        rest = &rest[at + 1..];
        if rest.as_bytes().first() == Some(&brace) {
            result.push(char::from(brace));
            rest = &rest[1..];
            continue;
        }
        if brace == b'}' || rest.is_empty() {
            return Err(Error::new(
                Kind::ValueError,
                if brace == b'}' {
                    "Single '}' encountered in format string"
                } else {
                    "Single '{' encountered in format string"
                },
            ));
        }
        let Some(end) = rest.find(['{', '}', '!', ':']) else {
            return Err(Error::new(
                Kind::ValueError,
                "expected '}' before end of string",
            ));
        };
        let (field_name, mut tail) = rest.split_at(end);
        if tail.starts_with('{') {
            return Err(Error::new(Kind::ValueError, "unexpected '{' in field name"));
        }
        let name = field_name.split(['.', '[']).next().unwrap();
        let mut supported = name.len() == field_name.len();
        let unmatched = || Error::new(Kind::ValueError, "unmatched '{' in format spec");
        if let Some(conversion) = tail.strip_prefix('!') {
            let mut chars = conversion.chars();
            let flag = chars.next().ok_or_else(|| {
                Error::new(
                    Kind::ValueError,
                    "end of string while looking for conversion specifier",
                )
            })?;
            supported &= flag == 's';
            tail = chars.as_str();
            if !tail.is_empty() && !tail.starts_with([':', '}']) {
                return Err(Error::new(
                    Kind::ValueError,
                    "expected ':' after conversion specifier",
                ));
            }
        }
        if let Some(spec) = tail.strip_prefix(':') {
            let end = spec.find('}').ok_or_else(unmatched)?;
            supported &= end == 0;
            tail = &spec[end..];
        }
        rest = tail.strip_prefix('}').ok_or_else(unmatched)?;
        check_keyword_field(name)?;
        let text = field(name).ok_or_else(|| Error::string_key(Str::new(name)))?;
        if !supported {
            return Err(Error::new(
                Kind::ValueError,
                "Imre str.format supports plain named fields and !s",
            ));
        }
        match text {
            FormatValue::Text(text) => result.push_str(text),
            FormatValue::Integer(value) => {
                result.push_str(&crate::integer_text::decimal_string(&value)?)
            }
        }
    }
    result.push_str(rest);
    Ok(result)
}

/// # Safety
/// The template and each of the `count` pairs of immutable name and value
/// handles are rooted until return. The result has its own string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_format(
    template: *const Str,
    fields: *const [*const Str; 2],
    count: usize,
) -> *const Str {
    // SAFETY: the compiler supplies one stack slot per rooted argument pair.
    let (template, fields) = unsafe { (&*template, std::slice::from_raw_parts(fields, count)) };
    let formatted = format_named(template, |name| {
        fields.iter().find_map(|&[key, value]| {
            // SAFETY: the operands stay immutable and rooted until formatting ends.
            unsafe { ((&*key).as_str() == name).then(|| FormatValue::Text((&*value).as_str())) }
        })
    });
    super::finish(
        formatted.map(|text| StrOwner::into_raw(StrOwner::new(text))),
        std::ptr::null(),
    )
}

/// Format named fields without converting unused or not-yet-selected integers.
///
/// # Safety
/// The caller roots the template and every (name, word, representation) triple
/// while the formatter borrows each matching field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_format_fields(
    template: *const Str,
    fields: *const [usize; 3],
    count: usize,
) -> *const Str {
    // SAFETY: the compiler creates one triple per evaluated keyword argument.
    let (template, fields) = unsafe { (&*template, std::slice::from_raw_parts(fields, count)) };
    let result = format_named(template, |name| {
        fields.iter().find_map(|&[key, value, kind]| {
            // SAFETY: these immutable words use the signature-checked string and integer tags.
            unsafe {
                ((&*(key as *const Str)).as_str() == name).then(|| match kind {
                    2 => FormatValue::Integer(crate::Int::borrow(value)),
                    3 => FormatValue::Text((&*(value as *const Str)).as_str()),
                    _ => unreachable!("invalid string-format field type"),
                })
            }
        })
    });
    super::finish(result.and_then(crate::checked_owned_str), std::ptr::null())
}

/// # Safety
/// All three handles are rooted immutable UTF-8 strings. Empty matches occur at
/// Unicode scalar boundaries, including both ends, exactly as Python str.replace.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_replace(
    value: *const Str,
    old: *const Str,
    new: *const Str,
) -> *const Str {
    // SAFETY: inputs remain immutable and rooted through both sizing and copying.
    let (value, old, new) = unsafe { (&*value, &*old, &*new) };
    let count = if old == new {
        0
    } else {
        value.matches(old.as_str()).count()
    };
    if count == 0 {
        // SAFETY: the caller roots the input; the unchanged result takes its own owner.
        return StrOwner::into_raw(unsafe {
            crate::StrOwner::acquire_word(std::ptr::from_ref(value) as usize)
        });
    }
    let length = value
        .len()
        .checked_sub(old.len().checked_mul(count).unwrap())
        .and_then(|length| {
            new.len()
                .checked_mul(count)
                .and_then(|extra| length.checked_add(extra))
        });
    let Some(mut result) = string_buffer(length) else {
        return std::ptr::null();
    };
    let mut copied = 0;
    for (index, _) in value.match_indices(old.as_str()) {
        result.push_str(&value[copied..index]);
        result.push_str(new);
        copied = index + old.len();
    }
    result.push_str(&value[copied..]);
    StrOwner::into_raw(StrOwner::new(result))
}

/// Formats a binary64 value with correctly rounded fixed decimal precision.
/// Percent scaling is binary64 multiplication, before rounding. Rust's fixed
/// formatter uses ties-to-even; Python's lowercase nonfinite spellings are explicit.
#[unsafe(no_mangle)]
pub extern "C" fn imre_float_format(value: f64, precision: u32, flags: u32) -> *const Str {
    if precision > 1000 {
        fail("SystemError", "unchecked float format precision");
    }
    // The low bit selects percentages. A zero high byte keeps the default sign.
    let percent = flags & 1 != 0;
    let positive_sign = (flags >> 8) as u8;
    let value = if percent { value * 100.0 } else { value };
    let mut result = if value.is_nan() {
        Str::from("nan")
    } else if value.is_infinite() {
        Str::from(if value.is_sign_negative() {
            "-inf"
        } else {
            "inf"
        })
    } else {
        let precision = precision as usize;
        compact_str::format_compact!("{value:.precision$}")
    };
    if positive_sign != 0 && !result.starts_with('-') {
        result.insert(0, char::from(positive_sign));
    }
    if percent {
        result.push('%');
    }
    StrOwner::into_raw(StrOwner::new(result))
}

/// Suspend the current native thread, rounding a positive timeout up to nanoseconds.
#[unsafe(no_mangle)]
pub extern "C" fn imre_time_sleep(seconds: f64) {
    let error = if seconds.is_nan() {
        Some(Error::new(
            Kind::ValueError,
            "Invalid value NaN (not a number)",
        ))
    } else if !seconds.is_finite() || seconds.abs() >= i64::MAX as f64 / 1e9 {
        Some(Error::new(
            Kind::OverflowError,
            "timestamp out of range for platform time_t",
        ))
    } else if seconds < 0.0 {
        Some(Error::new(
            Kind::ValueError,
            "sleep length must be non-negative",
        ))
    } else {
        None
    };
    if let Some(e) = error {
        e.raise();
        return;
    }
    let duration = std::time::Duration::from_secs(seconds.trunc() as u64)
        + std::time::Duration::from_nanos((seconds.fract() * 1e9).ceil() as u64);
    std::thread::sleep(duration);
}

/// Seconds since the Unix epoch, including the subsecond fraction.
#[unsafe(no_mangle)]
pub extern "C" fn imre_wall_time() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64(),
        Err(before) => -before.duration().as_secs_f64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutation_growth_is_fallible_and_many_equal_keys_keep_their_positions() {
        assert!(ordered_indices(usize::MAX, false, |a, b| a.cmp(&b)).is_none());
        let keys: Vec<_> = (0..257).map(|index| index * 37 % 13).collect();
        for reverse in [false, true] {
            let mut expected: Vec<_> = (0..keys.len()).collect();
            expected.sort_by(|&left, &right| {
                let order = keys[left].cmp(&keys[right]);
                if reverse { order.reverse() } else { order }
            });
            let actual =
                ordered_indices(keys.len(), reverse, |a, b| keys[a].cmp(&keys[b])).unwrap();
            assert_eq!(actual.as_slice(), expected);
        }
    }

    #[test]
    fn distinct_word_sort_handles_run_boundaries() {
        for runs in 1..=10 {
            for length in [0, 1, 7, 8, 31, 129] {
                let mut words: Vec<_> = (0..runs)
                    .flat_map(|start| (start..length).step_by(runs))
                    .collect();
                super::sort_distinct_words(&mut words, usize::cmp);
                assert!(words.into_iter().eq(0..length));
            }
        }
    }

    #[test]
    fn empty_named_fields_do_not_reserve_the_template_length() {
        let template = "{x}".repeat(20_000) + "{y}";
        let result = format_named(&template, |name| match name {
            "x" => Some(FormatValue::Text("")),
            "y" => Some(FormatValue::Text("雪")),
            _ => None,
        })
        .unwrap_or_else(|_| panic!("valid named fields"));
        assert_eq!(result.as_str(), "雪");
        assert!(!result.is_heap_allocated());
    }

    #[test]
    fn stable_permutations_preserve_ties_in_both_directions() {
        let keys = [2, 1, 2, 1, 0];
        for (reverse, expected) in [(false, [4, 1, 3, 0, 2]), (true, [0, 2, 1, 3, 4])] {
            let mut order =
                ordered_indices(keys.len(), reverse, |a, b| keys[a].cmp(&keys[b])).unwrap();
            let mut values = [0, 1, 2, 3, 4];
            permute(&mut values, &mut order);
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn short_sorts_keep_the_permutation_inline() {
        for length in 0..=4 {
            let mut order = ordered_indices(length, true, |left, right| left.cmp(&right)).unwrap();
            assert!(!order.spilled());
            let mut values: Vec<_> = (0..length).collect();
            permute(&mut values, &mut order);
            assert!(values.into_iter().eq((0..length).rev()));
        }
    }

    #[test]
    fn direct_string_sorts_preserve_equal_owners_in_both_directions() {
        let texts = ["é", "", "é", "z", ""].map(|s| StrOwner::new(Str::from(s)));
        for (reverse, order) in [(0, [1, 4, 3, 0, 2]), (1, [0, 2, 3, 1, 4])] {
            // SAFETY: the list and its string owners stay rooted throughout the sort.
            unsafe {
                let list = crate::ObjectOwner::from_raw(super::super::imre_seq_new(3));
                let pointer = crate::ObjectOwner::as_ptr(&list);
                for text in &texts {
                    super::super::imre_seq_append(pointer, StrOwner::as_ptr(text) as usize, 3);
                }
                imre_sort_by_keys(pointer.cast(), 3, pointer.cast(), 3, reverse);
                list.0.with(|storage| {
                    assert_eq!(
                        storage
                            .sequence()
                            .ok()
                            .expect("fixture sequence")
                            .values
                            .as_slice(),
                        order.map(|i| StrOwner::as_ptr(&texts[i]) as usize),
                    );
                });
            }
            assert!(texts.iter().all(|text| StrOwner::strong_count(text) == 1));
        }
    }

    #[test]
    fn sorting_shared_prefixes_uses_unicode_byte_order_and_stable_ties() {
        for (prefix, count) in [("", 96), ("a-long-directory-name/", 1025)] {
            let keys: Vec<_> = (0..count)
                .map(|i| StrOwner::new(Str::from(format!("{prefix}{}", ["é", "è", "ê"][i % 3]))))
                .collect();
            for reverse in [false, true] {
                let mut expected: Vec<_> = keys.iter().collect();
                expected.sort_by(|a, b| if reverse { b.cmp(a) } else { a.cmp(b) });
                let mut words: Vec<_> = keys.iter().map(|s| StrOwner::as_ptr(s) as usize).collect();
                // SAFETY: keys owns each immutable string until the comparison finishes.
                unsafe { sort_strings(&mut words, StringSort::Stable { reverse }) };
                assert!(
                    expected
                        .into_iter()
                        .map(|s| StrOwner::as_ptr(s) as usize)
                        .eq(words)
                );
            }
        }
    }

    #[test]
    fn failed_key_restores_owners_and_drops_temporary_additions() {
        let original = StrOwner::new(Str::from("original"));
        let addition = StrOwner::new(Str::from("addition"));
        // SAFETY: both strings and both distinct list handles stay rooted. The
        // saved list is released exactly once after restoration.
        unsafe {
            let list = crate::ObjectOwner::from_raw(super::super::imre_seq_new(3));
            let alias = crate::ObjectOwner::clone(&list);
            let target = crate::ObjectOwner::as_ptr(&list);
            super::super::imre_seq_append(target, StrOwner::as_ptr(&original) as usize, 3);
            let saved =
                crate::ObjectOwner::from_raw(imre_sort_detach(target.cast(), 3).cast::<Object>());
            assert!(alias.0.with(|storage| {
                storage
                    .sequence()
                    .ok()
                    .expect("fixture sequence")
                    .values
                    .is_empty()
            }));
            assert_eq!(StrOwner::strong_count(&original), 2);
            super::super::imre_seq_append(target, StrOwner::as_ptr(&addition) as usize, 3);
            imre_sort_restore(
                target.cast(),
                3,
                crate::ObjectOwner::as_ptr(&saved).cast(),
                0,
            );
            assert_eq!(
                alias
                    .0
                    .with(|storage| storage.sequence().ok().expect("fixture sequence").values[0]),
                StrOwner::as_ptr(&original) as usize,
            );
            drop(saved);
            assert_eq!(StrOwner::strong_count(&addition), 1);
            assert_eq!(StrOwner::strong_count(&original), 2);
        }
        assert_eq!(StrOwner::strong_count(&original), 1);
    }

    #[test]
    fn speculative_range_capacity_does_not_mark_integer_sort_as_modified() {
        // SAFETY: the buffers are distinct; the range words encode 0, 8, and 1.
        unsafe {
            let target = crate::new_list(vec![crate::Int(5)]);
            let saved = imre_sort_detach(target.cast(), 2);
            crate::imre_list_reserve_range(target, 1, 17, 3, 1);
            assert!(
                (*target)
                    .0
                    .with(|values| crate::is_sorting_empty_ints(values))
            );
            imre_sort_restore(target.cast(), 2, saved, 1);
            crate::imre_list_release(saved.cast());
            assert_eq!((*target).0.with(|values| values[0].0), 5);
            crate::imre_list_release(target);
        }
    }

    #[test]
    fn sorting_keeps_generic_element_owners_and_supports_aliased_integer_keys() {
        let texts = ["third", "first", "second"].map(|text| StrOwner::new(Str::from(text)));
        // SAFETY: every raw argument is rooted here; returned list handles each
        // transfer one reference, released after inspecting their owned elements.
        unsafe {
            let values = super::super::imre_seq_new(3);
            let keys = crate::imre_list_new();
            for (text, key) in texts.iter().zip([3, 1, 2]) {
                super::super::imre_seq_append(values, StrOwner::as_ptr(text) as usize, 3);
                crate::imre_list_append(keys, crate::Int::small(key).unwrap().0);
            }
            imre_sort_by_keys(values.cast(), 3, keys.cast(), 2, 0);
            for (index, expected) in ["first", "second", "third"].iter().enumerate() {
                let text = StrOwner::from_raw(
                    super::super::imre_seq_get_index(values, index) as *const Str
                );
                assert_eq!(text.as_str(), *expected);
            }
            assert!(texts.iter().all(|text| StrOwner::strong_count(text) == 2));
            imre_sort_by_keys(keys.cast(), 2, keys.cast(), 2, 0);
            for index in 0..3 {
                let value = crate::Int(crate::imre_list_get_index(keys, index));
                assert_eq!(value.as_small(), Some(index as i64 + 1));
            }
            super::super::imre_object_release(values);
            crate::imre_list_release(keys);
        }
        assert!(texts.iter().all(|text| StrOwner::strong_count(text) == 1));
    }

    #[test]
    fn joining_and_replacement_preserve_unicode_and_empty_matches() {
        let separator = StrOwner::new(Str::from("·"));
        let old = StrOwner::new(Str::from(""));
        let new = StrOwner::new(Str::from("/"));
        let texts = ["α", "", "🙂"].map(|text| StrOwner::new(Str::from(text)));
        // SAFETY: inputs stay rooted until both fresh returned owners are dropped.
        unsafe {
            let values = super::super::imre_seq_new(3);
            for text in &texts {
                super::super::imre_seq_append(values, StrOwner::as_ptr(text) as usize, 3);
            }
            let joined = StrOwner::from_raw(imre_str_join(StrOwner::as_ptr(&separator), values));
            assert_eq!(joined.as_str(), "α··🙂");
            let replaced = StrOwner::from_raw(imre_str_replace(
                StrOwner::as_ptr(&joined),
                StrOwner::as_ptr(&old),
                StrOwner::as_ptr(&new),
            ));
            assert_eq!(replaced.as_str(), "/α/·/·/🙂/");
            super::super::imre_object_release(values);
        }
        assert!(texts.iter().all(|text| StrOwner::strong_count(text) == 1));
    }

    #[test]
    fn unchanged_replacements_return_an_owner_of_the_source() {
        let value = StrOwner::new(Str::from("word".repeat(1024)));
        for (old, new) in [("absent", "!"), ("word", "word"), ("", "")] {
            let old = StrOwner::new(Str::from(old));
            let new = StrOwner::new(Str::from(new));
            // SAFETY: the three inputs stay rooted, and the result transfers an owner.
            let result = unsafe {
                StrOwner::from_raw(imre_str_replace(
                    StrOwner::as_ptr(&value),
                    StrOwner::as_ptr(&old),
                    StrOwner::as_ptr(&new),
                ))
            };
            assert!(StrOwner::ptr_eq(&result, &value));
        }
    }

    #[test]
    fn single_item_join_keeps_an_owner_after_the_input_list_is_released() {
        let separator = StrOwner::new(Str::from("unused separator"));
        let text = StrOwner::new(Str::from("an independently owned λ🙂 string"));
        // SAFETY: the list retains text; joining transfers a second string owner.
        unsafe {
            let values = super::super::imre_seq_new(3);
            super::super::imre_seq_append(values, StrOwner::as_ptr(&text) as usize, 3);
            let joined = StrOwner::from_raw(imre_str_join(StrOwner::as_ptr(&separator), values));
            assert!(StrOwner::ptr_eq(&text, &joined));
            super::super::imre_object_release(values);
            drop(text);
            assert_eq!(StrOwner::strong_count(&joined), 1);
            assert_eq!(joined.as_str(), "an independently owned λ🙂 string");
        }
    }

    #[test]
    fn fixed_float_formats_preserve_rounding_sign_and_nonfinite_spelling() {
        let plus = u32::from(b'+') << 8;
        let space = u32::from(b' ') << 8;
        for (value, precision, flags, expected) in [
            (1.25, 1, 0, "1.2"),
            (2.675, 2, 0, "2.67"),
            (-0.0, 2, 0, "-0.00"),
            (0.125, 0, 1, "12%"),
            (f64::INFINITY, 3, 1, "inf%"),
            (f64::NEG_INFINITY, 0, 0, "-inf"),
            (-f64::NAN, 2, 0, "nan"),
            (-f64::NAN, 2, plus | 1, "+nan%"),
            (0.0, 2, space, " 0.00"),
            (-0.0, 2, plus | 1, "-0.00%"),
        ] {
            // SAFETY: the formatter transfers exactly one fresh string owner.
            let actual = unsafe { StrOwner::from_raw(imre_float_format(value, precision, flags)) };
            assert_eq!(actual.as_str(), expected);
        }
    }
}
