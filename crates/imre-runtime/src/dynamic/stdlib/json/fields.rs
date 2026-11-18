//! Ordered JSON object fields without a per-object key-position table.

use super::{Id, Result};

const LINEAR_LIMIT: usize = 8;

#[derive(Clone, Copy)]
struct Field {
    key: Id,
    value: Id,
    ordinal: usize,
}

enum Storage {
    Small(Vec<(Id, Id)>),
    Wide {
        rows: Vec<Field>,
        entries: Vec<(Id, Id)>,
    },
}

/// Primary object-field storage. Equal keys keep the first position and last value.
pub(super) struct ObjectFields {
    storage: Storage,
}

impl ObjectFields {
    /// Reserve both the working rows and final output before any key is allocated.
    pub(super) fn with_capacity(field_count: usize) -> Result<Self> {
        let mut entries = Vec::new();
        entries
            .try_reserve(field_count)
            .map_err(|_| crate::Error::allocation())?;
        let storage = if field_count <= LINEAR_LIMIT {
            Storage::Small(entries)
        } else {
            let mut rows = Vec::new();
            rows.try_reserve(field_count)
                .map_err(|_| crate::Error::allocation())?;
            Storage::Wide { rows, entries }
        };
        Ok(Self { storage })
    }

    /// Append a field after the complete object capacity has been reserved.
    /// The private caller must supply no more than the original `field_count` fields.
    pub(super) fn push(&mut self, key: Id, value: Id) {
        match &mut self.storage {
            Storage::Small(entries) => {
                if let Some(entry) = entries.iter_mut().find(|entry| entry.0 == key) {
                    entry.1 = value;
                } else {
                    debug_assert!(entries.len() < entries.capacity());
                    entries.push((key, value));
                }
            }
            Storage::Wide { rows, .. } => {
                debug_assert!(rows.len() < rows.capacity());
                rows.push(Field {
                    key,
                    value,
                    ordinal: rows.len(),
                });
            }
        }
    }

    /// Collapse equal primary rows in place, then restore first-insertion order.
    pub(super) fn into_entries(self) -> Vec<(Id, Id)> {
        match self.storage {
            Storage::Small(entries) => entries,
            Storage::Wide {
                mut rows,
                mut entries,
            } => {
                rows.sort_unstable_by_key(|row| (row.key, row.ordinal));
                let mut read = 0;
                let mut write = 0;
                while read < rows.len() {
                    let first = rows[read];
                    let mut end = read + 1;
                    while end < rows.len() && rows[end].key == first.key {
                        end += 1;
                    }
                    rows[write] = Field {
                        value: rows[end - 1].value,
                        ..first
                    };
                    write += 1;
                    read = end;
                }
                rows.truncate(write);
                rows.sort_unstable_by_key(|row| row.ordinal);
                debug_assert!(entries.capacity() >= rows.len());
                entries.extend(rows.into_iter().map(|row| (row.key, row.value)));
                entries
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ObjectFields;

    #[test]
    fn duplicate_fields_keep_first_position_and_last_value() {
        let values = [
            (9, 0),
            (2, 1),
            (9, 2),
            (4, 3),
            (1, 4),
            (2, 5),
            (8, 6),
            (4, 7),
            (5, 8),
            (9, 9),
        ];
        let mut wide = ObjectFields::with_capacity(values.len()).unwrap();
        for (key, value) in values {
            wide.push(key, value);
        }
        assert_eq!(
            wide.into_entries(),
            [(9, 9), (2, 5), (4, 7), (1, 4), (8, 6), (5, 8)]
        );

        let mut small = ObjectFields::with_capacity(3).unwrap();
        small.push(9, 0);
        small.push(2, 1);
        small.push(9, 2);
        assert_eq!(small.into_entries(), [(9, 2), (2, 1)]);
    }
}
