//! Function links retain their targets just as an entry in a function's dict does.
use super::{Header, RecordDescriptor, descriptor, imre_record_release, imre_record_retain};
use crate::objects::exceptions::{Error, Kind};

const PYTHON_FUNCTION: u32 = 3;

pub(super) fn is_function(layout: &RecordDescriptor) -> bool {
    // SAFETY: record descriptors have static metadata or a null pointer.
    unsafe { layout.semantics.as_ref() }.is_some_and(|info| info.kind == PYTHON_FUNCTION)
}

/// # Safety
/// The caller roots a function whose third field is its optional owned link.
unsafe fn link(function: *const usize) -> *mut *const usize {
    // SAFETY: only generated Python function layouts reach this accessor.
    unsafe { function.add(2).cast::<*const usize>().cast_mut() }
}

/// # Safety
/// Both arguments borrow live Python functions with the same callable signature.
/// The caller takes one reference to the returned wrapper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_function_set_wrapped(
    wrapper: *const usize,
    wrapped: *const usize,
) -> *const usize {
    // SAFETY: retained before replacing the old owner, even on self-assignment.
    unsafe {
        debug_assert!(is_function(descriptor(wrapper)) && is_function(descriptor(wrapped)));
        imre_record_retain(wrapped);
        let previous = link(wrapper).replace(wrapped);
        imre_record_release(previous);
        imre_record_retain(wrapper);
    }
    wrapper
}

/// # Safety
/// The caller roots the function and takes one reference to a successful result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_function_get_wrapped(wrapper: *const usize) -> *const usize {
    // SAFETY: a function's link is null or owns a live callable.
    unsafe {
        debug_assert!(is_function(descriptor(wrapper)));
        let wrapped = link(wrapper).read();
        if wrapped.is_null() {
            Error::new(
                Kind::AttributeError,
                "'function' object has no attribute '__wrapped__'",
            )
            .raise();
        } else {
            imre_record_retain(wrapped);
        }
        wrapped
    }
}

/// A closed ring with exactly one owner on each node is held only by its own
/// links. A tail entering a ring necessarily gives the entry two owners.
///
/// # Safety
/// The starting function has one remaining owner. All links stay live until
/// the final step, which transfers exactly one reference to the release queue.
pub(super) unsafe fn release_unowned_cycle(first: *const usize) {
    let mut function = first;
    loop {
        // SAFETY: the preceding rooted function owns this edge, and the
        // compiler admits only plain native functions to a linked signature.
        unsafe {
            let next = link(function).read();
            if next.is_null() {
                return;
            }
            if next == first {
                link(function).write(std::ptr::null());
                imre_record_release(first);
                return;
            }
            let header = &*next.cast::<Header>().sub(1);
            if header.references.get() != 1 || !is_function(header.descriptor()) {
                return;
            }
            function = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::records::{RecordSemantics, imre_record_new};
    use crate::{Str, StrOwner};

    #[test]
    fn a_live_alias_keeps_the_entire_ring_until_its_last_release() {
        let info = RecordSemantics {
            kind: PYTHON_FUNCTION,
            flags: 0,
            name: std::ptr::null(),
            name_len: 0,
            qualified_name: std::ptr::null(),
            qualified_name_len: 0,
            fields: std::ptr::null(),
        };
        let function_kinds = [0, 6, 6];
        let function = RecordDescriptor {
            length: 3,
            kinds: function_kinds.as_ptr(),
            semantics: &info,
        };
        let module_spec = RecordSemantics { kind: 2, ..info };
        assert!(!is_function(&RecordDescriptor {
            semantics: &module_spec,
            ..function
        }));
        let leaf_kinds = [3];
        let leaf = RecordDescriptor {
            length: 1,
            kinds: leaf_kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        for size in [1, 2, 12] {
            let text = StrOwner::new(Str::from("lifetime witness"));
            // SAFETY: the fixture owns every descriptor and initializes the
            // witness in the strong context slot before publishing a link.
            unsafe {
                let records: Vec<_> = (0..size).map(|_| imre_record_new(&function)).collect();
                let observer = crate::objects::records::weak::WeakRecord::new(records[0]);
                let witness = imre_record_new(&leaf);
                witness
                    .cast::<*const Str>()
                    .write(StrOwner::into_raw(text.clone()));
                records[0].add(1).cast::<*mut usize>().write(witness);
                for (index, &record) in records.iter().enumerate() {
                    imre_record_release(imre_function_set_wrapped(
                        record,
                        records[(index + 1) % size],
                    ));
                }
                let alias = imre_function_get_wrapped(records[size - 1]);
                assert_eq!(alias, records[0].cast_const());
                for record in records {
                    imre_record_release(record);
                }
                assert!(observer.is_live());
                assert_eq!(StrOwner::strong_count(&text), 2);
                imre_record_release(alias);
                assert!(observer.upgrade().is_none());
                assert_eq!(StrOwner::strong_count(&text), 1);
            }
        }
    }
}
