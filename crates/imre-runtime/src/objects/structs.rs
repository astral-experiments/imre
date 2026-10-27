//! Scalar struct lists: dense rows, or one immutable default with sparse edits.
mod overrides;
use super::exceptions::{Error, Result};
use super::records::{RecordDescriptor, allocate_record};
use super::sequences::SliceIndices;
use super::words::{OwnedWord, WordKind};
use super::{MAX_LIST_LEN, Object, Storage, Words, object};
use overrides::Overrides;
use std::cell::OnceCell;

pub(super) fn float_key(word: usize) -> usize {
    let value = f64::from_bits(word as u64);
    if value.is_nan() {
        f64::NAN.to_bits() as usize
    } else if value == 0.0 {
        0
    } else {
        word
    }
}

enum RowStorage {
    Packed(Vec<usize>),
    Repeated(OwnedWord, Overrides),
}

pub(super) struct Rows {
    layout: &'static RecordDescriptor,
    len: usize,
    storage: RowStorage,
    // Legacy consumers that require a rooted word array may request a boxed view.
    boxed: OnceCell<Words>,
}

/// Fill the already reserved suffix by doubling a nonempty initialized pattern.
fn tile(words: &mut Vec<usize>, target: usize) {
    debug_assert!(!words.is_empty() && target <= words.capacity());
    while words.len() < target {
        words.extend_from_within(..words.len().min(target - words.len()));
    }
}

fn base_row(base: &OwnedWord, width: usize) -> &[usize] {
    // SAFETY: the shared owner roots a complete record of the list's exact layout.
    unsafe { std::slice::from_raw_parts(base.word as *const usize, width) }
}

impl Rows {
    pub(super) fn new(layout: &'static RecordDescriptor) -> Self {
        assert!(layout.is_value_struct() && layout.length != 0);
        Self {
            layout,
            len: 0,
            storage: RowStorage::Packed(Vec::new()),
            boxed: OnceCell::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn layout(&self) -> &RecordDescriptor {
        self.layout
    }

    fn direct_view(&self) -> Option<(*const usize, usize)> {
        match &self.storage {
            RowStorage::Packed(words) => Some((words.as_ptr(), usize::MAX)),
            RowStorage::Repeated(base, edits) if edits.len == 0 => {
                Some((base.word as *const usize, 0))
            }
            _ => None,
        }
    }

    /// A sort already has its permutation. Move only changed slots in a sparse
    /// list, publishing the new mapping after all allocations have succeeded.
    pub(super) fn reorder_shared(&mut self, order: &[usize]) -> Result<bool> {
        let RowStorage::Repeated(_, edits) = &mut self.storage else {
            return Ok(false);
        };
        debug_assert_eq!(order.len(), self.len);
        if edits.len != 0 {
            let mut out = Overrides::default();
            for (new, &old) in order.iter().enumerate() {
                if let Some(row) = edits.get(old, self.layout.length) {
                    out.put(new, row)?;
                }
            }
            *edits = out;
            self.boxed.take();
        }
        Ok(true)
    }

    pub(super) fn swap(&mut self, a: usize, b: usize) {
        let RowStorage::Packed(words) = &mut self.storage else {
            unreachable!()
        };
        self.boxed.take();
        let width = self.layout.length;
        for i in 0..width {
            words.swap(a * width + i, b * width + i);
        }
    }

    pub(super) fn row(&self, at: usize) -> &[usize] {
        debug_assert!(at < self.len);
        let width = self.layout.length;
        match &self.storage {
            RowStorage::Packed(words) => &words[at * width..(at + 1) * width],
            RowStorage::Repeated(base, edits) => edits
                .get(at, width)
                .unwrap_or_else(|| base_row(base, width)),
        }
    }

    pub(super) fn owned_row(&self, at: usize) -> Result<OwnedWord> {
        debug_assert!(at < self.len);
        if let RowStorage::Repeated(base, edits) = &self.storage
            && edits.get(at, self.layout.length).is_none()
        {
            return Ok(base.clone());
        }
        // SAFETY: the new allocation owns room for the exact initialized scalar row.
        unsafe {
            let record = allocate_record(self.layout);
            if record.is_null() {
                return Err(Error::allocation());
            }
            record.copy_from_nonoverlapping(self.row(at).as_ptr(), self.layout.length);
            Ok(OwnedWord::from_owned(record as usize, WordKind::new(6)))
        }
    }

    pub(super) fn get(&self, at: usize) -> Result<usize> {
        if at >= self.len {
            return Err(Error::index());
        }
        Ok(self.owned_row(at)?.into_word())
    }

    pub(super) fn boxed(&self) -> &Words {
        self.boxed.get_or_init(|| {
            self.try_boxed_words()
                .unwrap_or_else(|_| super::fail("MemoryError", "struct view allocation failed"))
        })
    }

    pub(super) fn try_boxed_words(&self) -> Result<Words> {
        let mut out = Words::new(6);
        out.values
            .try_reserve_exact(self.len)
            .map_err(|_| Error::allocation())?;
        for at in 0..self.len {
            out.values.push(self.get(at)?);
        }
        out.update_header();
        Ok(out)
    }

    pub(super) fn take_boxed(&mut self) -> Words {
        self.boxed();
        self.boxed.take().unwrap()
    }

    /// Preserve the same representable size limit in both layouts, without
    /// reserving the hypothetical dense buffer just to repeat a single value.
    fn word_count(&self, len: usize) -> Result<usize> {
        len.checked_mul(self.layout.length)
            .filter(|&words| words <= MAX_LIST_LEN)
            .ok_or_else(Error::allocation)
    }

    /// Reserve before replacing shared storage, so a failed write leaves the list intact.
    fn writable(&mut self, len: usize) -> Result<&mut Vec<usize>> {
        let target = self.word_count(len)?;
        if let RowStorage::Repeated(base, edits) = &self.storage {
            let width = self.layout.length;
            let mut words = Vec::new();
            words
                .try_reserve_exact(target)
                .map_err(|_| Error::allocation())?;
            words.extend_from_slice(base_row(base, width));
            tile(&mut words, self.len * width);
            for (at, row) in edits.iter(width) {
                words[at * width..(at + 1) * width].copy_from_slice(row);
            }
            self.storage = RowStorage::Packed(words);
        }
        let RowStorage::Packed(words) = &mut self.storage else {
            unreachable!()
        };
        words
            .try_reserve(target - words.len())
            .map_err(|_| Error::allocation())?;
        Ok(words)
    }

    /// Commit an exact list of rows after every required allocation succeeds.
    pub(super) fn extend<'a>(
        &mut self,
        rows: impl ExactSizeIterator<Item = &'a [usize]>,
    ) -> Result<()> {
        let len = self
            .len
            .checked_add(rows.len())
            .ok_or_else(Error::allocation)?;
        if rows.len() == 0 {
            return Ok(());
        }
        if let RowStorage::Repeated(base, edits) = &self.storage {
            let mut result = self.with_default(base.clone(), self.len);
            for (at, row) in edits.iter(self.layout.length) {
                result.set(at, row)?;
            }
            for row in rows {
                result.append(row)?;
            }
            *self = result;
        } else {
            let words = self.writable(len)?;
            for row in rows {
                words.extend_from_slice(row);
            }
            self.len = len;
            self.boxed.take();
        }
        Ok(())
    }

    pub(super) fn append(&mut self, row: &[usize]) -> Result<()> {
        debug_assert_eq!(row.len(), self.layout.length);
        if let RowStorage::Packed(words) = &mut self.storage {
            // Vec checks the dense byte-size limit as part of this reservation.
            words
                .try_reserve(row.len())
                .map_err(|_| Error::allocation())?;
            words.extend_from_slice(row);
        } else {
            self.write_shared(self.len, self.len + 1, row)?;
        }
        self.len += 1;
        self.boxed.take();
        Ok(())
    }

    pub(super) fn set(&mut self, at: usize, row: &[usize]) -> Result<()> {
        if at >= self.len {
            return Err(Error::index());
        }
        let width = self.layout.length;
        if let RowStorage::Packed(words) = &mut self.storage {
            words[at * width..(at + 1) * width].copy_from_slice(row);
        } else {
            self.write_shared(at, self.len, row)?;
        }
        self.boxed.take();
        Ok(())
    }

    fn write_shared(&mut self, at: usize, len: usize, row: &[usize]) -> Result<()> {
        self.word_count(len)?;
        let width = self.layout.length;
        let RowStorage::Repeated(base, edits) = &mut self.storage else {
            unreachable!()
        };
        // Equality of public values deliberately ignores signed zero and NaN bits.
        // Storage must preserve them: fields can expose the original float bits.
        if row == base_row(base, width) {
            edits.remove(at, width);
        } else if !edits.should_pack_after_insert(at, len) || edits.get(at, width).is_some() {
            edits.put(at, row)?;
        } else {
            let append = at == self.len;
            let Ok(words) = self.writable(len) else {
                // Packing is a performance choice. If its reservation fails,
                // an independently affordable sparse write can still succeed.
                let RowStorage::Repeated(_, edits) = &mut self.storage else {
                    unreachable!()
                };
                return edits.put(at, row);
            };
            if append {
                words.extend_from_slice(row);
            } else {
                words[at * width..(at + 1) * width].copy_from_slice(row);
            }
        }
        Ok(())
    }

    pub(super) fn pop(&mut self) -> Result<usize> {
        let last = self
            .len
            .checked_sub(1)
            .ok_or_else(|| Error::new(super::Kind::IndexError, "pop from empty list"))?;
        let result = self.owned_row(last)?;
        self.len = last;
        if let RowStorage::Packed(words) = &mut self.storage {
            words.truncate(last * self.layout.length);
        } else if last == 0 {
            self.storage = RowStorage::Packed(Vec::new());
        } else if let RowStorage::Repeated(_, edits) = &mut self.storage {
            edits.remove(last, self.layout.length);
        }
        self.boxed.take();
        Ok(result.into_word())
    }

    pub(super) fn clear(&mut self) {
        *self = Self::new(self.layout);
    }

    fn with_default(&self, base: OwnedWord, len: usize) -> Self {
        Self {
            layout: self.layout,
            len,
            storage: RowStorage::Repeated(base, Overrides::default()),
            boxed: OnceCell::new(),
        }
    }

    pub(super) fn slice(&self, indices: SliceIndices) -> Result<Words> {
        let len = indices.len();
        let mut out = Self::new(self.layout);
        if len != 0 {
            if let RowStorage::Repeated(base, edits) = &self.storage {
                out = self.with_default(base.clone(), len);
                // A sparse slice takes work proportional to the smaller side.
                if len < edits.len {
                    for (dest, source) in indices.enumerate() {
                        if let Some(row) = edits.get(source, self.layout.length) {
                            out.set(dest, row)?;
                        }
                    }
                } else {
                    for (source, row) in edits.iter(self.layout.length) {
                        if let Some(dest) = indices.position_of(source) {
                            out.set(dest, row)?;
                        }
                    }
                }
            } else {
                let words = out.writable(len)?;
                for i in indices {
                    words.extend_from_slice(self.row(i));
                }
                out.len = len;
            }
        }
        Words::try_from_rows(out)
    }

    pub(super) fn repeat(&self, count: usize) -> Result<Words> {
        let len = self.len.checked_mul(count).ok_or_else(Error::allocation)?;
        let target = self.word_count(len)?;
        let mut out = Self::new(self.layout);
        if len != 0 {
            if self.len == 1 {
                out = self.with_default(self.owned_row(0)?, len);
            } else if let RowStorage::Repeated(base, edits) = &self.storage {
                out = self.with_default(base.clone(), len);
                for (at, row) in edits.iter(self.layout.length) {
                    for copy in 0..count {
                        out.set(copy * self.len + at, row)?;
                    }
                }
            } else if let RowStorage::Packed(source) = &self.storage {
                let words = out.writable(len)?;
                words.extend_from_slice(source);
                tile(words, target);
                out.len = len;
            }
        }
        Words::try_from_rows(out)
    }

    /// # Safety
    /// The source is an initialized, independently rooted payload of the exact layout.
    pub(super) unsafe fn payload<'a>(&self, ptr: *const usize) -> &'a [usize] {
        // SAFETY: the caller roots the entire exact scalar payload through its use.
        unsafe { std::slice::from_raw_parts(ptr, self.layout.length) }
    }
}

/// # Safety
/// The descriptor is the compiler's static, nonempty scalar struct layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_list_new(layout: *const RecordDescriptor) -> *const Object {
    // SAFETY: generated descriptors have program lifetime.
    crate::ObjectOwner::into_raw(Words::from_rows(Rows::new(unsafe { &*layout })).into_object())
}

/// # Safety
/// The rooted list has this exact element layout. The input is initialized,
/// independently rooted scalar storage; no pointer into the list is accepted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_append(
    list: *const Object,
    layout: *const RecordDescriptor,
    row: *const usize,
) -> i32 {
    // SAFETY: neither copying scalar words nor moving list storage calls user code.
    let res = unsafe {
        object(list).0.with_mut(|s| {
            let Storage::Sequence(words) = s else {
                unreachable!()
            };
            if words.rows().is_none() {
                // A generic empty list (including sort's detached destination)
                // first learns its layout on append.
                words.pack_structs()?;
                if words.rows().is_none() {
                    debug_assert!(words.values.is_empty());
                    *words = Words::from_rows(Rows::new(&*layout));
                }
            }
            let rows = words.rows_mut().unwrap();
            rows.append(rows.payload(row))?;
            words.update_header();
            Ok(())
        })
    };
    super::finish(res.map(|()| 1), 0)
}

/// # Safety
/// A proven read-only loop keeps the list and storage unchanged. index_mask
/// accepts one native word: zero selects row zero for repetitions; all ones
/// preserves the index for packed rows. Null requires an indexed read (sparse or boxed).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_read_view(
    list: *const Object,
    index_mask: *mut usize,
) -> *const usize {
    // SAFETY: generated code roots the list and supplies writable, disjoint mask storage.
    unsafe {
        object(list).0.with(|s| {
            let rows = s.sequence_storage().rows();
            let (data, mask) = rows
                .and_then(Rows::direct_view)
                .unwrap_or((std::ptr::null(), 0));
            *index_mask = mask;
            data
        })
    }
}

/// Private ABI shared with the compiler's imre_struct_cursor_row. A zero
/// index mask broadcasts one immutable value; all ones expose consecutive rows.
#[repr(C)]
pub struct ReadSpan {
    start: usize,
    len: usize,
    index_mask: usize,
    data: *const usize,
}

/// # Safety
/// The list stays unchanged throughout the proven read-only loop, `at` is in
/// bounds, and `out` points to the compiler's private four-word read cursor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_seek_span(list: *const Object, at: usize, out: *mut ReadSpan) {
    // SAFETY: all payload pointers borrow scalar words from this rooted list.
    unsafe {
        object(list).0.with(|s| {
            let seq = s.sequence_storage();
            let span = if let Some(rows) = seq.rows() {
                match &rows.storage {
                    RowStorage::Repeated(base, edits) => {
                        edits.span(at, rows.len, base_row(base, rows.layout.length))
                    }
                    RowStorage::Packed(_) => ReadSpan {
                        start: at,
                        len: rows.len - at,
                        index_mask: usize::MAX,
                        data: rows.row(at).as_ptr(),
                    },
                }
            } else {
                ReadSpan {
                    start: at,
                    len: 1,
                    index_mask: 0,
                    data: seq.values[at] as *const usize,
                }
            };
            out.write(span);
        });
    }
}

/// # Safety
/// The exact list is rooted, the current native index is in bounds, and the
/// caller copies the payload before any mutation or release of the list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_row(list: *const Object, at: usize) -> *const usize {
    // SAFETY: the borrow ends before the caller's immediate nonreentrant read.
    unsafe {
        object(list).0.with(|s| {
            let words = s.sequence_storage();
            if let Some(rows) = words.rows() {
                rows.row(at).as_ptr()
            } else {
                words.values[at] as *const usize
            }
        })
    }
}

/// # Safety
/// Both the exact list and fully initialized, independently rooted value remain live.
pub(super) unsafe fn set_if_packed(
    value: *const Object,
    at: impl FnOnce(usize) -> Result<usize>,
    word: usize,
) -> bool {
    // SAFETY: the input payload is copied before invalidating any boxed view.
    let result = unsafe {
        object(value).0.with_mut(|s| {
            let Storage::Sequence(words) = s else {
                unreachable!()
            };
            let rows = words.rows_mut()?;
            Some(
                at(rows.len())
                    .and_then(|index| rows.set(index, rows.payload(word as *const usize))),
            )
        })
    };
    if let Some(result) = result {
        super::finish(result, ());
        true
    } else {
        false
    }
}

impl Rows {
    fn equal_row(&self, a: &[usize], b: &[usize]) -> bool {
        a.iter().zip(b).enumerate().all(|(i, (&x, &y))| {
            // SAFETY: immutable metadata covers each field in both scalar rows.
            x == y || unsafe { *self.layout.kinds.add(i) == 7 } && float_key(x) == float_key(y)
        })
    }

    /// A decoder can return an ordinary empty list before seeing a first record.
    /// Compare its slots in place without boxing every row of the other list.
    pub(super) fn equal_words(&self, other: &Words) -> bool {
        other.kind.raw() == 6
            && self.len == other.values.len()
            && other.values.iter().enumerate().all(|(at, &word)| {
                let ptr = word as *const usize;
                // SAFETY: kind 6 roots each record; check its layout before reading any fields.
                unsafe {
                    std::ptr::eq(self.layout, super::records::descriptor(ptr))
                        && self.equal_row(self.row(at), self.payload(ptr))
                }
            })
    }

    pub(super) fn equal(&self, other: &Self) -> bool {
        if !std::ptr::eq(self.layout, other.layout) || self.len != other.len {
            return false;
        }
        let width = self.layout.length;
        match (&self.storage, &other.storage) {
            (RowStorage::Packed(a), RowStorage::Packed(b)) => a
                .chunks_exact(width)
                .zip(b.chunks_exact(width))
                .all(|(x, y)| self.equal_row(x, y)),
            (RowStorage::Packed(words), _) => words
                .chunks_exact(width)
                .enumerate()
                .all(|(i, row)| self.equal_row(row, other.row(i))),
            (_, RowStorage::Packed(_)) => other.equal(self),
            (RowStorage::Repeated(a, left), RowStorage::Repeated(b, right)) => {
                let mut covered = left.len;
                for (i, row) in left.iter(width) {
                    if !self.equal_row(row, other.row(i)) {
                        return false;
                    }
                }
                for (i, row) in right.iter(width) {
                    if left.get(i, width).is_none() {
                        covered += 1;
                        if !self.equal_row(base_row(a, width), row) {
                            return false;
                        }
                    }
                }
                covered == self.len || self.equal_row(base_row(a, width), base_row(b, width))
            }
        }
    }

    fn contains(&self, needle: &[usize]) -> bool {
        let width = self.layout.length;
        match &self.storage {
            RowStorage::Packed(words) => words
                .chunks_exact(width)
                .any(|row| self.equal_row(row, needle)),
            RowStorage::Repeated(base, edits) => {
                (edits.len < self.len && self.equal_row(base_row(base, width), needle))
                    || edits
                        .iter(width)
                        .any(|(_, row)| self.equal_row(row, needle))
            }
        }
    }
}

/// # Safety
/// The needle and list have the same exact scalar struct type and are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_list_contains(
    needle: *const usize,
    list: *const Object,
) -> u32 {
    // SAFETY: this read-only operation never executes user code.
    unsafe {
        object(list).0.with(|s| {
            if let Some(rows) = s.sequence_storage().rows() {
                let needle = rows.payload(needle);
                u32::from(rows.contains(needle))
            } else {
                u32::from(s.sequence_storage().values.iter().any(|word| {
                    super::values::imre_record_equal(needle, *word as *const usize) != 0
                }))
            }
        })
    }
}

/// # Safety
/// The list and tagged Python index are rooted. Copy the returned row before
/// mutation or release. Null signals a pending bounds or index conversion error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_row_checked(list: *const Object, at: usize) -> *const usize {
    // SAFETY: only the raw pointer escapes this nonreentrant immediate read.
    let row = unsafe {
        let at = super::decode_index(&super::Int::borrow(at));
        at.and_then(|at| {
            object(list).0.with(|s| {
                let w = s.sequence_storage();
                if let Some(rows) = w.rows() {
                    Ok(rows.row(super::index(at, rows.len())?).as_ptr())
                } else {
                    let words = w;
                    Ok(words.values[super::index(at, words.values.len())?] as *const usize)
                }
            })
        })
    };
    super::finish(row, std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::records::RecordSemantics;

    // The tests need the field kinds and struct flag, but never format a record.
    const POINT: &RecordDescriptor = &RecordDescriptor {
        length: 2,
        kinds: [7, 1].as_ptr(),
        semantics: &RecordSemantics {
            kind: 0,
            flags: 8,
            name: std::ptr::null(),
            name_len: 0,
            qualified_name: std::ptr::null(),
            qualified_name_len: 0,
            fields: std::ptr::null(),
        },
    };

    fn ok<T>(result: Result<T>) -> T {
        result.unwrap_or_else(|_| panic!("unexpected struct list error"))
    }

    #[test]
    fn invalid_json_separators_leave_the_boxed_view_unbuilt() {
        use crate::objects::exceptions::{Kind, imre_error_take, imre_exception_matches};
        use crate::objects::json::{Schema, imre_json_dumps};

        let integer = Schema {
            kind: 1,
            word_kind: 2,
            element: std::ptr::null(),
            record: std::ptr::null(),
            field_count: 0,
            fields: std::ptr::null(),
        };
        let record = Schema {
            kind: 7,
            word_kind: 6,
            record: POINT,
            ..integer
        };
        let list = Schema {
            kind: 6,
            word_kind: 5,
            element: &record,
            ..integer
        };
        let mut seed = Rows::new(POINT);
        ok(seed.append(&[0, 0]));
        for (count, expected) in [(2, Kind::TypeError), (4096, Kind::ValueError)] {
            let rows = ok(seed.repeat(count)).into_object();
            // SAFETY: the scalar is tagged and the separator list, row layout and
            // schemas stay rooted throughout the call. No row is being encoded.
            unsafe {
                let result = imre_json_dumps(
                    crate::Int::small(17).unwrap().0,
                    &integer,
                    0,
                    0,
                    1,
                    1,
                    0,
                    crate::ObjectOwner::as_ptr(&rows) as usize,
                    &list,
                );
                assert!(result.is_null());
                let error = imre_error_take();
                assert!(!error.is_null());
                let error = crate::ObjectOwner::from_raw(error);
                assert_eq!(
                    imre_exception_matches(crate::ObjectOwner::as_ptr(&error), expected as u32),
                    1
                );
                rows.0
                    .with(|s| assert!(s.sequence_storage().rows().unwrap().boxed.get().is_none()));
            }
        }
    }

    #[test]
    fn mixed_equality_and_short_islice_leave_the_boxed_view_unbuilt() {
        use crate::objects::{itertools, mappings, values};

        let mut seed = Rows::new(POINT);
        ok(seed.append(&[0, 0]));
        let mut repeated = ok(seed.repeat(4096));
        ok(repeated.rows_mut().unwrap().set(0, &[2, 1]));
        let list = repeated.into_object();
        let empty = Words::new(6).into_object();
        let ptr = crate::ObjectOwner::as_ptr(&list);
        // SAFETY: the list and the ordinary empty sequence are rooted; mode 4
        // is a sequence object, and the stop is a tagged integer.
        let cursor = unsafe {
            assert_eq!(
                values::imre_sequence_equal(ptr, crate::ObjectOwner::as_ptr(&empty)),
                0
            );
            assert_eq!(
                values::imre_sequence_equal(crate::ObjectOwner::as_ptr(&empty), ptr),
                0
            );
            crate::ObjectOwner::from_raw(itertools::imre_islice(
                4,
                ptr as usize,
                0,
                crate::Int::small(3).unwrap().0,
                0,
            ))
        };
        let first = ok(mappings::pull(&cursor)).unwrap().0;
        // SAFETY: the first pull has finished and owns its result independently.
        unsafe {
            list.0.with_mut(|s| {
                let rows = s.sequence_storage_mut().rows_mut().unwrap();
                assert!(rows.boxed.get().is_none());
                ok(rows.set(1, &[4, 1]));
            });
        }
        let second = ok(mappings::pull(&cursor)).unwrap().0;
        let third = ok(mappings::pull(&cursor)).unwrap().0;
        assert!(ok(mappings::pull(&cursor)).is_none());
        // SAFETY: no cursor pull or storage borrow overlaps the clear.
        unsafe {
            list.0.with_mut(|s| {
                let rows = s.sequence_storage_mut().rows_mut().unwrap();
                assert!(rows.boxed.get().is_none());
                rows.clear();
            });
        }
        assert_eq!(base_row(&first, 2), [2, 1]);
        assert_eq!(base_row(&second, 2), [4, 1]);
        assert_eq!(base_row(&third, 2), [0, 0]);

        let mut ordinary = Words::new(6);
        ordinary.values.push(ok(seed.get(0)));
        assert!(seed.equal_words(&ordinary));
        ok(seed.set(0, &[2, 1]));
        assert!(!seed.equal_words(&ordinary));
    }

    #[test]
    fn scattered_slots_promote_earlier_than_clustered_slots() {
        let mut seed = Rows::new(POINT);
        ok(seed.append(&[0, 0]));
        let n = 4096;
        let mut clustered = ok(seed.repeat(n));
        let mut scattered = ok(seed.repeat(n));
        for i in 0..64 {
            ok(clustered.rows_mut().unwrap().set(i, &[1, 1]));
            ok(scattered.rows_mut().unwrap().set(i * 64, &[1, 1]));
        }
        // At exactly 1/64 neither promotes, even after touching every block.
        assert!(scattered.rows().unwrap().direct_view().is_none());
        ok(scattered.rows_mut().unwrap().set(0, &[2, 1]));
        assert!(scattered.rows().unwrap().direct_view().is_none());
        ok(scattered.rows_mut().unwrap().set(1, &[3, 1]));
        ok(clustered.rows_mut().unwrap().set(64, &[3, 1]));
        assert_eq!(
            scattered.rows().unwrap().direct_view().unwrap().1,
            usize::MAX
        );
        assert!(clustered.rows().unwrap().direct_view().is_none());
        for (at, expected) in [(0, [2, 1]), (1, [3, 1]), (2, [0, 0]), (4032, [1, 1])] {
            assert_eq!(scattered.rows().unwrap().row(at), expected);
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn sparse_edits_above_u32_survive_reversal_and_allocation_failure() {
        let mut seed = Rows::new(POINT);
        ok(seed.append(&[0, 0]));
        let n = (1usize << 32) + 400;
        let mut list = ok(seed.repeat(n));
        let rows = list.rows_mut().unwrap();
        for at in [n - 1, 63, 64, n - 66] {
            ok(rows.set(at, &[at, 1]));
        }
        assert!(rows.writable(MAX_LIST_LEN).is_err());
        assert_eq!(rows.row(n - 66), [n - 66, 1]);
        // SAFETY: zero omits the bounds, and -1 is a tagged small integer.
        let reverse = ok(unsafe { SliceIndices::new(n, 0, 0, crate::Int::small(-1).unwrap().0) });
        let copy = ok(rows.slice(reverse));
        let mirrored = copy.rows().unwrap();
        assert_eq!(mirrored.row(0), [n - 1, 1]);
        assert_eq!(mirrored.row(65), [n - 66, 1]);
        assert_eq!(mirrored.row(n - 65), [64, 1]);
        assert_eq!(mirrored.row(n - 1), [0, 0]);
        ok(rows.set(n - 1, &[0, 0]));
        assert_eq!(mirrored.row(0), [n - 1, 1]);
    }

    #[test]
    fn sparse_slots_restore_exact_float_bits_and_promote_only_on_a_new_exception() {
        let minus = [(-0.0_f64).to_bits() as usize, 1];
        let plus = [0, 1];
        let mut original = Rows::new(POINT);
        assert!(original.append(&minus).is_ok());
        let mut list = ok(original.repeat(4096));
        let rows = list.rows_mut().unwrap();
        let saved = ok(rows.owned_row(12));
        assert_eq!(rows.direct_view().unwrap().1, 0);
        assert!(rows.boxed().values.iter().all(|&word| word == saved.word));
        assert_eq!(ok(rows.owned_row(99)).word, saved.word);
        assert!(rows.repeat(usize::MAX).is_err());
        assert!(rows.append(&minus).is_ok());
        assert!(rows.set(1000, &minus).is_ok());
        assert_eq!(rows.direct_view().unwrap().1, 0);
        let copy = ok(rows.repeat(1));
        assert!(rows.set(4097, &plus).is_err());
        assert_eq!(rows.direct_view().unwrap().1, 0);
        assert!(rows.set(1000, &plus).is_ok());
        assert!(rows.direct_view().is_none());
        assert_eq!(rows.row(999), minus);
        assert_eq!(rows.row(1000), plus);
        assert_eq!(rows.row(1001), minus);
        assert_eq!(copy.rows().unwrap().row(1000), minus);
        assert!(rows.set(1000, &minus).is_ok());
        assert_eq!(rows.direct_view().unwrap().1, 0);
        let cutoff = rows.len / 4;
        for at in 0..cutoff {
            assert!(rows.set(at, &plus).is_ok());
        }
        assert!(rows.set(0, &plus).is_ok());
        assert!(rows.direct_view().is_none());
        assert!(rows.set(cutoff, &plus).is_ok());
        assert_eq!(rows.direct_view().unwrap().1, usize::MAX);
        assert_eq!(rows.row(cutoff), plus);
        assert_eq!(rows.row(cutoff + 1), minus);
        // The box captured before mutation remains independently rooted.
        // SAFETY: saved owns one initialized two-word POINT.
        assert_eq!(unsafe { *(saved.word as *const usize) }, minus[0]);
    }
}
