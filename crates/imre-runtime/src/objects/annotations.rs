//! ForwardRef's immutable fields use the normal owned record layout. Nominal
//! equality omits is_argument, as required by annotationlib's compatibility API.
use super::records::descriptor;
use super::words::{NULLABLE, WordKind};
use super::{Str, exceptions, values};

/// # Safety
/// The immutable annotation box contains exactly one initialized member.
pub(super) unsafe fn member(word: usize) -> (WordKind, usize) {
    // SAFETY: the member is at offset zero regardless of its representation.
    unsafe { field(word as *const usize, 0) }
}

/// # Safety
/// Generated code roots the immutable annotation box.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_annotation_kind(word: *const usize) -> u32 {
    // SAFETY: the member descriptor lives as long as the boxed value.
    unsafe { member(word as usize).0.raw() }
}

/// # Safety
/// Both pointers borrow immutable annotation boxes of the closed representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_annotation_equal(left: *const usize, right: *const usize) -> u32 {
    // SAFETY: the generic equality traversal reads only the retained exact members.
    u32::from(unsafe { values::equal(left as usize, right as usize, WordKind::new(13)) })
}

/// # Safety
/// Both arguments borrow live annotation boxes through this comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_annotation_is(left: *const usize, right: *const usize) -> u32 {
    // SAFETY: the public identity is that of the two boxed members.
    unsafe {
        let (ka, a) = member(left as usize);
        let (kb, b) = member(right as usize);
        u32::from(ka == kb && (a == b || (ka.raw() == 11 && values::equal(a, b, ka))))
    }
}

/// # Safety
/// The arguments root the annotation and attribute name. The returned ForwardRef is owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_annotation_ref(word: *const usize, name: *const Str) -> *const usize {
    // SAFETY: the immutable member descriptor identifies its exact layout.
    unsafe {
        let (kind, field) = member(word as usize);
        if kind.raw() == 12 {
            super::records::imre_record_retain(field as *const usize);
            return field as *const usize;
        }
        let what = match kind.raw() {
            0 => "'NoneType' object".to_owned(),
            3 => "'str' object".to_owned(),
            11 => format!("type object '{}'", *(field as *const Str)),
            _ => unreachable!("unsupported annotation member"),
        };
        exceptions::Error::new(
            exceptions::Kind::AttributeError,
            format!("{what} has no attribute '{}'", *name),
        )
        .raise();
        std::ptr::null()
    }
}

/// # Safety
/// Both arguments borrow exact builtin class names from the compiler's closed domain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_builtin_issubclass(first: *const Str, second: *const Str) -> u32 {
    // SAFETY: builtin class operands retain their immutable native name storage.
    let (first, second) = unsafe { ((&*first).as_str(), (&*second).as_str()) };
    u32::from(first == second || second == "object" || (first == "bool" && second == "int"))
}

/// Read the exact field kind from the descriptor, including the native owner type.
unsafe fn field(record: *const usize, index: usize) -> (WordKind, usize) {
    // SAFETY: the internal constructor emits five initialized fields in signature order.
    unsafe {
        let kind = WordKind::new(*descriptor(record).kinds.add(index));
        (kind, kind.read_record_field(record.add(index)))
    }
}

/// # Safety
/// Both words root ForwardRef records with live immutable metadata.
pub(super) unsafe fn equal(left: usize, right: usize) -> bool {
    if left == right {
        return true;
    }
    // SAFETY: reads and recursive comparisons borrow fields from both rooted records.
    unsafe {
        [0, 1, 2, 4].into_iter().all(|i| {
            let (kind, a) = field(left as *const usize, i);
            let (other, b) = field(right as *const usize, i);
            kind == other && values::equal(a, b, kind)
        })
    }
}

/// # Safety
/// The word roots a ForwardRef; its owner has a compiler-checked printable layout.
pub(super) unsafe fn write_repr(output: &mut String, word: usize) {
    let record = word as *const usize;
    // SAFETY: all fields stay rooted during formatting, including the optional module and owner.
    unsafe {
        output.push_str("ForwardRef(");
        exceptions::push_string_repr(&*(field(record, 0).1 as *const Str), output);
        let (kind, module) = field(record, 1);
        if module != 0 {
            output.push_str(", module=");
            values::write_repr(output, module, kind);
        }
        if field(record, 4).1 != 0 {
            output.push_str(", is_class=True");
        }
        let (kind, owner) = field(record, 2);
        if kind.raw() != 0 && (kind.raw() & NULLABLE == 0 || owner != 0) {
            output.push_str(", owner=");
            values::write_repr(output, owner, kind);
        }
        output.push(')');
    }
}

/// # Safety
/// Generated code roots both native references through this comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_forward_ref_equal(left: *const usize, right: *const usize) -> u32 {
    // SAFETY: the generated signature supplies both ForwardRef records.
    u32::from(unsafe { equal(left as usize, right as usize) })
}
