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
    pub(super) fn message(&self) -> Rc<Str> {
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
    if !repr {
        match kind.raw() {
            // SAFETY: the owning field roots the exact string.
            3 => return unsafe { acquire_rc(word) },
            5 => {
                // SAFETY: kind five roots a concrete object through this borrow.
                let path = unsafe {
                    object(word as *const Object).0.with(|s| {
                        matches!(s, Storage::Path { .. } | Storage::PurePath(_))
                            .then(|| Rc::new(Str::from(s.path())))
                    })
                };
                if let Some(path) = path {
                    return path;
                }
            }
            10 => return Rc::new(super::super::string_enums::display(word).into()),
            _ => {}
        }
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
pub(super) fn concrete(kind: WordKind, word: usize) -> (WordKind, usize) {
    let base = kind.base();
    let word = if kind.raw() == base {
        word
    } else if word == 0 {
        return (WordKind::new(0), 0);
    } else {
        match base {
            1 => word - 1,
            // SAFETY: a present optional float owns an Rc<f64> through this read.
            7 => unsafe { (*(word as *const f64)).to_bits() as usize },
            _ => word,
        }
    };
    if base == 9 {
        let (word, kind) = super::super::unions::member(word);
        // A union boxes small integers to reserve its tag bits. Views use canonical integers.
        let word = if kind.raw() == 2 {
            // SAFETY: the original union roots the untagged BigInt until this read finishes.
            unsafe { Int::borrow(word) }
                .big()
                .to_i64()
                .and_then(Int::small)
                .map_or(word, Int::into_raw)
        } else {
            word
        };
        (kind, word)
    } else {
        (WordKind::new(base), word)
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
    if kind.raw() == 5 {
        // SAFETY: a value-view dictionary can supply an already retained Python value.
        if unsafe {
            object(word as *const Object)
                .0
                .with(|s| matches!(s, Storage::ValueView(_)))
        } {
            // SAFETY: the dictionary roots this retained view.
            return Rc::into_raw(unsafe { acquire_rc(word) });
        }
    }
    // SAFETY: the borrowed source field is rooted when the view acquires its own value.
    let field = unsafe { kind.acquire(word) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ValueView(field)))))
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
    let (kind, word) = unsafe { value_word(value) };
    Rc::into_raw(word_text(kind, word, true))
}

/// Read static record metadata only; ordinary class fields do not give a Python length.
fn tuple_length(word: usize) -> Option<usize> {
    // SAFETY: callers have a rooted kind-six record with a static descriptor.
    let d = unsafe { records::descriptor(word as *const usize) };
    // SAFETY: semantic metadata is either static or absent.
    unsafe { d.semantics.as_ref() }
        .filter(|s| s.kind == 1)
        .map(|_| d.length)
}

fn type_name(kind: WordKind, word: usize) -> String {
    match kind.raw() {
        0 => "NoneType",
        1 => "bool",
        2 => "int",
        3 => "str",
        4 => "list",
        5 => {
            // SAFETY: this field roots its concrete object.
            return unsafe {
                object(word as *const Object).0.with(|s| {
                    match s {
                        Storage::Sequence(_) => "list",
                        Storage::Dictionary(_) => "dict",
                        Storage::Set(_) => "set",
                        Storage::Path { .. } => "PosixPath",
                        Storage::PurePath(p) => p.class(),
                        _ => "object",
                    }
                    .to_owned()
                })
            };
        }
        6 => {
            // SAFETY: descriptor and UTF-8 class name are compiler-issued static data.
            if let Some(s) = unsafe { records::descriptor(word as *const usize).semantics.as_ref() }
            {
                // SAFETY: compiler class names are static UTF-8.
                return unsafe {
                    std::str::from_utf8_unchecked(std::slice::from_raw_parts(s.name, s.name_len))
                }
                .to_owned();
            }
            "object"
        }
        7 => "float",
        8 => "bytes",
        10 => return super::super::string_enums::class_name(word),
        11 | 12 => "type",
        _ => unreachable!("concrete field kind"),
    }
    .to_owned()
}

fn length(kind: WordKind, word: usize) -> Option<usize> {
    // SAFETY: each admitted exact type is read through its own rooted representation.
    unsafe {
        match kind.raw() {
            3 => Some((&*(word as *const Str)).chars().count()),
            4 => Some((&*(word as *const crate::List)).0.with(|v| v.len())),
            5 => object(word as *const Object).0.with(|s| match s {
                Storage::Sequence(v) => Some(v.resolved().values.len()),
                Storage::Dictionary(_) | Storage::Set(_) => s.table_length(),
                _ => None,
            }),
            6 => tuple_length(word),
            8 => Some((&*(word as *const Bytes)).len()),
            10 => super::super::string_enums::string_value(word).map(|s| s.chars().count()),
            _ => None,
        }
    }
}

/// Compute truth from the concrete retained Python value, including empty containers.
pub(super) fn truth(kind: WordKind, word: usize) -> bool {
    match kind.raw() {
        0 => false,
        1 => word != 0,
        2 => word != 1, // canonical tagged zero
        7 => f64::from_bits(word as u64) != 0.0,
        _ => length(kind, word) != Some(0),
    }
}

/// # Safety
/// The caller roots an exception field and checks pending state for TypeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_len(value: *const Object) -> usize {
    // SAFETY: the view retains the original value.
    let (kind, word) = unsafe { value_word(value) };
    match length(kind, word) {
        Some(n) => Int::from_usize(n).into_raw(),
        None => Error::new(
            Kind::TypeError,
            format!("object of type '{}' has no len()", type_name(kind, word)),
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
        let (kind, word) = value_word(value);
        // Integer subscription of a mapping uses the original key, including negatives.
        if kind.raw() == 5 {
            let item = object(word as *const Object).0.with(|s| match s {
                Storage::Dictionary(m) => Some(
                    m.integer_key(index)
                        .ok_or_else(|| Error::key(WordKind::new(2).acquire(index))),
                ),
                Storage::Set(_) => Some(Err(Error::new(
                    Kind::TypeError,
                    "'set' object is not subscriptable",
                ))),
                _ => None,
            });
            if let Some(item) = item {
                return match item {
                    Ok(v) => view(v.kind, v.word),
                    Err(e) => e.returning(std::ptr::null()),
                };
            }
        }
        let Some(len) = length(kind, word) else {
            return Error::new(
                Kind::TypeError,
                format!("'{}' object is not subscriptable", type_name(kind, word)),
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
                    3 | 10 => "string index out of range",
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
            4 => (&*(word as *const crate::List))
                .0
                .with(|v| view(WordKind::new(2), v[i].0)),
            5 => object(word as *const Object).0.with(|s| {
                let v = s.sequence();
                view(v.kind, v.values[i])
            }),
            10 => {
                let text =
                    super::super::string_enums::string_value(word).expect("sized string enum");
                let ch = Rc::new(Str::from(text.chars().nth(i).unwrap().to_string()));
                view(WordKind::new(3), Rc::as_ptr(&ch) as usize)
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
            let matches = match (ka.raw(), kb.raw()) {
                // SAFETY: both rooted operands have the selected numeric kinds.
                (2, 7) => unsafe {
                    crate::floats::imre_int_float_compare(wa, f64::from_bits(wb as u64)) == 0
                },
                // SAFETY: both rooted operands have the converse numeric kinds.
                (7, 2) => unsafe {
                    crate::floats::imre_int_float_compare(wb, f64::from_bits(wa as u64)) == 0
                },
                (3, 10) | (10, 3) => {
                    let (text, tag) = if ka.raw() == 3 { (wa, wb) } else { (wb, wa) };
                    // SAFETY: the string remains rooted during comparison.
                    let text = unsafe { &*(text as *const Str) };
                    super::super::string_enums::string_value(tag).is_some_and(|s| *s == *text)
                }
                _ => false,
            };
            if !matches {
                return false;
            }
        } else if ka.raw() == 10 && wa != wb {
            let (Some(a), Some(b)) = (
                super::super::string_enums::string_value(wa),
                super::super::string_enums::string_value(wb),
            ) else {
                return false;
            };
            if a != b {
                return false;
            }
        } else if ka.raw() == 6
            && wa != wb
            && tuple_length(wa).is_some()
            && tuple_length(wb).is_some()
        {
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
