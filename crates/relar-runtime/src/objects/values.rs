//! Generated record operations recurse through the existing concrete word kinds.
//! No operation calls user code or mutates a container; acyclic checked types
//! bound recursion and keep every borrowed child rooted by its original owner.

use std::fmt::Write;
use std::rc::Rc;

use super::records::descriptor;
use super::words::{NULLABLE, WordKind};
use super::{Bytes, Int, List, Object, Storage, Str, fail, object};

const GENERATED_EQ: u32 = 1;
const GENERATED_REPR: u32 = 2;
const TUPLE: u32 = 1;

/// Uses Python 3.13+ dataclass equality: identical objects compare equal before
/// fieldwise comparison, which deliberately has no tuple-style NaN shortcut.
/// Ordinary classes and dataclasses with `eq=False` retain identity equality.
///
/// # Safety
/// Both arguments are rooted, fully initialized records with immutable live
/// descriptors. No mutation or user callback may overlap this operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_record_equal(left: *const usize, right: *const usize) -> u8 {
    // SAFETY: the ABI promises both checked record roots for the entire traversal.
    u8::from(unsafe { record_equal(left, right) })
}

unsafe fn record_equal(left: *const usize, right: *const usize) -> bool {
    if left == right {
        return true;
    }
    // SAFETY: both pointers are rooted record payloads.
    let (left_layout, right_layout) = unsafe { (descriptor(left), descriptor(right)) };
    if !std::ptr::eq(left_layout, right_layout) {
        return false;
    }
    // SAFETY: nonnull semantic descriptors are immutable compiler-emitted data.
    let Some(semantics) = (unsafe { left_layout.semantics.as_ref() }) else {
        return false;
    };
    if semantics.kind != TUPLE && semantics.flags & GENERATED_EQ == 0 {
        return false;
    }
    for index in 0..left_layout.length {
        // SAFETY: the descriptor covers every initialized payload word. Tuples
        // compare all fields; classes have one immutable flag per declared field.
        unsafe {
            if semantics.kind != TUPLE && (*semantics.fields.add(index)).flags & GENERATED_EQ == 0 {
                continue;
            }
            let kind = WordKind::new(*left_layout.kinds.add(index));
            if !equal(
                kind.read_record_field(left.add(index)),
                kind.read_record_field(right.add(index)),
                kind,
            ) {
                return false;
            }
        }
    }
    true
}

/// Canonical integers never box a value in the small-integer range. Mixed
/// representations are therefore unequal, and equality needs no temporary
/// BigInt conversion or call through the cold ordering-comparison path.
unsafe fn integer_equal(left: usize, right: usize) -> bool {
    if left & 1 != right & 1 {
        return false;
    }
    if left & 1 != 0 {
        return left == right;
    }
    // SAFETY: both rooted words are canonical heap integers in this branch.
    let (left, right) = unsafe { (Int::borrow(left), Int::borrow(right)) };
    left.big() == right.big()
}

/// Compares exact representations without transferring owners. Optional float
/// owners are unboxed before equality, preserving IEEE NaN behavior; the checker
/// excludes float-containing containers whose boxed identity would be observable.
///
/// # Safety
/// Each word is rooted and uses `kind`. All descendant ownership graphs are
/// acyclic and remain immutable throughout this nonreentrant traversal.
pub(super) unsafe fn equal(left: usize, right: usize, kind: WordKind) -> bool {
    let raw = kind.raw();
    if raw & NULLABLE != 0 && (left == 0 || right == 0) {
        return left == right;
    }
    // SAFETY: the shared kind selects the exact scalar or rooted allocation.
    unsafe {
        match raw & !NULLABLE {
            0 | 1 => left == right,
            2 => integer_equal(left, right),
            3 => *(left as *const Str) == *(right as *const Str),
            4 => {
                if left == right {
                    return true;
                }
                (&*(left as *const List)).0.with(|left| {
                    (&*(right as *const List)).0.with(|right| {
                        left.len() == right.len()
                            && left
                                .iter()
                                .zip(right)
                                .all(|(left, right)| integer_equal(left.0, right.0))
                    })
                })
            }
            5 => object_equal(left as *const Object, right as *const Object),
            6 => record_equal(left as *const usize, right as *const usize),
            7 if raw & NULLABLE != 0 => *(left as *const f64) == *(right as *const f64),
            7 => f64::from_bits(left as u64) == f64::from_bits(right as u64),
            8 => *(left as *const Bytes) == *(right as *const Bytes),
            9 if left & 1 != right & 1 => false,
            9 if left & 1 == 0 => *(left as *const Str) == *(right as *const Str),
            9 => *((left & !1) as *const Bytes) == *((right & !1) as *const Bytes),
            _ => unreachable!("WordKind validates concrete representations"),
        }
    }
}

unsafe fn object_equal(left: *const Object, right: *const Object) -> bool {
    if left == right {
        return true;
    }
    // SAFETY: immutable access cannot invoke callbacks; acyclic types and the
    // identity check prevent reentering the same cell through a descendant.
    unsafe {
        object(left).0.with(|left| {
            object(right).0.with(|right| match (left, right) {
                (Storage::Sequence(left), Storage::Sequence(right)) => {
                    left.kind == right.kind
                        && left.values.len() == right.values.len()
                        && left
                            .values
                            .iter()
                            .zip(&right.values)
                            .all(|(&left_word, &right_word)| {
                                equal(left_word, right_word, left.kind)
                            })
                }
                (Storage::Dictionary(left), Storage::Dictionary(right)) => left.equal(right),
                (Storage::Set(left), Storage::Set(right)) => left.equal(right),
                (Storage::Path(left), Storage::Path(right)) => left == right,
                _ => false,
            })
        })
    }
}

/// Returns one owned Python representation, borrowing every input until the
/// string is complete. Compiler validation excludes resource representations.
///
/// # Safety
/// `word` uses `kind` and roots an initialized acyclic value throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_value_repr(word: usize, kind: u32) -> *const Str {
    let mut output = String::new();
    // SAFETY: the caller supplies the exact checked representation and root.
    unsafe { write_repr(&mut output, word, WordKind::new(kind)) };
    Rc::into_raw(Rc::new(Str::from(output)))
}

/// # Safety
/// The word and all descendants are rooted with the supplied representation;
/// no runtime mutation or user callback can overlap formatting.
pub(super) unsafe fn write_repr(output: &mut String, word: usize, kind: WordKind) {
    let raw = kind.raw();
    if raw & NULLABLE != 0 && word == 0 {
        output.push_str("None");
        return;
    }
    // SAFETY: each branch borrows only its exact immutable scalar or container.
    unsafe {
        match raw & !NULLABLE {
            0 => output.push_str("None"),
            1 => {
                let value = if raw & NULLABLE != 0 { word - 1 } else { word };
                output.push_str(if value == 0 { "False" } else { "True" });
            }
            2 => {
                let value = Int::borrow(word);
                if let Some(value) = value.as_small() {
                    write!(output, "{value}").unwrap();
                } else {
                    write!(output, "{}", value.big()).unwrap();
                }
            }
            3 => output.push_str(&super::exceptions::string_repr(&*(word as *const Str))),
            4 => (&*(word as *const List)).0.with(|values| {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    write_repr(output, value.0, WordKind::new(2));
                }
                output.push(']');
            }),
            5 => object(word as *const Object)
                .0
                .with(|storage| match storage {
                    Storage::Sequence(values) => {
                        output.push('[');
                        for (index, &value) in values.values.iter().enumerate() {
                            if index != 0 {
                                output.push_str(", ");
                            }
                            write_repr(output, value, values.kind);
                        }
                        output.push(']');
                    }
                    Storage::Dictionary(value) => value.write_repr(output),
                    Storage::Set(value) => value.write_repr(output),
                    Storage::Path(value) => {
                        output.push_str("PosixPath(");
                        output.push_str(&super::exceptions::string_repr(value));
                        output.push(')');
                    }
                    _ => fail("SystemError", "unsupported resource representation"),
                }),
            6 => record_repr(output, word as *const usize),
            7 => {
                let value = if raw & NULLABLE != 0 {
                    *(word as *const f64)
                } else {
                    f64::from_bits(word as u64)
                };
                output.push_str(&crate::floats::render(value));
            }
            8 => output.push_str(&super::bytes::repr(&*(word as *const Bytes))),
            9 if word & 1 == 0 => {
                write_repr(output, word, WordKind::new(3));
            }
            9 => write_repr(output, word & !1, WordKind::new(8)),
            _ => unreachable!("WordKind validates concrete representations"),
        }
    }
}

/// The compiler emits these bytes from already validated source identifiers.
unsafe fn text<'a>(pointer: *const u8, length: usize) -> &'a str {
    // SAFETY: metadata owns this immutable, valid UTF-8 slice for the process.
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(pointer, length)) }
}

unsafe fn record_repr(output: &mut String, payload: *const usize) {
    // SAFETY: the caller roots a fully initialized record and its descriptor.
    let layout = unsafe { descriptor(payload) };
    // SAFETY: public record representations always have static semantic metadata.
    let semantics = unsafe { layout.semantics.as_ref() }
        .unwrap_or_else(|| fail("SystemError", "internal records have no representation"));
    // SAFETY: checked descriptors contain valid names, flags, and field arrays.
    unsafe {
        if semantics.kind != TUPLE && semantics.flags & GENERATED_REPR == 0 {
            write!(
                output,
                "<{} object at {payload:p}>",
                text(semantics.qualified_name, semantics.qualified_name_len)
            )
            .unwrap();
            return;
        }
        if semantics.kind != TUPLE {
            output.push_str(text(semantics.name, semantics.name_len));
        }
        output.push('(');
        let mut written = 0;
        for index in 0..layout.length {
            let field = if semantics.kind == TUPLE {
                None
            } else {
                Some(&*semantics.fields.add(index))
            };
            if field.is_some_and(|field| field.flags & GENERATED_REPR == 0) {
                continue;
            }
            if written != 0 {
                output.push_str(", ");
            }
            if let Some(field) = field {
                output.push_str(text(field.name, field.name_len));
                output.push('=');
            }
            let kind = WordKind::new(*layout.kinds.add(index));
            write_repr(output, kind.read_record_field(payload.add(index)), kind);
            written += 1;
        }
        if semantics.kind == TUPLE && written == 1 {
            output.push(',');
        }
        output.push(')');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::records::{
        RecordDescriptor, RecordField, RecordSemantics, relar_record_new, relar_record_release,
    };

    #[test]
    fn generated_equality_preserves_identity_and_fieldwise_nan_comparison() {
        let name = b"Reading";
        let field_name = b"value";
        let fields = [RecordField {
            name: field_name.as_ptr(),
            name_len: field_name.len(),
            flags: GENERATED_EQ | GENERATED_REPR,
        }];
        let semantics = RecordSemantics {
            kind: 0,
            flags: GENERATED_EQ | GENERATED_REPR,
            name: name.as_ptr(),
            name_len: name.len(),
            qualified_name: name.as_ptr(),
            qualified_name_len: name.len(),
            fields: fields.as_ptr(),
        };
        let kinds = [7];
        let layout = RecordDescriptor {
            length: 1,
            kinds: kinds.as_ptr(),
            semantics: &semantics,
        };
        // SAFETY: stack descriptors and names outlive both exact initialized records.
        unsafe {
            let left = relar_record_new(&layout);
            let right = relar_record_new(&layout);
            left.cast::<f64>().write(f64::NAN);
            right.cast::<f64>().write(f64::NAN);
            assert_eq!(relar_record_equal(left, left), 1);
            assert_eq!(relar_record_equal(left, right), 0);
            let rendered = Rc::from_raw(relar_value_repr(left as usize, 6));
            assert_eq!(rendered.as_str(), "Reading(value=nan)");
            left.cast::<f64>().write(-0.0);
            right.cast::<f64>().write(0.0);
            assert_eq!(relar_record_equal(left, right), 1);
            relar_record_release(left);
            relar_record_release(right);
        }
    }
}
