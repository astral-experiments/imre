//! ImportError retains its positional arguments separately from its named attributes.
mod suggestions;

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
    pub(super) fn message(&self) -> Rc<Str> {
        if self.msg.kind.raw() == 3 {
            word_text(self.msg.kind, self.msg.word, false)
        } else {
            self.args.message()
        }
    }

    /// A live native module supplies candidates just before uncaught display.
    pub(super) fn suggestion(&self) -> Option<&'static str> {
        let text = |field: &OwnedWord| {
            (field.kind == WordKind::new(3)).then(|| {
                // SAFETY: the concrete field owns this exact string through the lookup.
                unsafe { &*(field.word as *const Str) }.as_str()
            })
        };
        if text(&self.fields[0])? != "errno" {
            return None;
        }
        let wrong = text(&self.fields[2])?;
        suggestions::closest(wrong, crate::objects::errno::exported_names())
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

    pub(super) fn message(&self) -> Rc<Str> {
        match self.len() {
            0 => Rc::new(Str::new("")),
            1 => {
                let (kind, word) = self.item(0);
                word_text(kind, word, false)
            }
            _ => word_text(self.0.kind, self.0.word, true),
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

/// Reads one statically described field; this works for both inline and shared record storage.
unsafe fn tuple_item(tuple: usize, index: usize) -> (WordKind, usize) {
    let ptr = tuple as *const usize;
    // SAFETY: the caller roots a tuple and proves index is in its descriptor.
    unsafe {
        let kind = WordKind::new(*records::descriptor(ptr).kinds.add(index));
        concrete(kind, kind.read_record_field(ptr.add(index)))
    }
}

pub(super) fn word_text(kind: WordKind, word: usize, repr: bool) -> Rc<Str> {
    if kind.raw() == 3 && !repr {
        // SAFETY: the owning field roots the exact string.
        return unsafe { acquire_rc(word) };
    }
    let mut result = String::new();
    // SAFETY: only concrete, representable field types reach this routine.
    unsafe {
        values::write_repr(&mut result, word, kind);
    }
    Rc::new(result.into())
}

// A tuple may store an optional scalar in its nullable representation. Views
// expose the contained Python value, not the slot's storage tag.
fn concrete(kind: WordKind, word: usize) -> (WordKind, usize) {
    if kind.raw() & crate::objects::words::NULLABLE == 0 {
        if kind.raw() == 5 {
            // SAFETY: kind 5 roots a native object; a nested field retains its own value.
            unsafe {
                object(word as *const Object).0.with(|s| match s {
                    Storage::ExceptionField(value) => (value.kind, value.word),
                    Storage::Exception(e) => match &e.payload {
                        Payload::Exit(Some(value)) => (value.kind, value.word),
                        Payload::Exit(None) => (WordKind::new(0), 0),
                        _ => (kind, word),
                    },
                    _ => (kind, word),
                })
            }
        } else {
            (kind, word)
        }
    } else if word == 0 {
        (WordKind::new(0), 0)
    } else {
        let base = kind.raw() & !crate::objects::words::NULLABLE;
        let value = match base {
            1 => word - 1,
            // SAFETY: a present nullable float points to its rooted boxed f64.
            7 => unsafe { (*(word as *const f64)).to_bits() as usize },
            _ => word,
        };
        concrete(WordKind::new(base), value)
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

/// Test identity against the None singleton or a list with a stable native owner.
///
/// # Safety
/// Both the retained view and the compiler-checked comparison operand are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_identity(
    value: *const Object,
    kind: u32,
    word: usize,
) -> u32 {
    debug_assert!(matches!(kind, 0 | 4 | 5));
    // SAFETY: a field view owns the concrete value.
    let (actual, stored) = unsafe { exit_word(value) };
    u32::from(actual.raw() == kind && (kind == 0 || word == stored))
}

/// Retain a Python argument independently for a stored exception attribute.
///
/// # Safety
/// The word has the stated representation and remains rooted through acquisition.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_box_field(word: usize, kind: u32) -> *const Object {
    view(WordKind::new(kind), word)
}

/// Apply CPython's spelling hint in original parameter order.
///
/// # Safety
/// Both strings and every element of the fixed string tuple are rooted for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_keyword_message(
    prefix: *const Str,
    wrong: *const Str,
    names: *const usize,
) -> *const Str {
    // SAFETY: the compiler describes this bounds-checked tuple as all exact strings.
    unsafe {
        let count = records::descriptor(names).length;
        let choices: Vec<_> = (0..count)
            .map(|i| {
                let (kind, ptr) = tuple_item(names as usize, i);
                debug_assert_eq!(kind.raw(), 3);
                (&*(ptr as *const Str)).as_str()
            })
            .collect();
        match suggestions::declaration_order((&*wrong).as_str(), &choices) {
            Some(spelling) => Rc::into_raw(Rc::new(
                format!("{}. Did you mean '{spelling}'?", &*prefix).into(),
            )),
            None => Rc::into_raw(acquire_rc(prefix as usize)),
        }
    }
}

/// Verify the formatter's result before using it as a native UTF-8 string.
///
/// # Safety
/// The field view is rooted and the caller checks pending errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_formatter_result(
    value: *const Object,
    repr: u32,
) -> *const Str {
    // SAFETY: the field view retains this exact value through the check.
    let (kind, word) = unsafe { exit_word(value) };
    if kind.raw() == 3 {
        // SAFETY: representation 3 is a live native string.
        Rc::into_raw(unsafe { acquire_rc(word) })
    } else {
        let hook = if repr == 0 { "__str__" } else { "__repr__" };
        Error::new(
            Kind::TypeError,
            format!("{hook} returned non-string (type {})", type_name(kind)),
        )
        .returning(std::ptr::null())
    }
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
            fields: [(name_kind, name), (path_kind, path), (from_kind, from)].map(|(k, w)| {
                let (kind, word) = concrete(WordKind::new(k), w);
                kind.acquire(word)
            }),
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
    Rc::into_raw(word_text(kind, word, true))
}

fn type_name(kind: WordKind) -> &'static str {
    match kind.raw() {
        0 => "NoneType",
        1 => "bool",
        2 => "int",
        3 => "str",
        4 | 5 => "list",
        6 => "tuple",
        7 => "float",
        8 => "bytes",
        _ => unreachable!("checked exception field"),
    }
}

pub(super) fn length(kind: WordKind, word: usize) -> Option<usize> {
    // SAFETY: each admitted exact type is read through its own representation.
    unsafe {
        match kind.raw() {
            3 => Some((&*(word as *const Str)).chars().count()),
            4 => Some((&*(word as *const crate::List)).0.with(Vec::len)),
            5 => object(word as *const Object).0.with(|s| match s {
                Storage::Sequence(v) => Some(v.values.len()),
                _ => None,
            }),
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
/// The caller roots a sequence and has checked the element index against its length.
unsafe fn sequence_item(kind: WordKind, word: usize, i: usize) -> (WordKind, usize) {
    // SAFETY: each concrete representation retains its element during the read.
    unsafe {
        match kind.raw() {
            4 => (&*(word as *const crate::List))
                .0
                .with(|v| (WordKind::new(2), v[i].0)),
            5 => object(word as *const Object).0.with(|s| {
                let Storage::Sequence(v) = s else {
                    unreachable!("admitted typed list")
                };
                concrete(v.kind, v.values[i])
            }),
            6 => tuple_item(word, i),
            _ => unreachable!("list or tuple"),
        }
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
                    4 | 5 => "list index out of range",
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
            4..=6 => {
                let (k, w) = sequence_item(kind, word, i);
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

/// Compare nested native sequences independent of their storage specialization.
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
    type Field = (WordKind, usize);
    let mut pending = smallvec::SmallVec::<[(Field, Field, usize, usize); 4]>::new();
    let mut next = (left, right);
    loop {
        let ((ka, wa), (kb, wb)) = (
            numeric(concrete(next.0.0, next.0.1)),
            numeric(concrete(next.1.0, next.1.1)),
        );
        let sequence = |kind: WordKind| match kind.raw() {
            4 | 5 => 1,
            6 => 2,
            _ => 0,
        };
        let (a, b) = ((ka, wa), (kb, wb));
        if sequence(ka) != 0 && sequence(ka) == sequence(kb) {
            // SAFETY: both words are rooted exact sequences, read within checked bounds.
            unsafe {
                let n = length(ka, wa).unwrap();
                if Some(n) != length(kb, wb) {
                    return false;
                }
                if !(ka == kb && wa == wb) && n != 0 {
                    if n > 1 {
                        pending.push((a, b, 1, n));
                    }
                    next = (sequence_item(ka, wa, 0), sequence_item(kb, wb, 0));
                    continue;
                }
            }
        } else if ka != kb {
            let (integer, float) = match (ka.raw(), kb.raw()) {
                (2, 7) => (wa, wb),
                (7, 2) => (wb, wa),
                _ => return false,
            };
            // SAFETY: the exact int stays rooted throughout the lossless comparison.
            if crate::floats::compare(
                &*unsafe { Int::borrow(integer) },
                f64::from_bits(float as u64),
            ) != 0
            {
                return false;
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
        next = unsafe { (sequence_item(a.0, a.1, i), sequence_item(b.0, b.1, i)) };
    }
}
