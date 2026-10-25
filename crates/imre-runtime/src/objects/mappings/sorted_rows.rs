//! Prepare one row per distinct key of two to four sealed list-factory
//! dictionaries. The input dictionaries root every existing list throughout
//! this nonreentrant operation. Each result record owns its key and lists.

use super::*;
use crate::StrOwner;
use crate::objects::records::{RecordDescriptor, allocate_record};
use crate::objects::{Words, stdlib::shared_prefix};

struct Row<const N: usize> {
    key: StrOwner,
    // Zero means absent in the initial table. Existing list handles cannot be
    // invalidated by a sealed default insertion, even when input maps alias.
    values: [usize; N],
}

/// Returns null with a pending MemoryError if a row buffer or record cannot be allocated.
///
/// # Safety
/// Every source is a rooted exact defaultdict[str, list[T]], with two to four
/// sources in union order. The static descriptor contains only string or list
/// fields; each column byte names the key (0) or a source (1-based) of the
/// matching field kind. The compiler proves that the generated constructor
/// contains only these field assignments, in evaluation order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sorted_record_rows(
    sources: *const *const Object,
    count: usize,
    descriptor: *const RecordDescriptor,
    columns: *const u8,
) -> *const Object {
    // SAFETY: the compiler supplies the count and both static descriptor arrays.
    unsafe {
        let descriptor = &*descriptor;
        let fields = std::slice::from_raw_parts(columns, descriptor.length);
        let result = match count {
            2 => gather(&*sources.cast::<[_; 2]>(), descriptor, fields),
            3 => gather(&*sources.cast::<[_; 3]>(), descriptor, fields),
            4 => gather(&*sources.cast::<[_; 4]>(), descriptor, fields),
            _ => fail("SystemError", "unexpected sorted-row source count"),
        };
        result.unwrap_or_else(|| {
            crate::objects::exceptions::record_allocation_failed();
            std::ptr::null()
        })
    }
}

/// # Safety
/// The sources and field metadata obey `imre_sorted_record_rows`'s contract.
unsafe fn gather<const N: usize>(
    sources: &[*const Object; N],
    descriptor: &RecordDescriptor,
    fields: &[u8],
) -> Option<*const Object> {
    let mut rows = Vec::<Row<N>>::new();
    let mut index = HashTable::<(u64, usize)>::new();
    let mut kinds = [WordKind::new(0); N];
    for (column, kind) in kinds.iter_mut().enumerate() {
        // SAFETY: one rooted dictionary is borrowed at a time; no key comparison
        // calls user code, and later rows retain their own representative key.
        *kind = unsafe {
            object(sources[column]).0.with(|storage| {
                let mapping = storage.dictionary();
                let Tables::Str(table) = &mapping.tables else {
                    fail("SystemError", "expected a string-key dictionary");
                };
                let extra = table.index.len().saturating_sub(rows.len());
                rows.try_reserve(extra).ok()?;
                index.try_reserve(extra, |entry| entry.0).ok()?;
                for entry in table.entries.iter().flatten() {
                    if let Some(&(_, position)) = index.find(entry.hash, |&(hash, position)| {
                        hash == entry.hash && rows[position].key == entry.key
                    }) {
                        rows[position].values[column] = entry.value;
                    } else {
                        // Disjoint keys can exceed the initial per-table estimate.
                        rows.try_reserve(1).ok()?;
                        index.try_reserve(1, |entry| entry.0).ok()?;
                        let position = rows.len();
                        let mut values = [0; N];
                        values[column] = entry.value;
                        rows.push(Row {
                            key: entry.key.clone(),
                            values,
                        });
                        index.insert_unique(entry.hash, (entry.hash, position), |entry| entry.0);
                    }
                }
                Some(mapping.value_kind)
            })
        }?;
    }
    drop(index);
    let prefix = shared_prefix(rows.iter().map(|row| row.key.as_bytes()));
    // Equal keys already share a row, so stability cannot affect the order.
    rows.sort_unstable_by(|a, b| a.key.as_bytes()[prefix..].cmp(&b.key.as_bytes()[prefix..]));

    let mut result = Words::new(6);
    result.values.try_reserve(rows.len()).ok()?;
    for row in rows {
        // SAFETY: the generated descriptor lives for the whole process. Every
        // field receives one independently owned word of the exact checked kind.
        unsafe {
            let record = std::ptr::NonNull::new(allocate_record(descriptor))?.as_ptr();
            // The record starts with zeroed fields, so the result can drop
            // a partially initialized row if a default insertion fails.
            result.values.push(record as usize);
            for (field, &column) in fields.iter().enumerate() {
                let word = if column == 0 {
                    row.key.clone().into_word()
                } else {
                    let i = usize::from(column - 1);
                    let word = row.values[i];
                    if word == 0 {
                        // Do the actual subscription at its original position.
                        // Repeated fields and aliased maps see the same fresh list.
                        let list = imre_dict_get(sources[i], row.key.word());
                        if list == 0 {
                            return None;
                        }
                        list
                    } else {
                        kinds[i].acquire(word).into_word()
                    }
                };
                record.add(field).write(word);
            }
        }
    }
    Some(crate::ObjectOwner::into_raw(result.into_object()))
}
