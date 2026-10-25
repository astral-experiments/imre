//! Preserve original source functions by module, qualified name and first source line.
use super::records::{self, RecordDescriptor};
use super::words::{OwnedWord, WordKind, Words};
use super::{Int, Object, Str};
use crate::StrOwner;
use std::cell::RefCell;
use std::collections::HashMap;

// The first four words own module, qualname, name and an optional deprecation
// marker. The remaining words are untraced scalars private to this layout.
const LINE: usize = 4;
const SHARED: usize = 5;
struct AttributeLayout(RecordDescriptor);
// SAFETY: the descriptor and its kind array are immutable and static.
unsafe impl Sync for AttributeLayout {}
static KINDS: [u32; 6] = [3, 3, 3, 3, 0, 0];
static ATTRIBUTES: AttributeLayout = AttributeLayout(RecordDescriptor {
    length: KINDS.len(),
    kinds: KINDS.as_ptr(),
    semantics: std::ptr::null(),
});

type Groups = HashMap<String, HashMap<String, Vec<(usize, OwnedWord)>>>;
thread_local! {
    // Source ordinals are compiler-assigned and independent of native code folding.
    // These defaults contain strings and scalars only, never function owners.
    static SITES: RefCell<HashMap<u32, OwnedWord>> = RefCell::default();
    static REGISTRY: RefCell<Groups> = RefCell::default();
    static DUMMY: crate::ObjectOwner = {
        // SAFETY: this constructor transfers one strong reference.
        unsafe { crate::ObjectOwner::from_raw(super::inspect::imre_plain_object_new()) }
    };
}

unsafe fn fields(function: *const usize) -> *mut usize {
    // SAFETY: only compiler-proven unbound functions use this API. Their
    // adapter ignores its context and the ordinary callable owns that field.
    unsafe { function.add(1).cast::<*mut usize>().read() }
}

/// Attaches a shared default record to the fresh function. The first mutation
/// copies this record, so separately executed definitions retain distinct state.
///
/// # Safety
/// The callable owns a null context. Its module, qualname and short name are
/// rooted native strings, and the ordinal names a unique source function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_declare(
    function: *mut usize,
    id: u32,
    module: *const Str,
    qualname: *const Str,
    name: *const Str,
    line: u32,
) {
    SITES.with_borrow_mut(|sites| {
        let site = sites.entry(id).or_insert_with(|| {
            // SAFETY: the descriptor is static. The fresh record owns each
            // retained string and the absent marker remains zero initialized.
            unsafe {
                let attrs = records::imre_record_new(&ATTRIBUTES.0);
                for (i, value) in [module, qualname, name].into_iter().enumerate() {
                    StrOwner::increment_strong_count(value);
                    attrs.add(i).cast::<*const Str>().write(value);
                }
                attrs.add(LINE).write(line as usize);
                attrs.add(SHARED).write(1);
                OwnedWord::from_owned(attrs as usize, WordKind::new(6))
            }
        });
        // SAFETY: transfer the new owner into the empty callable context.
        unsafe {
            site.clone().initialize_record_field(function.add(1));
        }
    });
}

unsafe fn with_key<R>(function: *const usize, f: impl FnOnce(&str, &str, usize) -> R) -> R {
    // SAFETY: the live source function roots both required strings across f.
    unsafe {
        let attrs = fields(function);
        let get = |index: usize| (&*attrs.add(index).cast::<*const Str>().read()).as_str();
        f(get(0), get(1), *attrs.add(LINE))
    }
}

/// # Safety
/// The source function and its attributes stay rooted until the registry takes
/// its own reference. Replacing a source line preserves its insertion position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_overload(function: *const usize) {
    // SAFETY: the caller holds the function throughout registration.
    unsafe {
        with_key(function, |module, name, line| {
            REGISTRY.with_borrow_mut(|registry| {
                let group = registry
                    .entry(module.into())
                    .or_default()
                    .entry(name.into())
                    .or_default();
                let owner = WordKind::new(14).acquire(function as usize);
                if let Some((_, previous)) =
                    group.iter_mut().find(|(existing, _)| *existing == line)
                {
                    *previous = owner;
                } else {
                    group.push((line, owner));
                }
            })
        });
    }
}

/// CPython creates a name bucket before discovering that a builtin lacks __code__.
///
/// # Safety
/// Both strings are rooted native literals describing a proven builtin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_overload_builtin(module: *const Str, name: *const Str) {
    // SAFETY: both immutable strings remain live during the lookup.
    let (module, name) = unsafe { ((&*module).as_str(), (&*name).as_str()) };
    REGISTRY.with_borrow_mut(|registry| {
        registry
            .entry(module.into())
            .or_default()
            .entry(name.into())
            .or_default();
    });
}

fn snapshot(module: &str, name: &str) -> *const Object {
    REGISTRY.with_borrow(|registry| {
        let mut words = Words::new(14);
        if let Some(group) = registry.get(module).and_then(|names| names.get(name)) {
            words
                .values
                .extend(group.iter().map(|(_, owner)| owner.clone().into_word()));
        }
        crate::ObjectOwner::into_raw(words.into_object())
    })
}

/// # Safety
/// The source function and its current attributes remain live during lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_get_overloads(function: *const usize) -> *const Object {
    // SAFETY: the caller holds the function until the fresh result owns all elements.
    unsafe { with_key(function, |module, name, _| snapshot(module, name)) }
}

/// # Safety
/// The two arguments are rooted strings naming a builtin without a code object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_get_builtin_overloads(
    module: *const Str,
    name: *const Str,
) -> *const Object {
    // SAFETY: both literals remain rooted during lookup.
    unsafe { snapshot((&*module).as_str(), (&*name).as_str()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_typing_clear_overloads() {
    drop(REGISTRY.with_borrow_mut(std::mem::take));
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_typing_registry_len() -> usize {
    REGISTRY.with_borrow(|registry| Int::from_usize(registry.len()).into_raw())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_typing_overload_dummy() -> *const Object {
    crate::ObjectOwner::into_raw(DUMMY.with(crate::ObjectOwner::clone))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_typing_call_stub() -> u8 {
    super::exceptions::Error::new(super::exceptions::Kind::NotImplementedError,
        "You should not call an overloaded function. A series of @overload-decorated functions outside a stub module should always be followed by an implementation that is not @overload-ed.").raise();
    0
}

pub(super) unsafe fn write_repr(out: &mut String, function: usize) {
    use std::fmt::Write;
    // SAFETY: the enclosing container roots the admitted source function.
    unsafe {
        with_key(function as *const usize, |_, name, _| {
            write!(out, "<function {name} at {function:#x}>").unwrap()
        });
    }
}

/// # Safety
/// The source function owns its attributes; index 0..4 selects a text field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_function_attr(
    function: *const usize,
    index: u32,
) -> *const Str {
    assert!(index < 4);
    // SAFETY: the function roots its attribute record and strings.
    let value = unsafe { *fields(function).add(index as usize).cast::<*const Str>() };
    if value.is_null() {
        super::exceptions::Error::new(
            super::exceptions::Kind::AttributeError,
            "'function' object has no attribute '__deprecated__'",
        )
        .raise();
    } else {
        // SAFETY: transfer one additional retained reference to the caller.
        unsafe {
            StrOwner::increment_strong_count(value);
        }
    }
    value
}

/// # Safety
/// The caller roots both the string and the plain mutable function; the index
/// selects one of its four owned string fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_typing_set_function_attr(
    value: *const Str,
    function: *mut usize,
    index: u32,
) -> u8 {
    assert!(index < 4);
    // SAFETY: the callable owns its context. All attribute fields are either
    // strings or primitive scalars, with no user-defined finalizers.
    unsafe {
        let context = function.add(1).cast::<*mut usize>();
        let mut attrs = *context;
        if *attrs.add(SHARED) != 0 {
            let copy = records::imre_record_shallow_copy(attrs);
            if copy.is_null() {
                return 0;
            }
            records::imre_record_release(attrs);
            attrs = copy;
            *attrs.add(SHARED) = 0;
            *context = attrs;
        }
        StrOwner::increment_strong_count(value);
        let previous = attrs
            .add(index as usize)
            .cast::<*const Str>()
            .replace(value);
        if !previous.is_null() {
            StrOwner::decrement_strong_count(previous);
        }
    }
    0
}
