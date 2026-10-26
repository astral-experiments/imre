//! ImportError retains its positional arguments separately from its named attributes.
use crate::StrOwner;
mod suggestions;

use super::*;
use crate::objects::boxed_values::{
    checked_text, concrete, length, sequence_item, tuple_item, word_text,
};
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
            self.args.message(false)
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

    pub(super) fn message(&self, quote_single: bool) -> Rendered {
        match self.len() {
            0 => rendered(Str::new("")),
            1 => {
                let (kind, word) = self.item(0);
                word_text(kind, word, quote_single)
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

/// # Safety
/// `tuple` is a live, compiler-described immutable tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_tuple_view(tuple: *const usize) -> *const Object {
    view(WordKind::new(6), tuple as usize)
}

fn view(kind: WordKind, word: usize) -> *const Object {
    // SAFETY: every caller keeps this borrowed field rooted for the projection.
    let (kind, word) = unsafe { concrete(kind, word) };
    // SAFETY: the borrowed source field is rooted when the view acquires its own value.
    let field = unsafe { kind.acquire(word) };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::ObjectValue(field),
    ))))
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
    let owner = unsafe { object(value) };
    if super::super::errno::with_nonscalar_member(owner, |_| ()).is_some() {
        return 0;
    }
    // SAFETY: non-scalar metadata was handled above; the view owns this projected value.
    let (actual, stored) = unsafe { exit_word(value) };
    u32::from(actual.raw() == kind && (kind == 0 || word == stored))
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
            Some(spelling) => crate::StrOwner::into_raw(crate::StrOwner::new(
                format!("{}. Did you mean '{spelling}'?", *prefix).into(),
            )),
            None => crate::StrOwner::into_raw(crate::StrOwner::acquire_word(prefix as usize)),
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
    let hook = if repr == 0 { "__str__" } else { "__repr__" };
    // SAFETY: the return value remains rooted through both projections.
    let owner = unsafe { object(value) };
    if let Some(name) = super::super::errno::with_nonscalar_member(owner, |m| m.type_name()) {
        return Error::new(
            Kind::TypeError,
            format!("{hook} returned non-string (type {name})"),
        )
        .returning(std::ptr::null());
    }
    // SAFETY: the remaining value is a scalar or an owning typed view.
    let (kind, word) = unsafe { exit_word(value) };
    if kind.raw() == 3 {
        // SAFETY: representation 3 is a live native string.
        crate::StrOwner::into_raw(unsafe { crate::StrOwner::acquire_word(word) })
    } else {
        Error::new(
            Kind::TypeError,
            format!(
                "{hook} returned non-string (type {})",
                type_name(kind, word)
            ),
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
        crate::ObjectOwner::into_raw(err)
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
            e.clear_args();
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
            exception.clear_args();
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
    // SAFETY: the caller roots the boxed value throughout the immutable observation.
    let owner = unsafe { object(value) };
    if let Some(text) = super::super::errno::with_nonscalar_member(owner, |m| m.text(1)) {
        return text;
    }
    // SAFETY: the view retains the original value.
    unsafe {
        let (kind, word) = exit_word(value);
        checked_text(kind, word, true)
    }
}

fn type_name(kind: WordKind, word: usize) -> std::borrow::Cow<'static, str> {
    let plain = match kind.raw() {
        0 => "NoneType",
        1 => "bool",
        2 => "int",
        3 => "str",
        4 => "list",
        5 => {
            // SAFETY: the caller retains this concrete object's allocation.
            return unsafe {
                object(word as *const Object).0.with(|s| match s {
                    Storage::Sequence(_) => "list".into(),
                    Storage::Dictionary(_) => "dict".into(),
                    Storage::Set(_) => "set".into(),
                    Storage::Path { .. } => if cfg!(windows) {
                        "WindowsPath"
                    } else {
                        "PosixPath"
                    }
                    .into(),
                    Storage::PurePath(path) => path.class().into(),
                    Storage::Exception(e) => e.name().to_owned().into(),
                    _ => "object".into(),
                })
            };
        }
        6 => {
            // SAFETY: the rooted public tuple owns a descriptor with immutable UTF-8 names.
            unsafe {
                if let Some(semantic) = records::descriptor(word as *const usize).semantics.as_ref()
                    && !semantic.name.is_null()
                {
                    let bytes = std::slice::from_raw_parts(semantic.name, semantic.name_len);
                    return std::str::from_utf8_unchecked(bytes).to_owned().into();
                }
            }
            "tuple"
        }
        7 => "float",
        8 => "bytes",
        10 => return super::super::string_enums::class_name(word).into(),
        _ => unreachable!("checked exception field"),
    };
    plain.into()
}

/// # Safety
/// The caller roots an exception field and checks pending state for TypeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_len(value: *const Object) -> usize {
    // SAFETY: the caller roots the boxed value throughout the immutable observation.
    let owner = unsafe { object(value) };
    if let Some(n) = super::super::errno::with_nonscalar_member(owner, |m| m.length()) {
        return n;
    }
    // SAFETY: the view retains the original value.
    let (kind, word) = unsafe { exit_word(value) };
    match super::super::boxed_values::python_length(kind, word) {
        Ok(Some(n)) => Int::from_usize(n).into_raw(),
        Err(error) => error.returning(0),
        Ok(None) => Error::new(
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
    // SAFETY: the caller roots the boxed value throughout the immutable observation.
    let owner = unsafe { object(value) };
    // SAFETY: the caller passes a rooted integer for the index.
    let index_word = unsafe { Int::borrow(index) };
    if let Some(item) = super::super::errno::with_nonscalar_member(owner, |m| m.index(&index_word))
    {
        return item;
    }
    // SAFETY: the field and integer stay live throughout indexing.
    unsafe {
        let (kind, word) = exit_word(value);
        if kind.raw() == 5
            && let Some(item) = object(word as *const Object)
                .0
                .with_mut(|s| s.integer_subscript(index))
        {
            return finish(
                item.map(|owned| view(owned.kind, owned.word)),
                std::ptr::null(),
            );
        }
        let Some(len) = (match length(kind, word) {
            Ok(length) => length,
            Err(error) => return error.returning(std::ptr::null()),
        }) else {
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
                let s = StrOwner::new(Str::from(ch.to_string()));
                view(WordKind::new(3), StrOwner::as_ptr(&s) as usize)
            }
            4..=6 => finish(
                sequence_item(kind, word, i).map(|(k, w)| view(k, w)),
                std::ptr::null(),
            ),
            10 => {
                let text = super::super::string_enums::string_value(word).expect("sized StrEnum");
                let ch = StrOwner::new(Str::from(text.chars().nth(i).unwrap().to_string()));
                view(WordKind::new(3), StrOwner::as_ptr(&ch) as usize)
            }
            8 => view(
                WordKind::new(2),
                Int::from_usize((&*(word as *const Bytes))[i] as usize).into_raw(),
            ),
            _ => unreachable!(),
        }
    }
}
