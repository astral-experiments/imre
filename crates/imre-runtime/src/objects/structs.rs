//! Contiguous scalar payloads for a homogeneous list of immutable value structs.
use super::exceptions::{Error, Result};
use super::records::{RecordDescriptor, allocate_record};
use super::sequences::SliceIndices;
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

pub(super) struct Rows {
    layout: &'static RecordDescriptor,
    data: Vec<usize>,
    // Some legacy generic consumers require rooted record words for an entire traversal.
    // Normal indexing, mutation, iteration and scans do not create this view.
    boxed: OnceCell<Words>,
}

impl Rows {
    pub(super) fn new(layout: &'static RecordDescriptor) -> Self {
        assert!(layout.is_value_struct() && layout.length != 0);
        Self {
            layout,
            data: Vec::new(),
            boxed: OnceCell::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.data.len() / self.layout.length
    }

    pub(super) fn layout(&self) -> &RecordDescriptor {
        self.layout
    }

    pub(super) fn swap(&mut self, a: usize, b: usize) {
        self.boxed.take();
        let width = self.layout.length;
        for i in 0..width {
            self.data.swap(a * width + i, b * width + i);
        }
    }

    pub(super) fn row(&self, at: usize) -> &[usize] {
        let start = at * self.layout.length;
        &self.data[start..start + self.layout.length]
    }

    fn box_row(&self, at: usize) -> Result<usize> {
        // SAFETY: the descriptor is static, all fields are plain scalars and the new record owns sufficient space.
        unsafe {
            let record = allocate_record(self.layout);
            if record.is_null() {
                return Err(Error::allocation());
            }
            record.copy_from_nonoverlapping(self.row(at).as_ptr(), self.layout.length);
            Ok(record as usize)
        }
    }

    pub(super) fn get(&self, at: usize) -> Result<usize> {
        if at >= self.len() {
            return Err(Error::index());
        }
        self.box_row(at)
    }

    pub(super) fn boxed(&self) -> &Words {
        self.boxed.get_or_init(|| {
            let mut out = Words::new(6);
            out.values.reserve(self.len());
            for at in 0..self.len() {
                out.values.push(self.box_row(at).unwrap_or_else(|_| {
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

    fn reserve(&mut self, count: usize) -> Result<()> {
        if self
            .len()
            .checked_add(count)
            .is_none_or(|n| n > MAX_LIST_LEN)
        {
            return Err(Error::allocation());
        }
        self.data
            .try_reserve(
                count
                    .checked_mul(self.layout.length)
                    .ok_or_else(Error::allocation)?,
            )
            .map_err(|_| Error::allocation())
    }

    pub(super) fn append(&mut self, row: &[usize]) -> Result<()> {
        debug_assert_eq!(row.len(), self.layout.length);
        self.reserve(1)?;
        self.data.extend_from_slice(row);
        self.boxed.take();
        Ok(())
    }

    pub(super) fn set(&mut self, at: usize, row: &[usize]) -> Result<()> {
        if at >= self.len() {
            return Err(Error::index());
        }
        let start = at * self.layout.length;
        self.data[start..start + self.layout.length].copy_from_slice(row);
        self.boxed.take();
        Ok(())
    }

    pub(super) fn pop(&mut self) -> Result<usize> {
        let at = self
            .len()
            .checked_sub(1)
            .ok_or_else(|| Error::new(super::Kind::IndexError, "pop from empty list"))?;
        let result = self.box_row(at)?;
        self.boxed.take();
        self.data.truncate(at * self.layout.length);
        Ok(result)
    }

    pub(super) fn clear(&mut self) {
        self.boxed.take();
        self.data = Vec::new();
    }

    pub(super) fn slice(&self, indices: SliceIndices) -> Result<Words> {
        let mut out = Self::new(self.layout);
        out.reserve(indices.len())?;
        for i in indices {
            out.data.extend_from_slice(self.row(i));
        }
        Ok(Words::from_rows(out))
    }

    pub(super) fn repeat(&self, count: usize) -> Result<Words> {
        let mut out = Self::new(self.layout);
        out.reserve(
            self.len()
                .checked_mul(count)
                .ok_or_else(Error::allocation)?,
        )?;
        if count != 0 && !self.data.is_empty() {
            // The checked reservation above covers every word. Double an initialized
            // prefix so even [one_point] * n takes only logarithmically many copies.
            let target = self.data.len() * count;
            out.data.extend_from_slice(&self.data);
            while out.data.len() < target {
                let copy = out.data.len().min(target - out.data.len());
                out.data.extend_from_within(..copy);
            }
        }
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
/// A proven read-only view keeps the list owner and storage unchanged through
/// its final use. Null denotes an ordinary record-pointer list; indices remain checked by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_struct_data_readonly(list: *const Object) -> *const usize {
    // SAFETY: only the raw buffer pointer escapes, with the lifetime stated above.
    unsafe {
        object(list).0.with(|s| {
            s.sequence_storage()
                .rows()
                .map_or(std::ptr::null(), |r| r.data.as_ptr())
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
        std::ptr::eq(self.layout, other.layout)
            && self.data.len() == other.data.len()
            && self
                .data
                .chunks_exact(self.layout.length)
                .zip(other.data.chunks_exact(self.layout.length))
                .all(|(a, b)| self.equal_row(a, b))
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
                    rows.data
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
