//! A target-native errno namespace and its independently mutable reverse dictionary.
use super::exceptions::{Error, Kind, imre_error_pending};
use super::mappings::{
    imre_dict_contains, imre_dict_new, imre_dict_set, imre_set_add, imre_set_new,
};
use super::{Int, Object, Storage, StorageCell, Str, object, values};
use std::cell::OnceCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int};
use std::rc::Rc;

#[repr(C)]
struct Entry {
    name: *const c_char,
    code: c_int,
}
unsafe extern "C" {
    fn imre_platform_errno_entries(length: *mut usize) -> *const Entry;
}

const DOC: &str = include_str!("../../native/errno-doc.txt");
const LOADER: &str = "<class '_frozen_importlib.BuiltinImporter'>";
const SPEC: &str = "ModuleSpec(name='errno', loader=<class '_frozen_importlib.BuiltinImporter'>, origin='built-in')";

pub(super) struct Registry {
    codes: HashMap<&'static str, i32>,
    keys: Rc<Object>,
    errorcode: Rc<Object>,
}

pub(super) struct Module {
    registry: Rc<Registry>,
    namespace: Rc<Object>,
}

#[derive(Clone)]
pub(super) enum Member {
    Number(i32),
    Text(Rc<Str>),
    Dictionary(Rc<Object>),
    Loader,
    Spec,
}

fn into_object(storage: Storage) -> Rc<Object> {
    Rc::new(Object(StorageCell::new(storage)))
}

fn build() -> Option<Rc<Object>> {
    let mut length = 0;
    // SAFETY: this C table and its NUL-terminated symbol names have static lifetime.
    let entries = unsafe {
        let ptr = imre_platform_errno_entries(&mut length);
        std::slice::from_raw_parts(ptr, length)
    };
    // SAFETY: these constructors return independent owners with the specified representations.
    let (errorcode, keys) = unsafe {
        (
            Rc::from_raw(imre_dict_new(2, 3)),
            Rc::from_raw(imre_set_new(3)),
        )
    };
    let mut codes = HashMap::new();
    if codes.try_reserve(entries.len()).is_err() {
        Error::allocation().raise();
        return None;
    }
    let add_key = |text: &Rc<Str>| {
        // SAFETY: the set and exact string remain rooted throughout insertion.
        unsafe { imre_set_add(Rc::as_ptr(&keys), Rc::as_ptr(text) as usize) };
        imre_error_pending() == 0
    };
    for name in [
        "__name__",
        "__doc__",
        "__package__",
        "__loader__",
        "__spec__",
        "errorcode",
    ] {
        if !add_key(&Rc::new(Str::from(name))) {
            return None;
        }
    }
    for entry in entries {
        // SAFETY: generated C stores literal ASCII names in the target's actual registration order.
        let name = unsafe { CStr::from_ptr(entry.name) }
            .to_str()
            .expect("ASCII errno symbol");
        let text = Rc::new(Str::from(name));
        if !add_key(&text) {
            return None;
        }
        let code = Int::small(i64::from(entry.code)).unwrap();
        // SAFETY: this is a dict[int,str], and both compiler-word representations are rooted.
        unsafe {
            imre_dict_set(Rc::as_ptr(&errorcode), code.0, Rc::as_ptr(&text) as usize);
        }
        if imre_error_pending() != 0 {
            return None;
        }
        codes.insert(name, entry.code);
    }
    let registry = Rc::new(Registry {
        codes,
        keys,
        errorcode,
    });
    let namespace = into_object(Storage::ModuleNamespace(registry.clone()));
    Some(into_object(Storage::NativeModule(Module {
        registry,
        namespace,
    })))
}

thread_local! {static ERRNO: OnceCell<Rc<Object>>=const {OnceCell::new()};}

/// Return the per-thread builtin module; a failing dictionary build never publishes partial exports.
#[unsafe(no_mangle)]
pub extern "C" fn imre_errno_module() -> *const Object {
    ERRNO.with(|cell| {
        if let Some(module) = cell.get() {
            return Rc::into_raw(module.clone());
        }
        let Some(module) = build() else {
            return std::ptr::null();
        };
        Rc::into_raw(cell.get_or_init(|| module).clone())
    })
}

fn with_registry<T>(owner: *const Object, f: impl FnOnce(&Registry) -> T) -> T {
    // SAFETY: callers root a checked module or namespace object.
    unsafe {
        object(owner).0.with(|s| match s {
            Storage::NativeModule(m) => f(&m.registry),
            Storage::ModuleNamespace(n) => f(n),
            _ => unreachable!("checked module namespace"),
        })
    }
}

impl Registry {
    fn lookup(&self, name: &str) -> Option<Member> {
        Some(match name {
            "__name__" => Member::Text(Rc::new(Str::from("errno"))),
            "__doc__" => Member::Text(Rc::new(Str::from(DOC))),
            "__package__" => Member::Text(Rc::new(Str::new(""))),
            "__loader__" => Member::Loader,
            "__spec__" => Member::Spec,
            "errorcode" => Member::Dictionary(self.errorcode.clone()),
            _ => Member::Number(*self.codes.get(name)?),
        })
    }
}

fn missing(name: &str) -> Error {
    Error::new(
        Kind::AttributeError,
        format!("module 'errno' has no attribute '{name}'"),
    )
}

/// # Safety
/// The ABI caller roots an errno module object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_namespace(owner: *const Object) -> *const Object {
    // SAFETY: this projection retains the module's canonical dictionary object.
    unsafe {
        object(owner).0.with(|s| {
            let Storage::NativeModule(m) = s else {
                unreachable!()
            };
            Rc::into_raw(m.namespace.clone())
        })
    }
}

/// # Safety
/// The caller roots the namespace object; its exported keys have stable insertion order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_keys(owner: *const Object) -> *const Object {
    with_registry(owner, |r| Rc::into_raw(r.keys.clone()))
}

/// # Safety
/// The caller roots the builtin module. All projections retain exactly the same mutable dict.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_errorcode(owner: *const Object) -> *const Object {
    with_registry(owner, |r| Rc::into_raw(r.errorcode.clone()))
}

/// # Safety
/// Root the target module and UTF-8 symbol name, and check pending state on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_number(owner: *const Object, name: *const Str) -> usize {
    // SAFETY: the compiler supplies an owned string through this lookup.
    let name = unsafe { &*name };
    with_registry(owner, |r| {
        r.codes.get(name.as_str()).map_or_else(
            || missing(name).returning(0),
            |&n| Int::small(i64::from(n)).unwrap().into_raw(),
        )
    })
}

/// # Safety
/// The caller roots a native module and one of its statically resolved text export names.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_text(owner: *const Object, name: *const Str) -> *const Str {
    // SAFETY: the compiler supplies an exact text metadata name.
    let name = unsafe { &*name };
    with_registry(owner, |r| {
        let Some(Member::Text(s)) = r.lookup(name) else {
            unreachable!()
        };
        Rc::into_raw(s)
    })
}

fn inherited(name: &str) -> bool {
    matches!(
        name,
        "__class__"
            | "__dict__"
            | "__new__"
            | "__repr__"
            | "__getattribute__"
            | "__init__"
            | "__dir__"
            | "__hash__"
            | "__str__"
            | "__setattr__"
            | "__delattr__"
            | "__lt__"
            | "__le__"
            | "__eq__"
            | "__ne__"
            | "__gt__"
            | "__ge__"
            | "__reduce_ex__"
            | "__reduce__"
            | "__getstate__"
            | "__subclasshook__"
            | "__init_subclass__"
            | "__format__"
            | "__sizeof__"
            | "__annotations__"
            | "__annotate__"
    )
}

/// # Safety
/// Both module and attribute name stay live through lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_has(owner: *const Object, name: *const Str) -> u32 {
    // SAFETY: the caller supplies a valid string.
    let name = unsafe { &*name };
    with_registry(owner, |r| {
        u32::from(inherited(name) || r.lookup(name).is_some())
    })
}

/// # Safety
/// Root the module/namespace and string. In mapping mode a miss raises KeyError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_lookup(
    owner: *const Object,
    name: *const Str,
    mapping: u32,
) -> *const Object {
    // SAFETY: the name is valid through construction of the result or error.
    let name = unsafe { &*name };
    with_registry(owner, |r| match r.lookup(name) {
        Some(member) => Rc::into_raw(into_object(Storage::ModuleMember(member))),
        None if mapping != 0 => Error::string_key(name.clone()).returning(std::ptr::null()),
        None => missing(name).returning(std::ptr::null()),
    })
}

fn with_member<T>(owner: *const Object, f: impl FnOnce(&Member) -> T) -> T {
    // SAFETY: all projections are rooted exact native module member objects.
    unsafe {
        object(owner).0.with(|s| {
            let Storage::ModuleMember(m) = s else {
                unreachable!()
            };
            f(m)
        })
    }
}

/// # Safety
/// Root an immutable module member view. Mutable dictionaries are formatted at observation time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_member_text(owner: *const Object, repr: u32) -> *const Str {
    with_member(owner, |m| match m {
        Member::Text(text) if repr == 0 => Rc::into_raw(text.clone()),
        Member::Text(text) => Rc::into_raw(Rc::new(super::exceptions::string_repr(text))),
        Member::Number(n) => Rc::into_raw(Rc::new(Str::from(n.to_string()))),
        // SAFETY: the actual dictionary remains rooted by the live attribute.
        Member::Dictionary(d) => unsafe { values::imre_value_repr(Rc::as_ptr(d) as usize, 5) },
        Member::Loader => Rc::into_raw(Rc::new(Str::from(LOADER))),
        Member::Spec => Rc::into_raw(Rc::new(Str::from(SPEC))),
    })
}

/// # Safety
/// The dictionary has integer keys; the member remains rooted through hashing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_member_contains(
    member: *const Object,
    mapping: *const Object,
) -> u32 {
    with_member(member, |m| match m {
        // SAFETY: the tagged integer and concrete integer-key dictionary are valid.
        Member::Number(n) => unsafe {
            imre_dict_contains(mapping, Int::small(i64::from(*n)).unwrap().0)
        },
        Member::Text(_) | Member::Loader => 0,
        Member::Dictionary(_) => {
            Error::new(Kind::TypeError, "unhashable type: 'dict'").returning(0)
        }
        Member::Spec => Error::new(Kind::TypeError, "unhashable type: 'ModuleSpec'").returning(0),
    })
}

/// # Safety
/// Root a dynamic module attribute and the exact integer, str, or second attribute.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_member_equal(
    left: *const Object,
    kind: u32,
    word: usize,
) -> u32 {
    let a = with_member(left, Clone::clone);
    let eq = match (kind, &a) {
        (2, Member::Number(n)) => unsafe { Int::borrow(word) }.as_small() == Some(i64::from(*n)),
        (3, Member::Text(sa)) => sa.as_ref() == unsafe { &*(word as *const Str) },
        (5, _) => with_member(word as *const Object, |b| match (&a, b) {
            (Member::Number(a), Member::Number(b)) => a == b,
            (Member::Text(a), Member::Text(b)) => a == b,
            (Member::Dictionary(a), Member::Dictionary(b)) => Rc::ptr_eq(a, b),
            (Member::Loader, Member::Loader) | (Member::Spec, Member::Spec) => true,
            _ => false,
        }),
        _ => false,
    };
    u32::from(eq)
}
