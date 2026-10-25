//! ImportError retains its positional arguments separately from its named attributes.
use super::*;
use crate::objects::boxed_values::{checked_text, tuple_item, word_text};
use crate::objects::{records, values};
use crate::{Bytes, Int};

#[derive(Clone)]
pub(super) struct Import {
    args: OwnedWord,
    msg: OwnedWord,
    fields: [OwnedWord; 3],
}

impl Import {
    fn len(&self) -> usize {
        // SAFETY: the exception owns the positional tuple.
        unsafe { records::descriptor(self.args.word as *const usize) }.length
    }

    fn item(&self, index: usize) -> (WordKind, usize) {
        // SAFETY: callers checked the index against this owned tuple.
        unsafe { tuple_item(self.args.word, index) }
    }

    pub(super) fn message(&self) -> Rc<Str> {
        if self.msg.kind.raw() == 3 {
            return word_text(self.msg.kind, self.msg.word, false);
        }
        match self.len() {
            0 => Rc::new(Str::new("")),
            1 => {
                let (kind, word) = self.item(0);
                word_text(kind, word, false)
            }
            _ => word_text(self.args.kind, self.args.word, true),
        }
    }

    pub(super) fn repr_args(&self) -> Str {
        let mut result = String::new();
        for i in 0..self.len() {
            if i != 0 {
                result.push_str(", ");
            }
            let (kind, word) = self.item(i);
            // SAFETY: each argument stays rooted by the immutable tuple.
            unsafe {
                values::write_repr(&mut result, word, kind);
            }
        }
        result.into()
    }
}

fn view(kind: WordKind, word: usize) -> *const Object {
    // SAFETY: the borrowed source field is rooted when the view acquires its own value.
    let field = unsafe { kind.acquire(word) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ObjectValue(
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
            args: WordKind::new(6).acquire(args as usize),
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
                0 => (e.args.kind, e.args.word),
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
            import.args = WordKind::new(6).acquire(args as usize);
        });
        Error::new(Kind::TypeError, (&*message).as_str()).raise();
    }
}

/// # Safety
/// The caller roots a view of a concrete exception field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_repr(value: *const Object) -> *const Str {
    // SAFETY: the view retains the original value.
    unsafe {
        let (kind, word) = exit_word(value);
        checked_text(kind, word, true)
    }
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
