//! The default display hook owns the last displayed object in the builtins namespace.
use super::words::{OwnedWord, WordKind};
use super::{Error, Kind, Object, boxed_values, exceptions, records};
use crate::StrOwner;
use std::cell::RefCell;

thread_local! {
    static LAST: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) };
    static LEN: RefCell<Option<OwnedWord>> = const { RefCell::new(None) };
}

// Unlike a concrete word kind, this argument already owns its runtime descriptor.
const VIEW: u32 = u32::MAX;

/// Obtain a retained snapshot of the namespace binding, independently of later writes.
#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_last() -> *const Object {
    LAST.with_borrow(|slot| slot.clone()).map_or_else(
        || {
            Error::new(
                Kind::AttributeError,
                "module 'builtins' has no attribute '_'",
            )
            .returning(std::ptr::null())
        },
        crate::ObjectOwner::into_raw,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_has_last() -> u32 {
    LAST.with_borrow(|slot| u32::from(slot.is_some()))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_del_last() {
    if LAST.take().is_none() {
        Error::new(Kind::AttributeError, "'module' object has no attribute '_'").raise();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_clear() {
    drop(LAST.take());
    drop(LEN.take());
}

/// The bracketed value stays rooted, including mutable containers whose length can change.
unsafe extern "C" fn original_len(_receiver: *const usize, argument: *const usize) -> usize {
    // SAFETY: the compiler emits [class, hash diagnostic, key, value] with concrete field types.
    unsafe {
        let class = &*argument.cast::<*const crate::Str>().read();
        let value_kind = WordKind::new(*records::descriptor(argument).kinds.add(3));
        let value = value_kind.read_record_field(argument.add(3));
        match class.as_bytes() {
            b"str" => crate::imre_str_len(value as *const crate::Str),
            b"bytes" => super::bytes::imre_bytes_len(value as *const super::Bytes),
            b"tuple" => {
                crate::Int::from_usize(records::descriptor(value as *const usize).length).into_raw()
            }
            b"list" if value_kind.base() == 4 => {
                crate::imre_list_len(value as *const crate::List)
            }
            b"list" => super::imre_seq_len(value as *const Object),
            b"dict" => super::mappings::imre_dict_len(value as *const Object),
            b"set" => super::mappings::imre_set_len(value as *const Object),
            _ => Error::new(
                Kind::TypeError,
                format!("object of type '{class}' has no len()"),
            )
            .returning(0),
        }
    }
}

/// Read a retained snapshot of the binding, creating the original callable on first use.
///
/// # Safety
/// The compiler supplies the permanent layout of its ordinary callable records.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_builtins_len(
    layout: *const records::RecordDescriptor,
) -> *const usize {
    LEN.with_borrow_mut(|slot| {
        if slot.is_none() {
            // SAFETY: a new ordinary callable has initialized null fields and a raw code slot.
            unsafe {
                let callable = records::imre_record_try_new(layout);
                if callable.is_null() {
                    return std::ptr::null();
                }
                callable.cast::<*const ()>().write(original_len as *const ());
                *slot = Some(OwnedWord::from_owned(callable as usize, WordKind::new(6)));
            }
        }
        slot.as_ref().unwrap().clone().into_word() as *const usize
    })
}

/// # Safety
/// The argument is a rooted native callable with the checked len signature.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_builtins_set_len(callable: *const usize) {
    // SAFETY: acquire before releasing the previous binding, including on self-assignment.
    let owner = unsafe { WordKind::new(6).acquire(callable as usize) };
    drop(LEN.replace(Some(owner)));
}

/// # Safety
/// A concrete argument has exactly its stated layout; a VIEW points to a live snapshot.
unsafe fn snapshot(kind: u32, word: usize) -> crate::ObjectOwner {
    if kind == VIEW {
        // SAFETY: the caller roots this snapshot until ownership has been acquired.
        unsafe { crate::ObjectOwner::acquire_word(word) }
    } else {
        // SAFETY: the compiler supplies a rooted word with this concrete representation.
        unsafe { crate::ObjectOwner::from_raw(boxed_values::imre_box_value(kind, word)) }
    }
}

/// # Safety
/// The compiler roots the borrowed word with the stated concrete kind or VIEW.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_builtins_set_last(kind: u32, word: usize) {
    // SAFETY: acquire the incoming owner before releasing the displaced value.
    let value = unsafe { snapshot(kind, word) };
    drop(LAST.replace(Some(value)));
}

/// Run the original sys display hook. A failed write leaves the recursion guard at None.
///
/// # Safety
/// The caller roots a supported display value through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_displayhook(kind: u32, word: usize) {
    // SAFETY: make an independent owner before touching the namespace binding.
    let view = unsafe { snapshot(kind, word) };
    // SAFETY: the view roots any concrete value or dynamic module member.
    if unsafe { exceptions::imre_system_exit_code_kind(crate::ObjectOwner::as_ptr(&view)) } == 0 {
        return;
    }
    // SAFETY: (0, 0) is the native representation of None.
    unsafe { imre_builtins_set_last(0, 0) };
    // SAFETY: repr returns an owned string or NULL with a pending Python error.
    let rendered =
        unsafe { exceptions::imre_exception_field_repr(crate::ObjectOwner::as_ptr(&view)) };
    if rendered.is_null() {
        return;
    }
    // SAFETY: the runtime returned one owned reference.
    let rendered = unsafe { StrOwner::from_raw(rendered) };
    if crate::sys::write_display_line(&rendered) {
        drop(LAST.replace(Some(view)));
    }
}
