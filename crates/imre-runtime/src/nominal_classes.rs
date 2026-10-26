//! Class metadata and subclass checks for the closed set of native class values.

use super::{Str, StrOwner};

/// # Safety
/// `class` points to a rooted, fully qualified native class name. Names from
/// builtins omit the module prefix, matching Python's representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_class_name_part(class: *const Str, module: u32) -> *const Str {
    // SAFETY: the caller retains the name until the new string is owned.
    let name = unsafe { &*class }.as_str();
    let (prefix, short) = name.rsplit_once('.').unwrap_or(("builtins", name));
    StrOwner::into_raw(StrOwner::new(Str::new(if module != 0 {
        prefix
    } else {
        short
    })))
}

/// # Safety
/// Both pointers are rooted class names. Virtual ABC registration is independent
/// of the concrete base: an array is a mutable sequence without inheriting it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_builtin_issubclass(child: *const Str, parent: *const Str) -> u32 {
    // SAFETY: these names are immutable throughout the query.
    let (child, parent) = unsafe { ((&*child).as_str(), (&*parent).as_str()) };
    let mutable_sequence = matches!(
        child,
        "list"
            | "bytearray"
            | "array.array"
            | "collections.deque"
            | "collections.abc.MutableSequence"
    );
    u32::from(
        child == parent
            || match parent {
                "object" => true,
                "int" => child == "bool",
                "collections.abc.MutableSequence" => mutable_sequence,
                "collections.abc.Sequence" => {
                    mutable_sequence
                        || matches!(child, "tuple" | "str" | "bytes" | "range" | "memoryview")
                }
                "collections.abc.MutableMapping" => child == "dict",
                "collections.abc.Mapping" => {
                    matches!(child, "dict" | "collections.abc.MutableMapping")
                }
                _ => false,
            },
    )
}
