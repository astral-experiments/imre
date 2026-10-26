//! A copytree exception owns the actual list of failed entries. Python can
//! inspect or mutate that list through args, so render its current contents.
use std::cell::OnceCell;
use std::rc::Rc;

use super::{Error, Kind, Object, OwnedWord, Payload, WordKind, finish, object};
use crate::objects::words::Words;
use crate::objects::{records, values};

pub(super) struct Errors {
    entries: crate::ObjectOwner,
    args: OnceCell<OwnedWord>,
}

impl Error {
    pub(in crate::objects) fn tree_errors(entries: crate::ObjectOwner) -> Self {
        Self {
            kind: Kind::ShutilError,
            payload: Some(Payload::Tree(Rc::new(Errors {
                entries,
                args: OnceCell::new(),
            }))),
            cause: None,
        }
    }
}

impl Errors {
    /// Reuse the owned one-element tuple for both typed and generic exception views.
    pub(super) fn args(
        &self,
        layout: &'static records::RecordDescriptor,
    ) -> super::Result<OwnedWord> {
        // SAFETY: this immutable layout describes exactly the owned list in slot zero.
        unsafe {
            if self.args.get().is_none() {
                let tuple = records::allocate_record(layout);
                if tuple.is_null() {
                    return Err(Error::allocation());
                }
                tuple.write(
                    crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(&self.entries)) as usize,
                );
                let owned = OwnedWord::from_owned(tuple as usize, WordKind::new(6));
                assert!(self.args.set(owned).is_ok());
            }
            Ok(self.args.get().unwrap().clone())
        }
    }

    pub(super) fn message(&self) -> super::Rendered {
        let mut text = String::new();
        self.write_message(&mut values::ReprWriter(&mut text))?;
        super::rendered(text.into())
    }

    pub(super) fn write_message(&self, output: &mut values::ReprWriter<'_>) -> std::fmt::Result {
        // SAFETY: this payload roots the list and every tuple entry during representation.
        unsafe {
            values::try_write_unchecked_repr(
                output,
                crate::ObjectOwner::as_ptr(&self.entries) as usize,
                WordKind::new(5),
            )
        }
    }
}

/// Flatten the entries when a custom copy function calls another copytree.
pub(in crate::objects) fn append_tree_entries(
    error: &Object,
    output: &mut Words,
) -> super::Result<bool> {
    // SAFETY: neither immutable borrow invokes Python or changes the source buffers.
    unsafe {
        error.0.with(|s| {
            let Payload::Tree(errors) = &s.exception().payload else {
                return Ok(false);
            };
            errors.entries.0.with(|storage| {
                let entries = storage.sequence()?;
                output
                    .values
                    .try_reserve(entries.values.len())
                    .map_err(|_| Error::allocation())?;
                for &entry in &entries.values {
                    output.values.push(entries.kind.acquire(entry).into_word());
                }
                Ok(true)
            })
        })
    }
}

/// Return the same one-element args tuple on each access.
///
/// # Safety
/// The caller roots the exception and supplies the static tuple layout for a list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_tree_error_args(
    error: *const Object,
    layout: *const records::RecordDescriptor,
) -> *const usize {
    // SAFETY: the retained tuple's one field is exactly the native list handle.
    let result = unsafe {
        object(error).0.with(|s| {
            let Payload::Tree(errors) = &s.exception().payload else {
                return Err(Error::new(
                    Kind::TypeError,
                    "exception does not have copytree error entries",
                ));
            };
            errors
                .args(&*layout)
                .map(|tuple| tuple.into_word() as *const usize)
        })
    };
    finish(result, std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::words::Words;

    #[test]
    fn projected_args_keep_the_same_tuple_and_original_list() {
        let kinds = [5];
        let layout = records::RecordDescriptor {
            length: 1,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let list = Words::new(6).into_object();
        let error = Error::tree_errors(crate::ObjectOwner::clone(&list)).into_object();
        // SAFETY: the layout outlives the exception and both returned tuple owners.
        unsafe {
            let first = imre_shutil_tree_error_args(crate::ObjectOwner::as_ptr(&error), &layout);
            let second = imre_shutil_tree_error_args(crate::ObjectOwner::as_ptr(&error), &layout);
            assert_eq!(first, second);
            drop(error);
            assert_eq!(*first, crate::ObjectOwner::as_ptr(&list) as usize);
            drop(OwnedWord::from_owned(first as usize, WordKind::new(6)));
            drop(OwnedWord::from_owned(second as usize, WordKind::new(6)));
        }
        assert_eq!(crate::ObjectOwner::strong_count(&list), 1);
    }
}
