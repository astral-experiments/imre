//! Scalar struct lists: dense rows, or one immutable record for a repeated singleton.
use super::exceptions::{Error, Result};
use super::records::{RecordDescriptor, allocate_record};
use super::sequences::SliceIndices;
use super::words::{OwnedWord, WordKind};
use super::{MAX_LIST_LEN, Object, Storage, Words, object};
use std::cell::OnceCell;
use std::rc::Rc;

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
    // Every logical slot reads this one immutable, independently owned record.
    Repeated(OwnedWord),
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

    fn repeated(&self) -> bool {
        matches!(self.storage, RowStorage::Repeated(_))
    }

    /// Repeated lists expose only one physical row, regardless of logical length.
    fn words(&self) -> &[usize] {
        match &self.storage {
            RowStorage::Packed(words) => words,
            // SAFETY: the stored owner roots one initialized record of this layout.
            RowStorage::Repeated(word) => unsafe { self.payload(word.word as *const usize) },
        }
    }

    pub(super) fn swap(&mut self, a: usize, b: usize) {
        if let RowStorage::Packed(words) = &mut self.storage {
            self.boxed.take();
            let width = self.layout.length;
            for i in 0..width {
                words.swap(a * width + i, b * width + i);
            }
        }
    }

    pub(super) fn row(&self, at: usize) -> &[usize] {
        debug_assert!(at < self.len);
        let start = if self.repeated() {
            0
        } else {
            at * self.layout.length
        };
        &self.words()[start..start + self.layout.length]
    }

    fn owned_row(&self, at: usize) -> Result<OwnedWord> {
        if let RowStorage::Repeated(word) = &self.storage {
            return Ok(word.clone());
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
            let mut out = Words::new(6);
            out.values.reserve(self.len);
            for at in 0..self.len {
                out.values.push(self.get(at).unwrap_or_else(|_| {
                    super::fail("MemoryError", "struct view allocation failed")
                }));
            }
            out.update_header();
            out
        })
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
        if self.repeated() {
            let mut words = Vec::new();
            words
                .try_reserve_exact(target)
                .map_err(|_| Error::allocation())?;
            words.extend_from_slice(self.words());
            tile(&mut words, self.len * self.layout.length);
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

    pub(super) fn append(&mut self, row: &[usize]) -> Result<()> {
        debug_assert_eq!(row.len(), self.layout.length);
        if self.repeated() && self.words() == row {
            self.word_count(self.len + 1)?;
        } else {
            self.writable(self.len + 1)?.extend_from_slice(row);
        }
        self.len += 1;
        self.boxed.take();
        Ok(())
    }

    pub(super) fn set(&mut self, at: usize, row: &[usize]) -> Result<()> {
        if at >= self.len {
            return Err(Error::index());
        }
        // Equal structs may contain observably different signed zeros or NaN bits.
        if self.repeated() && self.words() == row {
            return Ok(());
        }
        let width = self.layout.length;
        self.writable(self.len)?[at * width..(at + 1) * width].copy_from_slice(row);
        self.boxed.take();
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
        }
        self.boxed.take();
        Ok(result.into_word())
    }

    pub(super) fn clear(&mut self) {
        *self = Self::new(self.layout);
    }

    fn single_value(&self, len: usize) -> Result<Words> {
        let mut out = Self::new(self.layout);
        if len != 0 {
            out.storage = RowStorage::Repeated(self.owned_row(0)?);
            out.len = len;
        }
        Ok(Words::from_rows(out))
    }

    pub(super) fn slice(&self, indices: SliceIndices) -> Result<Words> {
        let len = indices.len();
        if self.repeated() {
            return self.single_value(len);
        }
        let mut out = Self::new(self.layout);
        let words = out.writable(len)?;
        for i in indices {
            words.extend_from_slice(self.row(i));
        }
        out.len = len;
        Ok(Words::from_rows(out))
    }

    pub(super) fn repeat(&self, count: usize) -> Result<Words> {
        let len = self.len.checked_mul(count).ok_or_else(Error::allocation)?;
        let target = self.word_count(len)?;
        if len == 0 || self.len == 1 || self.repeated() {
            return self.single_value(len);
        }
        let mut out = Self::new(self.layout);
        let words = out.writable(len)?;
        words.extend_from_slice(self.words());
        tile(words, target);
        out.len = len;
        Ok(Words::from_rows(out))
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
    Rc::into_raw(Words::from_rows(Rows::new(unsafe { &*layout })).into_object())
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
                words.pack_structs();
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
/// preserves the index for packed rows. Null denotes ordinary record pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_read_view(
    list: *const Object,
    index_mask: *mut usize,
) -> *const usize {
    // SAFETY: generated code roots the list and supplies writable, disjoint mask storage.
    unsafe {
        object(list).0.with(|s| {
            let rows = s.sequence_storage().rows();
            *index_mask = if rows.is_some_and(Rows::repeated) {
                0
            } else {
                usize::MAX
            };
            rows.map_or(std::ptr::null(), |r| r.words().as_ptr())
        })
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
                words.resolved().values[at] as *const usize
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

    pub(super) fn equal(&self, other: &Self) -> bool {
        if !std::ptr::eq(self.layout, other.layout) || self.len != other.len {
            return false;
        }
        let width = self.layout.length;
        if self.repeated() {
            other
                .words()
                .chunks_exact(width)
                .all(|row| self.equal_row(self.words(), row))
        } else if other.repeated() {
            self.words()
                .chunks_exact(width)
                .all(|row| self.equal_row(row, other.words()))
        } else {
            self.words()
                .chunks_exact(width)
                .zip(other.words().chunks_exact(width))
                .all(|(a, b)| self.equal_row(a, b))
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
                u32::from(
                    rows.words()
                        .chunks_exact(rows.layout.length)
                        .any(|row| rows.equal_row(row, needle)),
                )
            } else {
                u32::from(s.sequence().values.iter().any(|word| {
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
                    let words = w.resolved();
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
    fn repeated_slots_share_one_record_until_a_bit_changes() {
        let minus = [(-0.0_f64).to_bits() as usize, 1];
        let plus = [0, 1];
        let mut original = Rows::new(POINT);
        assert!(original.append(&minus).is_ok());
        let mut list = ok(original.repeat(4096));
        let rows = list.rows_mut().unwrap();
        let saved = ok(rows.owned_row(12));
        assert_eq!(rows.words().len(), 2);
        assert!(rows.boxed().values.iter().all(|&word| word == saved.word));
        assert_eq!(ok(rows.owned_row(99)).word, saved.word);
        assert!(rows.repeat(usize::MAX).is_err());
        assert!(rows.append(&minus).is_ok());
        assert!(rows.set(1000, &minus).is_ok());
        assert!(rows.repeated());
        let copy = ok(rows.repeat(1));
        assert!(rows.set(4097, &plus).is_err());
        assert!(rows.repeated());
        assert!(rows.set(1000, &plus).is_ok());
        assert_eq!(rows.words().len(), 4097 * 2);
        assert_eq!(rows.row(999), minus);
        assert_eq!(rows.row(1000), plus);
        assert_eq!(rows.row(1001), minus);
        assert_eq!(copy.rows().unwrap().row(1000), minus);
        // The box captured before mutation remains independently rooted.
        // SAFETY: saved owns one initialized two-word POINT.
        assert_eq!(unsafe { *(saved.word as *const usize) }, minus[0]);
    }
}
