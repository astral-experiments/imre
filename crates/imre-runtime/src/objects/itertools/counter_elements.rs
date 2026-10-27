use super::*;

/// Counter.elements takes the items iterator immediately, then starts each
/// repetition only when it reaches that entry.
pub(in crate::objects) struct Elements {
    items: Option<crate::ObjectOwner>,
    repeat: Option<Repeat<OwnedWord>>,
}

impl Elements {
    pub(super) fn next(&mut self) -> Result<Option<OwnedWord>, Error> {
        if let Some(key) = self.repeat.as_mut().and_then(Repeat::next) {
            return Ok(Some(key));
        }
        self.repeat = None;
        // chain.from_iterable exhausts its outer iterator after an error
        // acquiring the next repetition, including a change in mapping size.
        let Some(items) = self.items.take() else {
            return Ok(None);
        };
        while let Some((key, count)) = mappings::pull(&items)? {
            let count = count.expect("a dictionary items iterator always supplies a value");
            // SAFETY: the compiler requires an integer-valued mapping and the
            // items iterator has returned an owned count.
            let value = unsafe { Int::borrow(count.word) };
            let times = ssize(&value)?;
            if times > 0 {
                self.repeat = Some(Repeat {
                    value: key.clone(),
                    remaining: Some((times - 1) as usize),
                });
                self.items = Some(items);
                return Ok(Some(key));
            }
        }
        Ok(None)
    }
}

/// # Safety
/// The caller roots a dictionary with integer values. Its item iterator keeps
/// the mapping alive after the method returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_counter_elements(value: *const Object) -> *const Object {
    // SAFETY: mode one creates and transfers an owned dictionary items cursor.
    let items = unsafe { crate::ObjectOwner::from_raw(mappings::imre_dict_iter(value, 1)) };
    crate::ObjectOwner::into_raw(adapter(Adapter::CounterElements(RefCell::new(Elements {
        items: Some(items),
        repeat: None,
    }))))
}
