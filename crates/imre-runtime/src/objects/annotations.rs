//! ForwardRef's immutable fields use the normal owned record layout. Nominal
//! equality omits is_argument, as required by annotationlib's compatibility API.
use super::records::descriptor;
use super::words::{NULLABLE, WordKind};
use super::{Str, exceptions, values};

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
