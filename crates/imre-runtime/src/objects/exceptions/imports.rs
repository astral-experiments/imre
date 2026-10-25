//! ImportError retains its positional arguments separately from its named attributes.
use super::*;
use crate::objects::{records, values};
use crate::{Bytes, Int};

#[derive(Clone)]
pub(super) struct Import {
    pub(super) args: Positional,
    msg: OwnedWord,
    fields: [OwnedWord; 3],
}

impl Import {
    pub(super) fn message(&self) -> Rendered {
        if self.msg.kind.raw() == 3 {
            word_text(self.msg.kind, self.msg.word, false)
        } else {
            self.args.message()
        }
    }
}

/// The actual immutable argument tuple shared by native exception constructors.
#[derive(Clone)]
pub(super) struct Positional(pub(super) OwnedWord);

impl Positional {
    fn len(&self) -> usize {
        // SAFETY: the exception owns the positional tuple.
        unsafe { records::descriptor(self.0.word as *const usize) }.length
    }

    fn item(&self, index: usize) -> (WordKind, usize) {
        // SAFETY: callers checked the index against this owned tuple.
        unsafe { tuple_item(self.0.word, index) }
    }

    pub(super) fn message(&self) -> Rendered {
        match self.len() {
            0 => Ok(Rc::new(Str::new(""))),
            1 => {
                let (kind, word) = self.item(0);
                word_text(kind, word, false)
            }
            _ => word_text(self.0.kind, self.0.word, true),
        }
    }

    pub(super) fn write_repr_args(&self, output: &mut values::ReprWriter<'_>) -> std::fmt::Result {
        use std::fmt::Write as _;
        for i in 0..self.len() {
            if i != 0 {
                output.write_str(", ")?;
            }
            let (kind, word) = self.item(i);
            // SAFETY: each argument stays rooted by the immutable tuple.
            unsafe {
                values::try_write_repr(output, word, kind)?;
            }
        }
        Ok(())
    }
}

/// Reads one statically described field; this works for both inline and shared record storage.
unsafe fn tuple_item(tuple: usize, index: usize) -> (WordKind, usize) {
    let ptr = tuple as *const usize;
    // SAFETY: the caller roots a tuple and proves index is in its descriptor.
    unsafe {
        let kind = WordKind::new(*records::descriptor(ptr).kinds.add(index));
        concrete(kind, kind.read_record_field(ptr.add(index)))
    }
}

pub(super) fn word_text(kind: WordKind, word: usize, repr: bool) -> Rendered {
    if kind.raw() == 3 && !repr {
        // SAFETY: the owning field roots the exact string.
        return Ok(unsafe { acquire_rc(word) });
    }
    word_repr(kind, word).map(|result| Rc::new(result.into()))
}

pub(super) fn word_repr(
    kind: WordKind,
    word: usize,
) -> std::result::Result<String, std::fmt::Error> {
    let mut result = String::new();
    // SAFETY: only concrete, representable field types reach this routine.
    unsafe {
        values::try_write_repr(&mut values::ReprWriter(&mut result), word, kind)?;
    }
    Ok(result)
}

// A tuple may store an optional scalar in its nullable representation. Views
// expose the contained Python value, not the slot's storage tag.
fn concrete(kind: WordKind, word: usize) -> (WordKind, usize) {
    if kind.raw() & crate::objects::words::NULLABLE == 0 {
        (kind, word)
    } else if word == 0 {
        (WordKind::new(0), 0)
    } else {
        let base = kind.raw() & !crate::objects::words::NULLABLE;
        (WordKind::new(base), if base == 1 { word - 1 } else { word })
    }
}

/// # Safety
/// `tuple` is a live, compiler-described immutable tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_tuple_view(tuple: *const usize) -> *const Object {
    view(WordKind::new(6), tuple as usize)
}

fn view(kind: WordKind, word: usize) -> *const Object {
    let (kind, word) = concrete(kind, word);
    // SAFETY: the borrowed source field is rooted when the view acquires its own value.
    let field = unsafe { kind.acquire(word) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ExceptionField(
        field,
    )))))
}

/// # Safety
/// The caller roots the compiler-described positional tuple and three exact typed fields.
/// A non-null descriptor is the static identity of a marker subclass.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_import_error_new(
    kind: u32,
    descriptor: *const NamedDescriptor,
    args: *const usize,
    name_kind: u32,
    name: usize,
    path_kind: u32,
    path: usize,
    from_kind: u32,
    from: usize,
) -> *const Object {
    // SAFETY: every incoming field is rooted until this exception retains it.
    unsafe {
        let named = descriptor.as_ref();
        let (msg_kind, msg) = if records::descriptor(args).length == 1 {
            tuple_item(args as usize, 0)
        } else {
            (WordKind::new(0), 0)
        };
        let payload = Import {
            args: Positional(WordKind::new(6).acquire(args as usize)),
            msg: msg_kind.acquire(msg),
            fields: [(name_kind, name), (path_kind, path), (from_kind, from)]
                .map(|(k, w)| WordKind::new(k).acquire(w)),
        };
        let err = new_exception(
            Kind::from_raw(named.map_or(kind, |d| d.base)),
            Payload::Import(Box::new(payload)),
        );
        if let Some(named) = named {
            err.0.with_mut(|s| {
                let Storage::Exception(e) = s else {
                    unreachable!()
                };
                e.details_mut().named = Some(named);
            });
        }
        Rc::into_raw(err)
    }
}

/// # Safety
/// `err` is a rooted, statically constructed or dynamically narrowed import exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_import_error_field(err: *const Object, field: u32) -> *const Object {
    // SAFETY: the error roots its tuple and named fields through the projection.
    unsafe {
        object(err).0.with(|s| {
            let Payload::Import(e) = &s.exception().payload else {
                unreachable!("proved import error")
            };
            let (kind, word) = match field {
                0 => (e.args.0.kind, e.args.0.word),
                1 => (e.msg.kind, e.msg.word),
                2..=4 => {
                    let x = &e.fields[field as usize - 2];
                    (x.kind, x.word)
                }
                _ => unreachable!("validated import field"),
            };
            view(kind, word)
        })
    }
}

/// # Safety
/// Both arguments are rooted import exceptions; the temporary replacement is fresh.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_import_error_reset(err: *const Object, replacement: *const Object) {
    // SAFETY: read the replacement before mutably borrowing the existing object.
    unsafe {
        let new = object(replacement)
            .0
            .with(|s| s.exception().payload.clone());
        object(err).0.with_mut(|s| {
            let Storage::Exception(e) = s else {
                unreachable!()
            };
            e.payload = new;
        });
    }
}

/// BaseException installs the argument tuple before ImportError validates its keywords.
///
/// # Safety
/// The caller roots the import exception, positional tuple and error message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_import_error_reject_reset(
    err: *const Object,
    args: *const usize,
    message: *const Str,
) {
    // SAFETY: all three inputs remain rooted through the update and error construction.
    unsafe {
        object(err).0.with_mut(|s| {
            let Storage::Exception(exception) = s else {
                unreachable!()
            };
            let Payload::Import(import) = &mut exception.payload else {
                unreachable!()
            };
            import.args = Positional(WordKind::new(6).acquire(args as usize));
        });
        Error::new(Kind::TypeError, (&*message).as_str()).raise();
    }
}

/// # Safety
/// The caller roots a view of a concrete exception field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_repr(value: *const Object) -> *const Str {
    // SAFETY: the view retains the original value.
    let (kind, word) = unsafe { exit_word(value) };
    finish_display(word_text(kind, word, true))
}

fn type_name(kind: WordKind) -> &'static str {
    match kind.raw() {
        0 => "NoneType",
        1 => "bool",
        2 => "int",
        3 => "str",
        6 => "tuple",
        8 => "bytes",
        _ => unreachable!("checked exception field"),
    }
}

fn length(kind: WordKind, word: usize) -> Option<usize> {
    // SAFETY: each admitted exact type is read through its own representation.
    unsafe {
        match kind.raw() {
            3 => Some((&*(word as *const Str)).chars().count()),
            6 => Some(records::descriptor(word as *const usize).length),
            8 => Some((&*(word as *const Bytes)).len()),
            _ => None,
        }
    }
}

/// # Safety
/// The caller roots an exception field and checks pending state for TypeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_len(value: *const Object) -> usize {
    // SAFETY: the view retains the original value.
    let (kind, word) = unsafe { exit_word(value) };
    match length(kind, word) {
        Some(n) => Int::from_usize(n).into_raw(),
        None => Error::new(
            Kind::TypeError,
            format!("object of type '{}' has no len()", type_name(kind)),
        )
        .returning(0),
    }
}

/// # Safety
/// Both the field and the integer index remain rooted; success returns an independent owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_index(
    value: *const Object,
    index: usize,
) -> *const Object {
    // SAFETY: the field and integer stay live throughout indexing.
    unsafe {
        let (kind, word) = exit_word(value);
        let Some(len) = length(kind, word) else {
            return Error::new(
                Kind::TypeError,
                format!("'{}' object is not subscriptable", type_name(kind)),
            )
            .returning(std::ptr::null());
        };
        let at = match crate::decode_index(&Int::borrow(index)) {
            Ok(v) => v,
            Err(e) => return e.returning(std::ptr::null()),
        };
        let i = if at < 0 {
            len.checked_add_signed(at)
        } else {
            Some(at as usize)
        }
        .filter(|&n| n < len);
        let Some(i) = i else {
            return Error::new(
                Kind::IndexError,
                match kind.raw() {
                    3 => "string index out of range",
                    6 => "tuple index out of range",
                    8 => "index out of range",
                    _ => unreachable!(),
                },
            )
            .returning(std::ptr::null());
        };
        match kind.raw() {
            3 => {
                let ch = (&*(word as *const Str)).chars().nth(i).unwrap();
                let s = Rc::new(Str::from(ch.to_string()));
                view(WordKind::new(3), Rc::as_ptr(&s) as usize)
            }
            6 => {
                let (k, w) = tuple_item(word, i);
                view(k, w)
            }
            8 => view(
                WordKind::new(2),
                Int::from_usize((&*(word as *const Bytes))[i] as usize).into_raw(),
            ),
            _ => unreachable!(),
        }
    }
}

/// Compare nested fixed tuples by their members, even across distinct native descriptors.
/// Python's bool/int equivalence applies to leaves at every depth.
pub(super) unsafe fn equal(left: (WordKind, usize), right: (WordKind, usize)) -> bool {
    let numeric = |(k, w): (WordKind, usize)| {
        if k.raw() == 1 {
            (WordKind::new(2), (w << 1) | 1)
        } else {
            (k, w)
        }
    };
    // Only unvisited siblings occupy the worklist. A wide tuple costs no
    // auxiliary space, and ordinary scalar comparisons do not allocate.
    let mut pending = smallvec::SmallVec::<[(usize, usize, usize, usize); 4]>::new();
    let mut next = (left, right);
    loop {
        let ((ka, wa), (kb, wb)) = (
            numeric(concrete(next.0.0, next.0.1)),
            numeric(concrete(next.1.0, next.1.1)),
        );
        if ka != kb {
            return false;
        }
        if ka.raw() == 6 && wa != wb {
            // SAFETY: both words are rooted tuple payloads with static descriptors.
            unsafe {
                let n = records::descriptor(wa as *const usize).length;
                if n != records::descriptor(wb as *const usize).length {
                    return false;
                }
                if n != 0 {
                    if n > 1 {
                        pending.push((wa, wb, 1, n));
                    }
                    next = (tuple_item(wa, 0), tuple_item(wb, 0));
                    continue;
                }
            }
        } else {
            // SAFETY: a matching concrete scalar or identical rooted tuple.
            if !unsafe { values::equal(wa, wb, ka) } {
                return false;
            }
        }
        let Some((a, b, i, n)) = pending.pop() else {
            return true;
        };
        if i + 1 < n {
            pending.push((a, b, i + 1, n));
        }
        // SAFETY: the outer exception or comparison operand roots every tuple.
        next = unsafe { (tuple_item(a, i), tuple_item(b, i)) };
    }
}
