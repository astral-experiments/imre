//! Generated record operations recurse through the existing concrete word kinds.
//! Structural traversal uses explicit cursors. Exception repr may call Python;
//! it roots outstanding cursors before releasing control to the callback.

use crate::StrOwner;
use std::cmp::Ordering;
use std::fmt::Write;

use num_traits::ToPrimitive;

use super::exceptions::Error;
use super::records::descriptor;
use super::words::{NULLABLE, WordKind};
use super::{Bytes, Int, List, Object, Storage, Str, fail, object};

const GENERATED_EQ: u32 = 1;
const GENERATED_REPR: u32 = 2;
const HAS_REPR_TEXT: u32 = 1 << 4;
const REPR_TEXT_FIELD: u32 = 1 << 2;
const TUPLE: u32 = 1;
const MODULE_SPEC: u32 = 2;

/// Compares an exact immutable tuple lexicographically. The checker excludes
/// nullable and float fields. Comparison can fail when reserving traversal frames.
///
/// # Safety
/// Both pointers root initialized tuples of the same exact ordered type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_compare(left: *const usize, right: *const usize) -> i32 {
    // SAFETY: the caller proves the identical static ordered tuple shape.
    unsafe { tuple_order(left, right) }.map_or_else(
        || super::exceptions::Error::allocation().returning(0),
        |order| order as i32,
    )
}

/// # Safety
/// Both rooted tuples share a descriptor containing only ordered scalar/tuple fields.
pub(super) unsafe fn tuple_order(left: *const usize, right: *const usize) -> Option<Ordering> {
    if left == right {
        return Some(Ordering::Equal);
    }
    let mut pending = Vec::new();
    let mut current = (left, right, 0);
    'tuples: loop {
        let (left_tuple, right_tuple, position) = current;
        // SAFETY: the exact tuple type supplies this live immutable descriptor.
        let layout = unsafe { descriptor(left_tuple) };
        for index in position..layout.length {
            // SAFETY: both roots own corresponding tuples with the same layout.
            let order = unsafe {
                let kind = WordKind::new(*layout.kinds.add(index));
                let left = kind.read_record_field(left_tuple.add(index));
                let right = kind.read_record_field(right_tuple.add(index));
                match kind.raw() {
                    0 => Ordering::Equal,
                    1 => left.cmp(&right),
                    10 => super::string_enums::compare(left, right),
                    2 if left & right & 1 != 0 => (left as isize).cmp(&(right as isize)),
                    2 => crate::imre_int_compare(left, right).cmp(&0),
                    3 => (&*(left as *const Str)).cmp(&*(right as *const Str)),
                    6 if left == right => Ordering::Equal,
                    6 => {
                        // Resume the siblings only if this child compares equal.
                        if index + 1 < layout.length {
                            pending.try_reserve(1).ok()?;
                            pending.push((left_tuple, right_tuple, index + 1));
                        }
                        current = (left as *const usize, right as *const usize, 0);
                        continue 'tuples;
                    }
                    8 => (&*(left as *const Bytes)).cmp(&*(right as *const Bytes)),
                    _ => unreachable!("the checker proves ordered immutable tuple fields"),
                }
            };
            if order != Ordering::Equal {
                return Some(order);
            }
        }
        let Some(next) = pending.pop() else {
            return Some(Ordering::Equal);
        };
        current = next;
    }
}

/// Uses Python 3.13+ dataclass equality: identical objects compare equal before
/// fieldwise comparison, which deliberately has no tuple-style NaN shortcut.
/// Ordinary classes and dataclasses with `eq=False` retain identity equality.
///
/// # Safety
/// Both arguments are rooted, fully initialized records with immutable live
/// descriptors. No mutation or user callback may overlap this operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_equal(left: *const usize, right: *const usize) -> u32 {
    // SAFETY: the ABI promises both checked record roots for the entire traversal.
    u32::from(unsafe { equal(left as usize, right as usize, WordKind::new(6)) })
}

pub(super) enum Comparison {
    Error(Error),
    Done(bool),
    Child {
        left: usize,
        right: usize,
        kind: WordKind,
        resume: Option<usize>,
    },
}

unsafe fn record_field(left: *const usize, right: *const usize, position: usize) -> Comparison {
    if left == right {
        return Comparison::Done(true);
    }
    // SAFETY: both pointers are rooted record payloads.
    let (left_layout, right_layout) = unsafe { (descriptor(left), descriptor(right)) };
    // SAFETY: semantic descriptors stay rooted through the comparison.
    let Some(semantics) = (unsafe { left_layout.semantics.as_ref() }) else {
        return Comparison::Done(false);
    };
    let distinct = !std::ptr::eq(left_layout, right_layout);
    if distinct && !left_layout.same_class(right_layout) {
        // Tuples can compare across layouts; records can share a nominal class.
        // SAFETY: the immutable descriptors cover each initialized field.
        let compatible = unsafe {
            semantics.kind == TUPLE
                && right_layout
                    .semantics
                    .as_ref()
                    .is_some_and(|other| other.kind == TUPLE)
                && left_layout.length == right_layout.length
        };
        if !compatible {
            return Comparison::Done(false);
        }
    }
    if semantics.kind != TUPLE && semantics.flags & GENERATED_EQ == 0 {
        return Comparison::Done(false);
    }
    for index in position..left_layout.length {
        // SAFETY: the descriptor covers every initialized payload word. Tuples
        // compare all fields; classes have one immutable flag per declared field.
        unsafe {
            if semantics.kind != TUPLE && (*semantics.fields.add(index)).flags & GENERATED_EQ == 0 {
                continue;
            }
            let kind = WordKind::new(*left_layout.kinds.add(index));
            // Check the kind exactly once per visited field, including across
            // resumptions after comparing a nested tuple.
            if distinct && kind.raw() != *right_layout.kinds.add(index) {
                return Comparison::Done(false);
            }
            let left_word = kind.read_record_field(left.add(index));
            let right_word = kind.read_record_field(right.add(index));
            if left_layout.is_value_struct() && kind.raw() == 7 {
                if super::structs::float_key(left_word) != super::structs::float_key(right_word) {
                    return Comparison::Done(false);
                }
                continue;
            }
            if !matches!(kind.raw() & !NULLABLE, 5 | 6 | 9) {
                if !scalar_equal(left_word, right_word, kind) {
                    return Comparison::Done(false);
                }
                continue;
            }
            return Comparison::Child {
                left: left_word,
                right: right_word,
                kind,
                resume: (index + 1 < left_layout.length).then_some(index + 1),
            };
        }
    }
    Comparison::Done(true)
}

/// Canonical integers never box a value in the small-integer range. Mixed
/// representations are therefore unequal, and equality needs no temporary
/// BigInt conversion or call through the cold ordering-comparison path.
unsafe fn integer_equal(left: usize, right: usize) -> bool {
    if left & 1 != right & 1 {
        return false;
    }
    if left & 1 != 0 {
        return left == right;
    }
    // SAFETY: both rooted words are canonical heap integers in this branch.
    let (left, right) = unsafe { (Int::borrow(left), Int::borrow(right)) };
    left.big() == right.big()
}

/// Compares exact representations without transferring owners. Optional float
/// owners are unboxed before equality, preserving IEEE NaN behavior; the checker
/// excludes float-containing containers whose boxed identity would be observable.
///
/// # Safety
/// Each word is rooted and uses `kind`. All descendant ownership graphs are
/// acyclic and remain immutable throughout this nonreentrant traversal.
pub(super) unsafe fn equal(left: usize, right: usize, kind: WordKind) -> bool {
    // SAFETY: both words remain rooted under the shared descriptor.
    unsafe { try_equal(left, right, kind) }.unwrap_or_else(|error| error.returning(false))
}

/// # Safety
/// The words must remain rooted and match the same concrete or nullable kind.
pub(super) unsafe fn try_equal(left: usize, right: usize, kind: WordKind) -> Result<bool, Error> {
    let mut pending = Vec::new();
    let mut current = (left, right, kind, 0);
    loop {
        let (left, right, kind, position) = current;
        let raw = kind.raw();
        let step = if raw & NULLABLE != 0 && (left == 0 || right == 0) {
            Comparison::Done(left == right)
        } else {
            // SAFETY: the roots keep every copied child word alive. No storage
            // reference or buffer address is kept across a comparison step.
            unsafe {
                match raw & !NULLABLE {
                    5 => object_element(left as *const Object, right as *const Object, position),
                    6 => record_field(left as *const usize, right as *const usize, position),
                    13 => {
                        let (a, left) = super::annotations::member(left);
                        let (b, right) = super::annotations::member(right);
                        if a != b {
                            Comparison::Done(false)
                        } else {
                            Comparison::Child {
                                left,
                                right,
                                kind: a,
                                resume: None,
                            }
                        }
                    }
                    9 => {
                        let (left, left_kind) = super::unions::member(left);
                        let (right, right_kind) = super::unions::member(right);
                        if left_kind != right_kind {
                            let numbers = match (left_kind.base(), right_kind.base()) {
                                (1, 2) => Some((left, right)),
                                (2, 1) => Some((right, left)),
                                _ => None,
                            };
                            Comparison::Done(numbers.is_some_and(|(flag, integer)| {
                                Int::borrow(integer).big().to_usize() == Some(flag)
                            }))
                        } else {
                            Comparison::Child {
                                left,
                                right,
                                kind: left_kind,
                                resume: None,
                            }
                        }
                    }
                    _ => Comparison::Done(scalar_equal(left, right, kind)),
                }
            }
        };
        match step {
            Comparison::Error(error) => return Err(error),
            Comparison::Done(false) => return Ok(false),
            Comparison::Done(true) => match pending.pop() {
                Some(next) => current = next,
                None => return Ok(true),
            },
            Comparison::Child {
                left: child_left,
                right: child_right,
                kind: child_kind,
                resume,
            } => {
                if let Some(position) = resume {
                    pending.try_reserve(1).map_err(|_| Error::memory_error())?;
                    pending.push((left, right, kind, position));
                }
                current = (child_left, child_right, child_kind, 0);
            }
        }
    }
}

/// # Safety
/// The words are present, rooted values of this concrete scalar or integer-list kind.
unsafe fn scalar_equal(left: usize, right: usize, kind: WordKind) -> bool {
    let raw = kind.raw();
    if raw & NULLABLE != 0 && (left == 0 || right == 0) {
        return left == right;
    }
    // SAFETY: the shared kind selects the exact scalar or rooted allocation.
    unsafe {
        match raw & !NULLABLE {
            12 => super::annotations::equal(left, right),
            0 | 1 | 10 => left == right,
            2 => integer_equal(left, right),
            3 | 11 => *(left as *const Str) == *(right as *const Str),
            4 => {
                if left == right {
                    return true;
                }
                (&*(left as *const List)).0.with(|left| {
                    (&*(right as *const List)).0.with(|right| {
                        left.len() == right.len()
                            && left
                                .iter()
                                .zip(right)
                                .all(|(left, right)| integer_equal(left.0, right.0))
                    })
                })
            }
            7 if raw & NULLABLE != 0 => *(left as *const f64) == *(right as *const f64),
            7 => f64::from_bits(left as u64) == f64::from_bits(right as u64),
            14 => left == right,
            8 => *(left as *const Bytes) == *(right as *const Bytes),
            _ => unreachable!("WordKind validates concrete representations"),
        }
    }
}

/// Compares checked containers, including ordered sequences and integer counters.
///
/// # Safety
/// Both inputs are rooted objects of the same checked container type. Element
/// comparisons have no user callbacks and neither container changes during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sequence_equal(left: *const Object, right: *const Object) -> u32 {
    // SAFETY: the compiler verified both sequence layouts before calling.
    u32::from(unsafe { equal(left as usize, right as usize, WordKind::new(5)) })
}

unsafe fn object_element(left: *const Object, right: *const Object, position: usize) -> Comparison {
    if left == right {
        return Comparison::Done(true);
    }
    // SAFETY: both objects remain rooted and immutable. Each step returns only
    // copied words; the storage closures end before their children are visited.
    unsafe {
        object(left).0.with(|left| {
            object(right).0.with(|right| match (left, right) {
                (Storage::Sequence(left), Storage::Sequence(right)) => {
                    match (left.rows(), right.rows()) {
                        (Some(a), Some(b)) => return Comparison::Done(a.equal(b)),
                        (Some(a), None) => return Comparison::Done(a.equal_words(right)),
                        (None, Some(b)) => return Comparison::Done(b.equal_words(left)),
                        (None, None) => {}
                    }
                    let left = match left.resolved() {
                        Ok(v) => v,
                        Err(error) => return Comparison::Error(error),
                    };
                    let right = match right.resolved() {
                        Ok(v) => v,
                        Err(error) => return Comparison::Error(error),
                    };
                    if left.kind != right.kind || left.values.len() != right.values.len() {
                        return Comparison::Done(false);
                    }
                    if !matches!(left.kind.raw() & !NULLABLE, 5 | 6 | 9) {
                        return Comparison::Done(left.values.iter().zip(&right.values).all(
                            |(&left_word, &right_word)| {
                                scalar_equal(left_word, right_word, left.kind)
                            },
                        ));
                    }
                    if position == left.values.len() {
                        return Comparison::Done(true);
                    }
                    Comparison::Child {
                        left: left.values[position],
                        right: right.values[position],
                        kind: left.kind,
                        resume: (position + 1 < left.values.len()).then_some(position + 1),
                    }
                }
                (Storage::ModuleMember(left), Storage::ModuleMember(right)) => {
                    Comparison::Done(left.equal_member(right))
                }
                (Storage::ObjectValue(l), Storage::ObjectValue(r)) => {
                    super::boxed_values::try_equal((l.kind, l.word), (r.kind, r.word))
                        .map_or_else(Comparison::Error, Comparison::Done)
                }
                (Storage::SimpleNamespace(left), Storage::SimpleNamespace(right)) => {
                    object_element(
                        crate::ObjectOwner::as_ptr(left),
                        crate::ObjectOwner::as_ptr(right),
                        position,
                    )
                }
                (Storage::Dictionary(left), Storage::Dictionary(right)) => {
                    left.compare_entry(right, position)
                }
                (Storage::Deque(left), Storage::Deque(right)) => {
                    left.compare_entry(right, position)
                }
                (Storage::Calendar(left), Storage::Calendar(right)) => {
                    Comparison::Done(super::calendar::equal(left, right))
                }
                (Storage::Set(left), Storage::Set(right)) => Comparison::Done(left.equal(right)),
                (Storage::Json(left), Storage::Json(right)) => left
                    .equal(right)
                    .map_or_else(Comparison::Error, Comparison::Done),
                (Storage::Document(left), Storage::Document(right)) => left
                    .view()
                    .and_then(|left| left.equal(&right.view()?))
                    .map_or_else(Comparison::Error, Comparison::Done),
                (
                    left @ (Storage::Path { .. } | Storage::PurePath(_)),
                    right @ (Storage::Path { .. } | Storage::PurePath(_)),
                ) => Comparison::Done(left.path_equal(right)),
                _ => Comparison::Done(false),
            })
        })
    }
}

/// A representation can outgrow the available address space while its input is live.
pub(super) struct ReprWriter<'a>(pub(super) &'a mut String);

impl Write for ReprWriter<'_> {
    #[inline]
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if text.len() > self.0.capacity() - self.0.len() {
            self.0
                .try_reserve(text.len())
                .or_else(|_| self.0.try_reserve_exact(text.len()))
                .map_err(|_| std::fmt::Error)?;
        }
        self.0.push_str(text);
        Ok(())
    }
}

/// Obtain a buffer with room for the whole quoted spelling before emitting escapes.
pub(super) trait ReprTarget {
    fn quoted_buffer(&mut self, len: usize) -> Result<&mut String, std::fmt::Error>;
}

impl ReprTarget for String {
    fn quoted_buffer(&mut self, len: usize) -> Result<&mut String, std::fmt::Error> {
        if self.is_empty() {
            self.reserve_exact(len);
        } else {
            self.reserve(len);
        }
        Ok(self)
    }
}

impl ReprTarget for ReprWriter<'_> {
    fn quoted_buffer(&mut self, len: usize) -> Result<&mut String, std::fmt::Error> {
        if len > self.0.capacity() - self.0.len() {
            if self.0.is_empty() {
                self.0.try_reserve_exact(len)
            } else {
                self.0
                    .try_reserve(len)
                    .or_else(|_| self.0.try_reserve_exact(len))
            }
            .map_err(|_| std::fmt::Error)?;
        }
        Ok(self.0)
    }
}

/// Returns one owned Python representation, borrowing every input until the
/// string is complete. Compiler validation excludes resource representations.
///
/// # Safety
/// `word` uses `kind` and roots an initialized acyclic value throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_value_repr(word: usize, kind: u32) -> *const Str {
    let mut output = String::new();
    // SAFETY: the caller supplies the exact checked representation and root.
    let result = unsafe { try_write_repr(&mut ReprWriter(&mut output), word, WordKind::new(kind)) };
    super::exceptions::finish_display(
        result.and_then(|()| super::exceptions::rendered(output.into())),
    )
}

/// # Safety
/// The caller roots the value. The walker roots pending cursors before calling Python.
pub(super) unsafe fn try_write_repr(
    output: &mut ReprWriter<'_>,
    word: usize,
    kind: WordKind,
) -> std::fmt::Result {
    // SAFETY: the caller supplies a rooted value with its concrete kind.
    unsafe { write_repr_with(output, word, kind, true) }
}

/// # Safety
/// The caller roots the input throughout formatting.
pub(super) unsafe fn try_write_unchecked_repr(
    output: &mut ReprWriter<'_>,
    word: usize,
    kind: WordKind,
) -> std::fmt::Result {
    // SAFETY: the caller roots the value under its concrete kind.
    unsafe { write_repr_with(output, word, kind, false) }
}

type ReprStack = smallvec::SmallVec<[ReprTask; 4]>;
unsafe fn write_repr_with(
    output: &mut ReprWriter<'_>,
    word: usize,
    kind: WordKind,
    mut check_digits: bool,
) -> std::fmt::Result {
    let mut pending = ReprStack::new();
    let mut callback_roots = std::collections::HashMap::new();
    let mut current = Some(ReprTask::Word(word, kind));
    while let Some(task) = current.take().or_else(|| pending.pop()) {
        // SAFETY: tasks store values and positions, never references to mutable
        // storage. A callback roots every pending value before it may mutate a parent.
        current = unsafe {
            match task {
                ReprTask::Word(word, kind) => match kind.raw() & !NULLABLE {
                    5 if word != 0 => Some(ReprTask::Object(word as *const Object, 0)),
                    6 if word != 0 => Some(ReprTask::Record(word as *const usize, 0, 0)),
                    9 if word != 0 => {
                        let (member, kind) = super::unions::member(word);
                        Some(ReprTask::Word(member, kind))
                    }
                    _ => {
                        write_scalar(output, word, kind, check_digits)?;
                        None
                    }
                },
                ReprTask::Object(owner, position) => {
                    object(owner).0.with(|storage| match storage {
                        Storage::ObjectValue(field) => {
                            Ok(Some(ReprTask::Word(field.word, field.kind)))
                        }
                        Storage::Context(_)
                        | Storage::ContextVar(_)
                        | Storage::ContextToken(_)
                        | Storage::TokenMissing => Ok(Some(ReprTask::Context(
                            super::contextvars::repr_view(owner, storage),
                        ))),
                        Storage::File(file) if let Some((name, _)) = file.memory_class() => {
                            write!(output, "<{name} object at {owner:p}>")?;
                            Ok(None)
                        }
                        Storage::Calendar(value) => {
                            super::calendar::write_repr(value, output)?;
                            Ok(None)
                        }
                        Storage::ModuleMember(member) => {
                            let text = member.text(1);
                            if text.is_null() { return Err(std::fmt::Error); } else {
                                // SAFETY: the metadata formatter returns an independently owned string.
                                output.write_str(StrOwner::from_raw(text).as_str())?;
                            }
                            Ok(None)
                        }
                        Storage::Exception(_) => Ok(Some(ReprTask::Exception(owner))),
                        Storage::SourceModule { name, filename } => {
                            output.write_str("<module ")?;
                            super::exceptions::write_quoted_repr(name, output)?;
                            output.write_str(" from ")?;
                            super::exceptions::write_quoted_repr(filename, output)?;
                            output.write_char('>')?;
                            Ok(None)
                        }
                        Storage::Plain => {
                            write!(output, "<object object at {owner:p}>")?;
                            Ok(None)
                        }
                        Storage::FunctoolsPlaceholder { class } => {
                            output.write_str(if *class {
                                "<class 'functools._PlaceholderType'>"
                            } else {
                                "Placeholder"
                            })?;
                            Ok(None)
                        }
                        Storage::StrDescriptor(index) => {
                            write!(
                                output,
                                "<method '{}' of 'str' objects>",
                                super::inspect::descriptor_name(*index)
                            )?;
                            Ok(None)
                        }
                        Storage::ProcessSelectorClass(select) => {
                            output.write_str(if *select {
                                "<class 'selectors.SelectSelector'>"
                            } else {
                                "<class 'selectors.PollSelector'>"
                            })?;
                            Ok(None)
                        }
                        Storage::IteratorAdapter(adapter) => {
                            adapter.write_repr(output, owner)?;
                            Ok(None)
                        }
                        Storage::Repeat(repeat) => {
                            if position == 0 {
                                output.write_str("repeat(")?;
                                pending.try_reserve(1).map_err(|_| std::fmt::Error)?;
                                pending.push(ReprTask::Object(owner, 1));
                                Ok(Some(ReprTask::Word(repeat.value.word, repeat.value.kind)))
                            } else {
                                if let Some(count) = repeat.remaining {
                                    write!(output, ", {count}")?;
                                }
                                output.write_char(')')?;
                                Ok(None)
                            }
                        }
                        Storage::Sequence(sequence) => {
                            if let Some(rows) = sequence.rows() {
                                output.write_char('[')?;
                                for i in 0..rows.len() {
                                    if i != 0 {
                                        output.write_str(", ")?;
                                    }
                                    // Struct fields are scalars: formatting never queues a child.
                                    let next = fields_repr(
                                        output,
                                        rows.row(i).as_ptr(),
                                        rows.layout(),
                                        0,
                                        0,
                                        &mut pending,
                    check_digits,
                                    )?;
                                    debug_assert!(next.is_none());
                                }
                                output.write_char(']')?;
                                return Ok(None);
                            }
                            let values = sequence.resolved().map_err(|error| {
                                error.raise();
                                std::fmt::Error
                            })?;
                            if position == 0 && sequence.mock_call_list.get()
                                && write_mock_history(output, &values.values, values.kind)?
                            {
                                return Ok(None);
                            }
                            if position == 0 {
                                output.write_char('[')?;
                            }
                            for (index, &value) in values.values.iter().enumerate().skip(position) {
                                if index != 0 {
                                    output.write_str(", ")?;
                                }
                                if value != 0 && matches!(values.kind.raw() & !NULLABLE, 5 | 6 | 9)
                                {
                                    pending.try_reserve(1).map_err(|_| std::fmt::Error)?;
                                    pending.push(ReprTask::Object(owner, index + 1));
                                    return Ok(Some(ReprTask::Word(value, values.kind)));
                                }
                                write_scalar(output, value, values.kind, check_digits)?;
                            }
                            output.write_char(']')?;
                            Ok(None)
                        }
                        Storage::Dictionary(mapping) => {
                            format_entry(owner, mapping.repr_entry(output, position)?, &mut pending)
                        }
                        Storage::ChainMap(maps) => {
                            if position == 0 {
                                if pending.iter().any(|task| matches!(task, ReprTask::Object(active, _) if *active == owner)) {
                                    output.write_str("...")?;
                                    return Ok(None);
                                }
                                output.write_str("ChainMap(")?;
                            }
                            let mapping = maps.0.with(|s| s.sequence_storage().values.get(position).copied());
                            if let Some(mapping) = mapping {
                                if position != 0 {
                                    output.write_str(", ")?;
                                }
                                pending.try_reserve(1).map_err(|_| std::fmt::Error)?;
                                pending.push(ReprTask::Object(owner, position + 1));
                                Ok(Some(ReprTask::Object(mapping as *const Object, 0)))
                            } else {
                                output.write_char(')')?;
                                Ok(None)
                            }
                        }
                        Storage::SimpleNamespace(dict) => {
                            let entry = dict.0.with(|state| {
                                state.dictionary().namespace_repr_entry(output, position)
                            })?;
                            if let Some((next, word, kind)) = entry {
                                pending.try_reserve(1).map_err(|_| std::fmt::Error)?;
                                pending.push(ReprTask::Object(owner, next));
                                Ok(Some(ReprTask::Word(word, kind)))
                            } else {
                                Ok(None)
                            }
                        }
                        Storage::Set(set) => {
                            format_entry(owner, set.repr_entry(output, position)?, &mut pending)
                        }
                        Storage::Json(value) => {
                            value.try_write_repr(output).map_err(|error| {error.raise(); std::fmt::Error})?;
                            Ok(None)
                        }
                        Storage::Document(value) => {
                            match value.view() {
                                Ok(view) => view.try_write_repr(output).map_err(|error| {error.raise(); std::fmt::Error})?,
                                Err(error) => { error.raise(); return Err(std::fmt::Error); }
                            }
                            Ok(None)
                        }
                        Storage::PurePath(path) => {
                            output.write_str(path.class())?;
                            output.write_char('(')?;
                            path.write_posix_repr(output)?;
                            output.write_char(')')?;
                            Ok(None)
                        }
                        path @ Storage::Path { .. } => {
                            output.write_str("PosixPath(")?;
                            super::exceptions::write_quoted_repr(path.path(), output)?;
                            output.write_char(')')?;
                            Ok(None)
                        }
                        _ => fail("SystemError", "unsupported resource representation"),
                    })?
                }
                ReprTask::Context(view) => {
                    if let Some((word, kind, tail)) = view.begin(output)? {
                        pending.try_reserve(1).map_err(|_| std::fmt::Error)?;
                        pending.push(ReprTask::ContextEnd(tail, check_digits));
                        check_digits = true;
                        Some(ReprTask::Word(word, kind))
                    } else {
                        None
                    }
                }
                ReprTask::ContextEnd(tail, outer_check_digits) => {
                    check_digits = outer_check_digits;
                    tail.write(output)?;
                    None
                }
                ReprTask::Exception(owner) => {
                    // A custom repr can remove its own reference or a pending container.
                    // Retain the cursors outside every StorageCell borrow. Formatting
                    // ordinary values needs no extra owners.
                    for task in &pending {
                        if let Some((word, kind)) = task.root() {
                            let key = (word, kind.raw());
                            if !callback_roots.contains_key(&key) {
                                callback_roots.try_reserve(1).map_err(|_| std::fmt::Error)?;
                                callback_roots.insert(key, kind.acquire(word));
                            }
                        }
                    }
                    let exception: crate::ObjectOwner =
                        crate::ObjectOwner::acquire_word(owner as usize);
                    let text = super::exceptions::imre_exception_repr(crate::ObjectOwner::as_ptr(
                        &exception,
                    ));
                    if text.is_null() {
                        return Err(std::fmt::Error);
                    }
                    let text = StrOwner::from_raw(text);
                    if super::exceptions::imre_error_pending() != 0 {
                        return Err(std::fmt::Error);
                    }
                    output.write_str(&text)?;
                    None
                }
                ReprTask::Record(payload, position, written) => fields_repr(
                    output,
                    payload,
                    descriptor(payload),
                    position,
                    written,
                    &mut pending,
                    check_digits,
                )?,
                ReprTask::KeySeparator => {
                    output.write_str(": ")?;
                    None
                }
            }
        };
    }
    Ok(())
}

enum ReprTask {
    Context(super::contextvars::ReprView),
    ContextEnd(super::contextvars::ReprEnd, bool),
    Word(usize, WordKind),
    Object(*const Object, usize),
    Exception(*const Object),
    Record(*const usize, usize, usize),
    KeySeparator,
}

impl ReprTask {
    fn root(&self) -> Option<(usize, WordKind)> {
        match *self {
            Self::ContextEnd(_, _) => None,
            Self::Context(ref view) => view.owner().map(|owner| (owner as usize, WordKind::new(5))),
            Self::Word(word, kind) => Some((word, kind)),
            Self::Object(owner, _) | Self::Exception(owner) => {
                Some((owner as usize, WordKind::new(5)))
            }
            Self::Record(record, _, _) => Some((record as usize, WordKind::new(6))),
            Self::KeySeparator => None,
        }
    }
}

pub(super) struct ReprEntry {
    pub next: usize,
    pub key: (usize, WordKind),
    pub value: Option<(usize, WordKind)>,
}

fn format_entry(
    owner: *const Object,
    entry: Option<ReprEntry>,
    pending: &mut ReprStack,
) -> Result<Option<ReprTask>, std::fmt::Error> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    pending
        .try_reserve(if entry.value.is_some() { 3 } else { 1 })
        .map_err(|_| std::fmt::Error)?;
    pending.push(ReprTask::Object(owner, entry.next));
    if let Some((word, kind)) = entry.value {
        pending.push(ReprTask::Word(word, kind));
        pending.push(ReprTask::KeySeparator);
    }
    Ok(Some(ReprTask::Word(entry.key.0, entry.key.1)))
}

/// # Safety
/// The caller roots the exact integer during formatting.
pub(super) unsafe fn write_int_repr(output: &mut impl Write, word: usize) -> std::fmt::Result {
    // SAFETY: the caller roots the integer for this borrow.
    let value = unsafe { Int::borrow(word) };
    if let Some(small) = value.as_small() {
        write!(output, "{small}")
    } else {
        let value = value.big();
        if let Err(error) = crate::integer_text::check_decimal(&value) {
            error.raise();
            return Err(std::fmt::Error);
        }
        write!(output, "{value}")
    }
}

/// # Safety
/// The rooted word has its declared scalar, integer-list, or nullable representation.
unsafe fn write_scalar(
    output: &mut ReprWriter<'_>,
    word: usize,
    kind: WordKind,
    check_digits: bool,
) -> std::fmt::Result {
    let raw = kind.raw();
    if raw & NULLABLE != 0 && word == 0 {
        output.write_str("None")?;
        return Ok(());
    }
    // SAFETY: each branch borrows only its exact immutable scalar or container.
    unsafe {
        match raw & !NULLABLE {
            0 => output.write_str("None")?,
            10 => super::string_enums::write_repr(output, word)?,
            12 => super::annotations::write_repr(output, word)?,
            14 => super::typing::write_repr(output, word)?,
            13 => {
                let (member, value) = super::annotations::member(word);
                write_repr_with(output, value, member, false)?;
            }
            11 => write!(output, "<class '{}'>", *(word as *const Str))?,
            1 => {
                let value = if raw & NULLABLE != 0 { word - 1 } else { word };
                output.write_str(if value == 0 { "False" } else { "True" })?;
            }
            2 => {
                let value = Int::borrow(word);
                if let Some(value) = value.as_small() {
                    output.write_str(itoa::Buffer::new().format(value))?;
                } else {
                    let value = value.big();
                    if check_digits && let Err(error) = crate::integer_text::check_decimal(&value) {
                        error.raise();
                        return Err(std::fmt::Error);
                    }
                    write!(output, "{value}")?;
                }
            }
            3 => super::exceptions::write_quoted_repr(&*(word as *const Str), output)?,
            4 => (&*(word as *const List))
                .0
                .with(|values| -> std::fmt::Result {
                    output.write_char('[')?;
                    for (index, value) in values.iter().enumerate() {
                        if index != 0 {
                            output.write_str(", ")?;
                        }
                        write_scalar(output, value.0, WordKind::new(2), check_digits)?;
                    }
                    output.write_char(']')
                })?,
            7 => {
                let value = if raw & NULLABLE != 0 {
                    *(word as *const f64)
                } else {
                    f64::from_bits(word as u64)
                };
                output.write_str(&crate::floats::render(value))?;
            }
            8 => super::bytes::write_quoted_repr(output, &*(word as *const Bytes))?,
            _ => unreachable!("WordKind validates concrete representations"),
        }
    }
    Ok(())
}

/// The compiler emits these bytes from already validated source identifiers.
unsafe fn text<'a>(pointer: *const u8, length: usize) -> &'a str {
    // SAFETY: metadata owns this immutable, valid UTF-8 slice for the process.
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(pointer, length)) }
}

/// Borrows the cached display field while the caller roots the initialized record.
unsafe fn record_repr_text<'a>(
    payload: *const usize,
    layout: &super::records::RecordDescriptor,
) -> Option<&'a str> {
    let semantics = layout.class_semantics()?;
    if semantics.flags & HAS_REPR_TEXT == 0 {
        return None;
    }
    // SAFETY: the compiler emits one flag and kind for each physical word.
    // Reading the string borrows the record's owner without consuming a reference.
    unsafe {
        for index in 0..layout.length {
            if (*semantics.fields.add(index)).flags & REPR_TEXT_FIELD != 0 {
                let kind = WordKind::new(*layout.kinds.add(index));
                if kind.raw() != 3 {
                    return None;
                }
                let word = kind.read_record_field(payload.add(index));
                return (word as *const Str).as_ref().map(Str::as_str);
            }
        }
    }
    None
}

/// Writes a marked call history with its own 80-character wrapping.
unsafe fn write_mock_history(
    output: &mut ReprWriter<'_>,
    words: &[usize],
    kind: WordKind,
) -> Result<bool, std::fmt::Error> {
    if kind.raw() != 6 {
        return Ok(false);
    }
    let calls = words.iter().map(|&word| {
        let payload = word as *const usize;
        if payload.is_null() {
            return None;
        }
        // SAFETY: the sequence roots each record, and neither pass invokes Python.
        unsafe { record_repr_text(payload, descriptor(payload)) }
    });
    let punctuation = 2usize.saturating_add(words.len().saturating_sub(1).saturating_mul(2));
    let Some(width) = calls.clone().try_fold(punctuation, |width, call| {
        Some(width.saturating_add(call?.chars().take(81usize.saturating_sub(width)).count()))
    }) else {
        return Ok(false);
    };
    let separator = if width > 80 { ",\n " } else { ", " };
    output.write_char('[')?;
    for (index, call) in calls.enumerate() {
        if index != 0 {
            output.write_str(separator)?;
        }
        output.write_str(call.expect("validated immutable call text"))?;
    }
    output.write_char(']')?;
    Ok(true)
}

/// Format an instance without calling its Python display methods.
///
/// # Safety
/// The payload must remain rooted and have public source-class metadata.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_default_repr(payload: *const usize) -> *const Str {
    // SAFETY: source instances have an immutable concrete layout and qualified name.
    let semantics = unsafe { descriptor(payload).semantics.as_ref() }
        .unwrap_or_else(|| fail("SystemError", "internal records have no representation"));
    let mut output = String::new();
    let result = write_default_record_repr(&mut ReprWriter(&mut output), payload, semantics);
    super::exceptions::finish_display(
        result.and_then(|()| super::exceptions::rendered(output.into())),
    )
}

fn write_default_record_repr(
    output: &mut ReprWriter<'_>,
    payload: *const usize,
    semantics: &super::records::RecordSemantics,
) -> std::fmt::Result {
    // SAFETY: the compiler emits immutable UTF-8 identifiers for public record layouts.
    let name = unsafe { text(semantics.qualified_name, semantics.qualified_name_len) };
    write!(output, "<{name} object at {payload:p}>")
}

unsafe fn fields_repr(
    output: &mut ReprWriter<'_>,
    payload: *const usize,
    layout: &super::records::RecordDescriptor,
    position: usize,
    mut written: usize,
    pending: &mut ReprStack,
    check_digits: bool,
) -> Result<Option<ReprTask>, std::fmt::Error> {
    // SAFETY: public record representations always have static semantic metadata.
    let semantics = unsafe { layout.semantics.as_ref() }
        .unwrap_or_else(|| fail("SystemError", "internal records have no representation"));
    // SAFETY: checked descriptors contain valid names, flags, and field arrays.
    unsafe {
        if let Some(rendered) = record_repr_text(payload, layout) {
            output.write_str(rendered)?;
            return Ok(None);
        }
        if semantics.kind != TUPLE && semantics.flags & GENERATED_REPR == 0 {
            write_default_record_repr(output, payload, semantics)?;
            return Ok(None);
        }
        let plain_tuple = semantics.kind == TUPLE && semantics.name.is_null();
        if position == 0 {
            if !plain_tuple {
                output.write_str(text(semantics.name, semantics.name_len))?;
            }
            output.write_char('(')?;
        }
        for index in position..layout.length {
            let field = if plain_tuple {
                None
            } else {
                Some(&*semantics.fields.add(index))
            };
            if field.is_some_and(|field| field.flags & GENERATED_REPR == 0) {
                continue;
            }
            let kind = WordKind::new(*layout.kinds.add(index));
            let word = kind.read_record_field(payload.add(index));
            // ModuleSpec.__repr__ omits optional origins/locations only when None.
            if semantics.kind == MODULE_SPEC && matches!(index, 2 | 4) && word == 0 {
                continue;
            }
            if written != 0 {
                output.write_str(", ")?;
            }
            if let Some(field) = field {
                output.write_str(text(field.name, field.name_len))?;
                output.write_char('=')?;
            }

            written += 1;
            if word != 0 && matches!(kind.raw() & !NULLABLE, 5 | 6 | 9) {
                pending.try_reserve(1).map_err(|_| std::fmt::Error)?;
                pending.push(ReprTask::Record(payload, index + 1, written));
                return Ok(Some(ReprTask::Word(word, kind)));
            }
            write_scalar(output, word, kind, check_digits)?;
        }
        if plain_tuple && written == 1 {
            output.write_char(',')?;
        }
        output.write_char(')')?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::records::{
        RecordDescriptor, RecordField, RecordSemantics, imre_record_new, imre_record_release,
    };

    #[test]
    fn refused_repr_growth_leaves_the_buffer_usable() {
        let mut text = String::from("prefix ");
        let mut writer = ReprWriter(&mut text);
        assert!(writer.quoted_buffer(usize::MAX).is_err());
        super::super::exceptions::write_quoted_repr("a\nb", &mut writer).unwrap();
        assert_eq!(text, "prefix 'a\\nb'");
    }

    #[test]
    fn deeply_nested_tuple_order_resumes_only_after_equal_children() {
        let semantics = RecordSemantics {
            kind: TUPLE,
            flags: GENERATED_EQ | GENERATED_REPR,
            name: std::ptr::null(),
            name_len: 0,
            qualified_name: std::ptr::null(),
            qualified_name_len: 0,
            fields: std::ptr::null(),
        };
        let leaf_kinds = [2];
        let pair_kinds = [6, 2];
        let leaf_layout = RecordDescriptor {
            length: 1,
            kinds: leaf_kinds.as_ptr(),
            semantics: &semantics,
        };
        let pair_layout = RecordDescriptor {
            length: 2,
            kinds: pair_kinds.as_ptr(),
            semantics: &semantics,
        };
        let one = Int::small(1).unwrap().0;
        // SAFETY: each parent takes ownership of its child. The leaves remain
        // rooted until both complete tuples have been compared and released.
        unsafe {
            let mut roots = [imre_record_new(&leaf_layout), imre_record_new(&leaf_layout)];
            let right_leaf = roots[1];
            for root in &mut roots {
                root.write(one);
                for _ in 0..20_000 {
                    let parent = imre_record_new(&pair_layout);
                    parent.cast::<*mut usize>().write(*root);
                    parent.add(1).write(one);
                    *root = parent;
                }
            }
            let [left, right] = roots;
            assert_eq!(tuple_order(left, right), Some(Ordering::Equal));
            right.add(1).write(Int::small(0).unwrap().0);
            assert_eq!(tuple_order(left, right), Some(Ordering::Greater));
            right_leaf.write(Int::small(2).unwrap().0);
            assert_eq!(tuple_order(left, right), Some(Ordering::Less));
            assert_eq!(tuple_order(right, left), Some(Ordering::Greater));
            imre_record_release(left);
            imre_record_release(right);
        }
    }

    #[test]
    fn deeply_nested_sequences_compare_with_bounded_stack() {
        use crate::objects::{imre_object_release, imre_seq_append, imre_seq_new};
        let mut left = vec![imre_seq_new(2)];
        let mut right = vec![imre_seq_new(2)];
        // SAFETY: every sequence remains rooted. Each append borrows a word
        // of the declared type, and ancestors release before their children.
        unsafe {
            for chain in [&mut left, &mut right] {
                for _ in 0..20_000 {
                    let parent = imre_seq_new(5);
                    imre_seq_append(parent, *chain.last().unwrap() as usize, 5);
                    chain.push(parent);
                }
            }
            let (a, b) = (*left.last().unwrap(), *right.last().unwrap());
            assert_eq!(imre_sequence_equal(a, b), 1);
            imre_seq_append(left[0], 1, 2);
            assert_eq!(imre_sequence_equal(a, b), 0);
            for chain in [left, right] {
                for value in chain.into_iter().rev() {
                    imre_object_release(value);
                }
            }
        }
    }

    #[test]
    fn generated_equality_preserves_identity_and_fieldwise_nan_comparison() {
        let name = b"Reading";
        let field_name = b"value";
        let fields = [RecordField {
            name: field_name.as_ptr(),
            name_len: field_name.len(),
            flags: GENERATED_EQ | GENERATED_REPR,
        }];
        let semantics = RecordSemantics {
            kind: 0,
            flags: GENERATED_EQ | GENERATED_REPR,
            name: name.as_ptr(),
            name_len: name.len(),
            qualified_name: name.as_ptr(),
            qualified_name_len: name.len(),
            fields: fields.as_ptr(),
        };
        let kinds = [7];
        let layout = RecordDescriptor {
            length: 1,
            kinds: kinds.as_ptr(),
            semantics: &semantics,
        };
        // SAFETY: stack descriptors and names outlive both exact initialized records.
        unsafe {
            let left = imre_record_new(&layout);
            let right = imre_record_new(&layout);
            left.cast::<f64>().write(f64::NAN);
            right.cast::<f64>().write(f64::NAN);
            assert_eq!(imre_record_equal(left, left), 1);
            assert_eq!(imre_record_equal(left, right), 0);
            let rendered = StrOwner::from_raw(imre_value_repr(left as usize, 6));
            assert_eq!(rendered.as_str(), "Reading(value=nan)");
            left.cast::<f64>().write(-0.0);
            right.cast::<f64>().write(0.0);
            assert_eq!(imre_record_equal(left, right), 1);
            imre_record_release(left);
            imre_record_release(right);
        }
    }

    #[test]
    fn deeply_nested_sequence_repr_preserves_all_delimiters() {
        use crate::objects::{imre_object_release, imre_seq_append, imre_seq_new};
        let mut chain = vec![imre_seq_new(2)];
        // SAFETY: each sequence retains its declared child, and the separate
        // roots are released from the outermost sequence to the leaf.
        unsafe {
            for _ in 0..20_000 {
                let parent = imre_seq_new(5);
                imre_seq_append(parent, *chain.last().unwrap() as usize, 5);
                chain.push(parent);
            }
            let mut output = String::new();
            try_write_repr(
                &mut ReprWriter(&mut output),
                *chain.last().unwrap() as usize,
                WordKind::new(5),
            )
            .unwrap();
            assert_eq!(output, "[".repeat(chain.len()) + &"]".repeat(chain.len()));
            for value in chain.into_iter().rev() {
                imre_object_release(value);
            }
        }
    }
}
