//! A target-native errno namespace and its independently mutable reverse dictionary.
use super::exceptions::{Error, Kind, imre_error_pending};
use super::mappings::{
    imre_dict_contains, imre_dict_new, imre_dict_set, imre_set_add, imre_set_new,
};
use super::words::{OwnedWord, WordKind};
use super::{Int, Object, Storage, StorageCell, Str, object, values};
use crate::StrOwner;
use std::cell::{Cell, OnceCell, RefCell};
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

pub(crate) const DOC: &str = include_str!("../../native/errno-doc.txt");
const LOADER: &str = "<class '_frozen_importlib.BuiltinImporter'>";
const SPEC: &str = "ModuleSpec(name='errno', loader=<class '_frozen_importlib.BuiltinImporter'>, origin='built-in')";

pub(super) struct Registry {
    ordered: RefCell<Vec<&'static str>>,
    annotations: OnceCell<crate::ObjectOwner>,
    annotate: Cell<bool>,
    codes: HashMap<&'static str, i32>,
    keys: crate::ObjectOwner,
    errorcode: crate::ObjectOwner,
}

pub(super) struct Module {
    registry: Rc<Registry>,
    namespace: crate::ObjectOwner,
}

#[derive(Clone)]
pub(super) enum Member {
    Scalar(OwnedWord),
    Number(i32),
    Text(StrOwner),
    Dictionary(crate::ObjectOwner),
    Loader,
    Spec,
    Class,
    Namespace(Rc<Registry>),
    Bound(&'static str, usize),
}

fn into_object(storage: Storage) -> crate::ObjectOwner {
    crate::ObjectOwner::new(Object(StorageCell::new(storage)))
}

/// The target's actual registration order decides the name retained for errno aliases.
pub(crate) fn platform_entries() -> impl ExactSizeIterator<Item = (&'static str, i32)> {
    let mut length = 0;
    // SAFETY: this C table and its NUL-terminated symbol names have static lifetime.
    let entries = unsafe {
        let ptr = imre_platform_errno_entries(&mut length);
        std::slice::from_raw_parts(ptr, length)
    };
    entries.iter().map(|entry| {
        // SAFETY: the generated table stores NUL-terminated ASCII string literals.
        let name = unsafe { CStr::from_ptr(entry.name) }
            .to_str()
            .expect("ASCII errno symbol");
        (name, entry.code)
    })
}

fn build() -> Option<crate::ObjectOwner> {
    let entries = platform_entries();
    // SAFETY: these constructors return independent owners with the specified representations.
    let (errorcode, keys) = unsafe {
        (
            crate::ObjectOwner::from_raw(imre_dict_new(2, 3)),
            crate::ObjectOwner::from_raw(imre_set_new(3)),
        )
    };
    let mut codes = HashMap::new();
    if codes.try_reserve(entries.len()).is_err() {
        Error::allocation().raise();
        return None;
    }
    let add_key = |text: &StrOwner| {
        // SAFETY: the set and exact string remain rooted throughout insertion.
        unsafe {
            imre_set_add(
                crate::ObjectOwner::as_ptr(&keys),
                StrOwner::as_ptr(text) as usize,
            )
        };
        imre_error_pending() == 0
    };
    let mut ordered = Vec::new();
    for name in [
        "__name__",
        "__doc__",
        "__package__",
        "__loader__",
        "__spec__",
        "errorcode",
    ] {
        ordered.push(name);
        if !add_key(&StrOwner::new(Str::from(name))) {
            return None;
        }
    }
    for (name, value) in entries {
        ordered.push(name);
        let text = StrOwner::new(Str::from(name));
        if !add_key(&text) {
            return None;
        }
        let code = Int::small(i64::from(value)).unwrap();
        // SAFETY: this is a dict[int,str], and both compiler-word representations are rooted.
        unsafe {
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&errorcode),
                code.0,
                StrOwner::as_ptr(&text) as usize,
            );
        }
        if imre_error_pending() != 0 {
            return None;
        }
        codes.insert(name, value);
    }
    let registry = Rc::new(Registry {
        ordered: RefCell::new(ordered),
        annotations: OnceCell::new(),
        annotate: Cell::new(false),
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

thread_local! {static ERRNO: OnceCell<crate::ObjectOwner>=const {OnceCell::new()};}

/// Return the per-thread builtin module; a failing dictionary build never publishes partial exports.
#[unsafe(no_mangle)]
pub extern "C" fn imre_errno_module() -> *const Object {
    ERRNO.with(|cell| {
        if let Some(module) = cell.get() {
            return crate::ObjectOwner::into_raw(module.clone());
        }
        let Some(module) = build() else {
            return std::ptr::null();
        };
        crate::ObjectOwner::into_raw(cell.get_or_init(|| module).clone())
    })
}

/// Match `dir(errno)` at display time, including lazily installed attributes.
pub(super) fn exported_names() -> Vec<&'static str> {
    let owner = imre_errno_module();
    if owner.is_null() {
        return Vec::new();
    }
    // SAFETY: module acquisition transferred one owner. Its registry is distinct
    // from the exception being formatted and cannot call user code here.
    let owner = unsafe { crate::ObjectOwner::from_raw(owner) };
    with_registry(crate::ObjectOwner::as_ptr(&owner), |registry| {
        registry.ordered.borrow().clone()
    })
}

fn with_registry<T>(owner: *const Object, f: impl FnOnce(&Rc<Registry>) -> T) -> T {
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
    fn append_export(&self, name: &'static str) {
        let text = StrOwner::new(Str::from(name));
        // SAFETY: insertion acquires this exact key while the canonical keyset is rooted.
        unsafe {
            imre_set_add(
                crate::ObjectOwner::as_ptr(&self.keys),
                StrOwner::as_ptr(&text) as usize,
            )
        };
        self.ordered.borrow_mut().push(name);
    }

    fn lookup(self: &Rc<Self>, name: &str, mapping: bool) -> Option<Member> {
        Some(match name {
            "__name__" => Member::Text(StrOwner::new(Str::from("errno"))),
            "__doc__" => Member::Text(StrOwner::new(Str::from(DOC))),
            "__package__" => Member::Text(StrOwner::new(Str::new(""))),
            "__dict__" if !mapping => Member::Namespace(self.clone()),
            "__class__" if !mapping => Member::Class,
            "__annotations__" if !mapping || self.annotations.get().is_some() => {
                let dict = self.annotations.get_or_init(|| {
                    self.append_export("__annotations__");
                    // SAFETY: a fresh empty annotations dictionary owns its storage.
                    unsafe { crate::ObjectOwner::from_raw(imre_dict_new(3, 3)) }
                });
                Member::Dictionary(dict.clone())
            }
            "__annotate__" if !mapping || self.annotate.get() => {
                if !self.annotate.replace(true) {
                    self.append_export("__annotate__");
                }
                // SAFETY: CPython installs None for a builtin's annotation function.
                Member::Scalar(unsafe { WordKind::new(0).acquire(0) })
            }
            _ if !mapping && bound(name).is_some() => {
                Member::Bound(bound(name).unwrap(), Rc::as_ptr(self) as usize)
            }
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
            crate::ObjectOwner::into_raw(m.namespace.clone())
        })
    }
}

/// # Safety
/// The caller roots the namespace object; its exported keys have stable insertion order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_keys(owner: *const Object) -> *const Object {
    with_registry(owner, |r| crate::ObjectOwner::into_raw(r.keys.clone()))
}

/// # Safety
/// The caller roots the builtin module. All projections retain exactly the same mutable dict.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_errorcode(owner: *const Object) -> *const Object {
    with_registry(owner, |r| crate::ObjectOwner::into_raw(r.errorcode.clone()))
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
        let Some(Member::Text(s)) = r.lookup(name, false) else {
            unreachable!()
        };
        StrOwner::into_raw(s)
    })
}

fn bound(name: &str) -> Option<&'static str> {
    super::OBJECT_METHODS
        .iter()
        .copied()
        .find(|method| *method == name)
}

/// # Safety
/// Both module and attribute name stay live through lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_has(owner: *const Object, name: *const Str) -> u32 {
    // SAFETY: the caller supplies a valid string.
    let name = unsafe { &*name };
    with_registry(owner, |r| u32::from(r.lookup(name, false).is_some()))
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
    with_registry(owner, |r| match r.lookup(name, mapping != 0) {
        Some(member) => crate::ObjectOwner::into_raw(into_object(Storage::ModuleMember(member))),
        None if mapping != 0 => Error::string_key(name.clone()).returning(std::ptr::null()),
        None => missing(name).returning(std::ptr::null()),
    })
}

/// # Safety
/// The caller roots the errno module, exact string, and already evaluated default.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_lookup_default(
    owner: *const Object,
    name: *const Str,
    default: *const Object,
) -> *const Object {
    // SAFETY: the name and the default remain rooted during lookup and acquisition.
    unsafe {
        with_registry(owner, |r| match r.lookup(&*name, false) {
            Some(member) => {
                crate::ObjectOwner::into_raw(into_object(Storage::ModuleMember(member)))
            }
            None => {
                crate::ObjectOwner::increment_strong_count(default);
                default
            }
        })
    }
}

impl Member {
    pub(super) fn scalar(&self) -> Option<(u32, usize)> {
        Some(match self {
            Self::Number(n) => (2, Int::small(i64::from(*n)).unwrap().0),
            Self::Text(s) => (3, StrOwner::as_ptr(s) as usize),
            Self::Scalar(s) => (s.kind.raw(), s.word),
            _ => return None,
        })
    }
}

/// Metadata objects preserve their Python identity and operations. Scalars from
/// module lookup use the same checked word path as other boxed Python values.
pub(super) fn with_nonscalar_member<T>(owner: &Object, f: impl FnOnce(&Member) -> T) -> Option<T> {
    // SAFETY: the caller roots a boxed value through this immutable observation.
    unsafe {
        owner.0.with(|s| match s {
            Storage::ModuleMember(m) if m.scalar().is_none() => Some(f(m)),
            _ => None,
        })
    }
}
impl Member {
    /// Compare two rooted dynamic module members at the top level or inside a tuple.
    pub(super) fn equal_member(&self, other: &Self) -> bool {
        if let (Some(a), Some(b)) = (self.scalar(), other.scalar()) {
            return scalar_equal(a, b);
        }
        match (self, other) {
            // SAFETY: both dictionary members remain rooted through value comparison.
            (Member::Dictionary(a), Member::Dictionary(b)) => unsafe {
                values::equal(
                    crate::ObjectOwner::as_ptr(a) as usize,
                    crate::ObjectOwner::as_ptr(b) as usize,
                    WordKind::new(5),
                )
            },
            (Member::Loader, Member::Loader)
            | (Member::Spec, Member::Spec)
            | (Member::Class, Member::Class) => true,
            (Member::Bound(a, x), Member::Bound(b, y)) => a == b && x == y,
            (Member::Namespace(a), Member::Namespace(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub(super) fn type_name(&self) -> &'static str {
        match self {
            Self::Dictionary(_) | Self::Namespace(_) => "dict",
            Self::Loader | Self::Class => "type",
            Self::Spec => "ModuleSpec",
            Self::Bound(
                "__new__" | "__init_subclass__" | "__subclasshook__" | "__dir__" | "__getstate__"
                | "__reduce__" | "__reduce_ex__" | "__format__" | "__sizeof__",
                _,
            ) => "builtin_function_or_method",
            Self::Bound(..) => "method-wrapper",
            _ => unreachable!("module metadata only"),
        }
    }

    pub(super) fn length(&self) -> usize {
        match self {
            // SAFETY: the member owns this exact dictionary.
            Self::Dictionary(d) => unsafe {
                super::mappings::imre_dict_len(crate::ObjectOwner::as_ptr(d))
            },
            Self::Namespace(r) => Int::from_usize(r.ordered.borrow().len()).into_raw(),
            _ => Error::new(
                Kind::TypeError,
                format!("object of type '{}' has no len()", self.type_name()),
            )
            .returning(0),
        }
    }

    pub(super) fn index(&self, key: &Int) -> *const Object {
        match self {
            Self::Dictionary(_) | Self::Namespace(_) => {
                if let Self::Dictionary(d) = self
                    // SAFETY: the member owns this concrete dictionary through the lookup.
                    && let Some(item) = unsafe { d.0.with(|s| s.dictionary().integer_word(key)) }
                {
                    return crate::ObjectOwner::into_raw(into_object(Storage::ObjectValue(item)));
                }
                // SAFETY: the caller owns the exact integer used for subscription.
                Error::key(unsafe { WordKind::new(2).acquire(key.0) }).returning(std::ptr::null())
            }
            _ => {
                let message = match self {
                    Self::Class => "type 'module' is not subscriptable".into(),
                    Self::Loader => "type 'BuiltinImporter' is not subscriptable".into(),
                    _ => format!("'{}' object is not subscriptable", self.type_name()),
                };
                Error::new(Kind::TypeError, message).returning(std::ptr::null())
            }
        }
    }
}

fn scalar_equal((left_kind, left): (u32, usize), (right_kind, right): (u32, usize)) -> bool {
    let normalize = |kind, word: usize| {
        if kind == 1 {
            (2, Int::small(word as i64).unwrap().0)
        } else {
            (kind, word)
        }
    };
    let (a, x) = normalize(left_kind, left);
    let (b, y) = normalize(right_kind, right);
    // SAFETY: both callers root the matched scalar descriptors.
    a == b && unsafe { values::equal(x, y, WordKind::new(a)) }
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

impl Member {
    pub(super) fn kind(&self) -> u32 {
        match self {
            Self::Dictionary(_) | Self::Namespace(_) => 5,
            _ => 15,
        }
    }

    pub(super) fn truth(&self) -> u32 {
        match self {
            // SAFETY: this opaque member owns the exact dictionary.
            Self::Dictionary(d) => u32::from(
                unsafe { super::mappings::imre_dict_len(crate::ObjectOwner::as_ptr(d)) }
                    != Int::small(0).unwrap().0,
            ),
            _ => 1,
        }
    }
}

impl Member {
    /// Render metadata at observation time; shared dictionaries and namespaces
    /// reflect mutations without manufacturing a second copy.
    pub(super) fn text(&self, repr: u32) -> *const Str {
        match self {
            Member::Scalar(s) if s.kind.raw() == 3 && repr == 0 => {
                // SAFETY: kind 3 is an owned exact string; acquire it for the result.
                unsafe { StrOwner::increment_strong_count(s.word as *const Str) };
                s.word as *const Str
            }
            // SAFETY: the owned scalar remains live throughout formatting.
            Member::Scalar(s) => unsafe { values::imre_value_repr(s.word, s.kind.raw()) },
            Member::Text(text) if repr == 0 => StrOwner::into_raw(text.clone()),
            Member::Text(text) => {
                StrOwner::into_raw(StrOwner::new(super::exceptions::string_repr(text)))
            }
            Member::Number(n) => StrOwner::into_raw(StrOwner::new(Str::from(n.to_string()))),
            // SAFETY: the actual dictionary remains rooted by the live attribute.
            Member::Dictionary(d) => unsafe {
                values::imre_value_repr(crate::ObjectOwner::as_ptr(d) as usize, 5)
            },
            Member::Loader => StrOwner::into_raw(StrOwner::new(Str::from(LOADER))),
            Member::Spec => StrOwner::into_raw(StrOwner::new(Str::from(SPEC))),
            Member::Class => StrOwner::into_raw(StrOwner::new(Str::from("<class 'module'>"))),
            Member::Bound(name, addr) => {
                let value = match *name {
                    "__new__" | "__init_subclass__" | "__subclasshook__" => {
                        format!("<built-in method {name} of type object at 0x{addr:x}>")
                    }
                    "__dir__" | "__getstate__" | "__reduce__" | "__reduce_ex__" | "__format__"
                    | "__sizeof__" => format!("<built-in function {name}>"),
                    _ => format!("<method-wrapper '{name}' of module object at 0x{addr:x}>"),
                };
                StrOwner::into_raw(StrOwner::new(Str::from(value)))
            }
            Member::Namespace(r) => {
                let mut result = String::from("{");
                for (i, &name) in r.ordered.borrow().iter().enumerate() {
                    if i != 0 {
                        result.push_str(", ");
                    }
                    result.push_str(&super::exceptions::string_repr(name));
                    result.push_str(": ");
                    // SAFETY: each member's renderer transfers a fresh owning reference.
                    let value =
                        unsafe { StrOwner::from_raw(r.lookup(name, true).unwrap().text(1)) };
                    result.push_str(&value);
                }
                result.push('}');
                StrOwner::into_raw(StrOwner::new(Str::from(result)))
            }
        }
    }
}

/// # Safety
/// The dictionary has integer keys; the member remains rooted through hashing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_errno_member_contains(
    member: *const Object,
    mapping: *const Object,
) -> u32 {
    // SAFETY: the caller roots the boxed member through the dictionary probe.
    let rooted = unsafe { object(member) };
    if let Some(result) = with_nonscalar_member(rooted, |m| {
        match m {
        Member::Dictionary(_) | Member::Namespace(_) => {
            Error::new(Kind::TypeError, "cannot use 'dict' as a dict key (unhashable type: 'dict')").returning(0)
        }
        Member::Spec => Error::new(Kind::TypeError, "cannot use '_frozen_importlib.ModuleSpec' as a dict key (unhashable type: 'ModuleSpec')").returning(0),
        _ => 0,
    }
    }) {
        return result;
    }
    // SAFETY: the member roots a scalar or immutable tuple. Only bool and int
    // can equal a concrete integer key in this closed dictionary.
    let (kind, word) = unsafe { super::exceptions::exit_word(member) };
    let key = match kind.raw() {
        1 => Int::small(word as i64).unwrap().0,
        2 => word,
        _ => return 0,
    };
    // SAFETY: the compiler supplied a live integer-key dictionary.
    unsafe { imre_dict_contains(mapping, key) }
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
    let eq = if kind == 16 {
        with_member(word as *const Object, |b| a.equal_member(b))
    } else if kind == 5 {
        // SAFETY: the compiler supplies a live dict[int,str], and the member owns its dictionary.
        matches!(&a, Member::Dictionary(d) if unsafe { values::equal(crate::ObjectOwner::as_ptr(d) as usize,word,WordKind::new(5)) })
    } else {
        a.scalar().is_some_and(|a| scalar_equal(a, (kind, word)))
    };
    u32::from(eq)
}
