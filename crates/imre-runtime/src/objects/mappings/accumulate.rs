//! In-place arithmetic on exact integer dictionary values.
use super::*;

/// Return the stored word for a rooted scalar key, keeping the borrow inside the update.
unsafe fn value_slot<K: ScalarKey>(
    table: &mut Table<K, usize>,
    word: usize,
) -> Result<Option<&mut usize>, Error> {
    // SAFETY: the native caller roots this key with the table's concrete kind.
    let key = unsafe { K::borrow(word) };
    let hash = key.try_hash().map_err(TableKeyError::into_error)?;
    let position = table
        .key_position(hash, &key)
        .map_err(TableKeyError::into_error)?;
    Ok(position.map(|index| &mut table.entries[index].as_mut().unwrap().value))
}

/// Add or subtract a rooted integer at an existing dictionary entry with one probe.
///
/// # Safety
/// The caller roots an integer-valued dictionary, its exact scalar key, and an
/// integer operand. Operation zero adds and operation one subtracts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_accumulate_int(
    value: *const Object,
    key: usize,
    right: usize,
    operation: u32,
) {
    // SAFETY: arithmetic on exact integers cannot reenter the dictionary.
    let result = unsafe {
        let right = Int::borrow(right);
        object(value)
            .0
            .with_mut(|storage| {
                let mapping = storage.dictionary_mut();
                let Some(slot) = table!(&mut mapping.tables, table => value_slot(table, key))?
                else {
                    if matches!(mapping.missing, Missing::Raise | Missing::NoFactory) {
                        return Err(Error::key(
                            WordKind::new(mapping.tables.key_kind()).acquire(key),
                        ));
                    }
                    return Ok(None);
                };
                let sum = crate::integer_binary(&Int::borrow(*slot), &right, operation)?.into_raw();
                let previous = Int(std::mem::replace(slot, sum));
                mapping.update_ascii_int(key, sum);
                Ok(Some(previous))
            })
            .and_then(|previous| {
                if previous.is_some() {
                    return Ok(());
                }
                // A missing Counter key reads as zero before the result is inserted.
                // Keep other native missing policies on their ordinary subscription path.
                let old = subscript(value, key)?.ok_or_else(|| missing_key(value, key))?;
                let sum = crate::integer_binary(&Int(old.into_word()), &right, operation)?;
                dict_set(value, key, sum.0)
            })
    };
    crate::finish(result, ());
}
