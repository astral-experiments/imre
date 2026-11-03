//! Append-only rows of integer pairs whose row and tuple identities never escape.
use super::{Object, Storage, StorageCell, object};
use crate::objects::exceptions::{Error, finish};
use crate::{Int, ObjectOwner, index, list_subscript_index};
// The last cell doubles as a row length while fewer than four cells are live.
// Real cells have one of their low two bits set. A larger multiple of four
// refers to an overflow row, so the common case uses exactly four words.
type Row = [usize; 4];

#[inline]
fn inline_len(last: usize) -> Option<usize> {
    if last & 3 != 0 {
        Some(4)
    } else if last < 16 {
        Some(last >> 2)
    } else {
        None
    }
}

#[derive(Default)]
pub(super) struct Table {
    rows: Vec<Row>,
    overflow: Vec<Vec<usize>>,
    // Two signed 32-bit tagged words fit in one cell. An even cell indexes
    // the full-width owning pair, including arbitrary-precision integers.
    wide: Vec<[Int; 2]>,
}

impl Table {
    fn values(&self, row: usize) -> &[usize] {
        let row = &self.rows[row];
        match inline_len(row[3]) {
            Some(len) => &row[..len],
            None => &self.overflow[(row[3] >> 2) - 4],
        }
    }

    fn push(
        &mut self,
        row: usize,
        left: &Int,
        right: &Int,
    ) -> crate::objects::exceptions::Result<()> {
        let (x, y) = (left.0, right.0);
        let compact =
            x & y & 1 != 0 && x as i32 as isize as usize == x && y as i32 as isize as usize == y;
        let cell = if compact {
            (x as u32 as usize) | ((y as u32 as usize) << 32)
        } else {
            self.wide.try_reserve(1).map_err(|_| Error::allocation())?;
            (self.wide.len() << 2) | 2
        };
        let row = &mut self.rows[row];
        match inline_len(row[3]) {
            Some(4) => {
                let mut cells = Vec::new();
                cells.try_reserve(8).map_err(|_| Error::allocation())?;
                self.overflow
                    .try_reserve(1)
                    .map_err(|_| Error::allocation())?;
                cells.extend_from_slice(row);
                cells.push(cell);
                row[3] = (self.overflow.len() + 4) << 2;
                self.overflow.push(cells);
            }
            Some(len) => {
                row[3] = (len + 1) << 2;
                row[len] = cell;
            }
            None => {
                let cells = &mut self.overflow[(row[3] >> 2) - 4];
                cells.try_reserve(1).map_err(|_| Error::allocation())?;
                cells.push(cell);
            }
        }
        if !compact {
            self.wide.push([left.clone(), right.clone()]);
        }
        Ok(())
    }
}

impl Storage {
    fn pair_table(&self) -> &Table {
        let Self::PairTable(table) = self else {
            unreachable!("expected pair table")
        };
        table
    }
    fn pair_table_mut(&mut self) -> &mut Table {
        let Self::PairTable(table) = self else {
            unreachable!("expected pair table")
        };
        table
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_pair_table_new() -> *const Object {
    let result = allocator_api2::boxed::Box::try_new(Table::default()).and_then(|table| {
        ObjectOwner::try_new(Object(StorageCell::new(Storage::PairTable(table))))
    });
    finish(
        result
            .map(ObjectOwner::into_raw)
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
}

/// # Safety
/// All table operations require a rooted compiler-created table. Integer inputs
/// remain rooted for the call. No storage borrow crosses a call into Python.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_add_row(table: *const Object) -> u32 {
    // SAFETY: the compiler roots the table for this nonreentrant append.
    let result = unsafe {
        object(table).0.with_mut(|storage| {
            let rows = &mut storage.pair_table_mut().rows;
            rows.try_reserve(1).map_err(|_| Error::allocation())?;
            rows.push(Row::default());
            Ok(1)
        })
    };
    finish(result, 0)
}

/// Try a whole unit-step range of empty-row appends. A refused reservation
/// leaves the table intact so the scalar loop can expose any partial progress.
///
/// # Safety
/// The caller roots an append-only pair table with no observable row identities.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_try_add_rows(table: *const Object, stop: usize) -> bool {
    // One speculative reservation is bounded; large or nonpositive Python
    // ranges keep the ordinary loop and its original allocation behavior.
    if stop & 1 == 0 || !(1..=1_048_576).contains(&(stop >> 1)) {
        return false;
    }
    let count = stop >> 1;
    // SAFETY: the checked loop performs no other operations or callbacks.
    unsafe {
        object(table).0.with_mut(|s| {
            let rows = &mut s.pair_table_mut().rows;
            if rows.try_reserve_exact(count).is_err() {
                return false;
            }
            rows.resize_with(rows.len() + count, Row::default);
            true
        })
    }
}

/// # Safety
/// The table is rooted, as is the arbitrary-precision Python index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_index(table: *const Object, at: usize) -> usize {
    // SAFETY: the table is rooted and its row count cannot change during this call.
    let len = unsafe {
        object(table)
            .0
            .with(|storage| storage.pair_table().rows.len())
    };
    if at & 1 != 0 {
        let limit = len << 1;
        if at < limit {
            return at;
        }
        // A carry means that this negative small integer lies within one table length.
        let (normalized, in_range) = at.overflowing_add(limit);
        if in_range {
            return normalized;
        }
    }
    // SAFETY: the caller also roots the index through normalization.
    unsafe { normalize_pair_table_index(at, len) }
}

/// Normalize heap integers and report out-of-range list subscripts.
#[inline(never)]
unsafe fn normalize_pair_table_index(at: usize, len: usize) -> usize {
    // SAFETY: the caller holds the Python integer alive.
    let value = unsafe { Int::borrow(at) };
    let result = list_subscript_index(&value)
        .and_then(|at| index(at, len))
        .map(|row| Int::from_usize(row).into_raw());
    finish(result, 0)
}

/// # Safety
/// The small row index has already been normalized against this same table.
/// Appending further rows preserves it while evaluating either integer field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_push(
    table: *const Object,
    row: usize,
    x: usize,
    y: usize,
) -> u32 {
    // SAFETY: inputs are rooted, and cloning a native int cannot reenter.
    let result = unsafe {
        object(table).0.with_mut(|s| {
            s.pair_table_mut()
                .push(row >> 1, &Int::borrow(x), &Int::borrow(y))?;
            Ok(1)
        })
    };
    finish(result, 0)
}

/// # Safety
/// The table is rooted during the read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_len(table: *const Object) -> usize {
    // SAFETY: no reference escapes this read.
    unsafe {
        object(table)
            .0
            .with(|s| Int::from_usize(s.pair_table().rows.len()).into_raw())
    }
}

/// # Safety
/// The normalized row index belongs to this rooted, append-only table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_row_len(table: *const Object, row: usize) -> usize {
    // SAFETY: rows can grow or move, so acquire the current row on each call.
    unsafe {
        object(table)
            .0
            .with(|s| Int::from_usize(s.pair_table().values(row >> 1).len()).into_raw())
    }
}

/// # Safety
/// The normalized row index belongs to this rooted table. The column is zero
/// or one, and the result transfers one owned native integer on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_field(
    table: *const Object,
    row: usize,
    at: usize,
    field: u32,
) -> usize {
    // SAFETY: only an independently owned integer escapes this read.
    let result = unsafe {
        object(table).0.with(|s| {
            let values = s.pair_table().values(row >> 1);
            let i = index(list_subscript_index(&Int::borrow(at))?, values.len())?;
            let cell = values[i];
            if cell & 1 != 0 {
                Ok((cell >> (32 * field)) as i32 as isize as usize)
            } else {
                Ok(s.pair_table().wide[cell >> 2][field as usize]
                    .clone()
                    .into_raw())
            }
        })
    };
    finish(result, 0)
}

#[repr(C)]
pub struct View {
    data: *const usize,
    tagged_len: usize,
}

/// # Safety
/// The caller provides a writable view slot. It roots the table and proves that
/// no row or table mutation can occur while reading the returned buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_pair_table_view(
    table: *const Object,
    at: usize,
    view: *mut View,
) -> usize {
    // SAFETY: the table will stay unchanged throughout use of the view. Publish
    // both fields only after validating the index, preserving the old binding on error.
    let result = unsafe {
        object(table).0.with(|s| {
            let table = s.pair_table();
            let i = index(list_subscript_index(&Int::borrow(at))?, table.rows.len())?;
            let row = table.values(i);
            view.write(View {
                data: row.as_ptr(),
                tagged_len: Int::from_usize(row.len()).into_raw(),
            });
            Ok(Int::from_usize(i).into_raw())
        })
    };
    finish(result, 0)
}
