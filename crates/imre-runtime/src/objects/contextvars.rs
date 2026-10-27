//! Contexts own shallow bindings. Copying a context shares its map root; the first
//! write detaches it without copying any Python value.
use super::exceptions::{Error, Kind};
use super::words::{OwnedWord, WordKind};
use super::{Object, Storage, StorageCell, Str, finish, object, values};
use crate::StrOwner;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::rc::Rc;

#[derive(Clone)]
struct Binding {
    // Keeps the key address unique as long as the mapping contains it.
    _key: crate::ObjectOwner,
    value: OwnedWord,
}

#[derive(Default)]
pub(super) struct State {
    bindings: RefCell<Rc<HashMap<usize, Binding>>>,
    entered: Cell<bool>,
}
impl State {
    fn copy(&self) -> Rc<Self> {
        Rc::new(Self {
            bindings: RefCell::new(self.bindings.borrow().clone()),
            entered: Cell::new(false),
        })
    }
    fn get(&self, key: *const Object) -> Option<OwnedWord> {
        self.bindings
            .borrow()
            .get(&(key as usize))
            .map(|b| b.value.clone())
    }
    fn replace(&self, key: crate::ObjectOwner, value: Option<OwnedWord>) -> Option<OwnedWord> {
        let id = crate::ObjectOwner::as_ptr(&key) as usize;
        let old = {
            let mut root = self.bindings.borrow_mut();
            let bindings = Rc::make_mut(&mut root);
            match value {
                Some(value) => bindings.insert(id, Binding { _key: key, value }),
                None => bindings.remove(&id),
            }
        };
        old.map(|b| b.value)
    }
}
pub(super) struct Variable {
    name: StrOwner,
    default: Option<OwnedWord>,
}
pub(super) struct Token {
    key: crate::ObjectOwner,
    context: Rc<State>,
    old: Option<OwnedWord>,
    used: Cell<bool>,
}
thread_local! {
    static CURRENT: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
    static TOUCHED: Cell<bool> = const { Cell::new(false) };
    static MISSING: crate::ObjectOwner = {
        let raw = wrap(Storage::TokenMissing);
        // SAFETY: the static boxed view takes the freshly allocated sentinel's one owner.
        let word = unsafe { OwnedWord::from_owned(raw as usize,WordKind::new(5)) };
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::ObjectValue(word))))
    };
    static REPR: RefCell<HashSet<usize>> = RefCell::default();
}
fn current() -> Rc<State> {
    TOUCHED.set(true);
    CURRENT.with_borrow_mut(|slot| slot.get_or_insert_with(Default::default).clone())
}
/// Called when a cold coroutine becomes a separate task, not when it is constructed.
pub(super) fn capture() -> Option<Rc<State>> {
    if !TOUCHED.get() {
        return None;
    }
    CURRENT.with_borrow(|slot| slot.as_ref().map(|ctx| ctx.copy()))
}
/// A first assignment inside a previously empty fiber is saved on its first yield.
pub(super) fn swap_task(task: &mut Option<Rc<State>>) {
    if TOUCHED.get() {
        CURRENT.with_borrow_mut(|slot| std::mem::swap(slot, task));
    }
}
fn wrap(storage: Storage) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(storage))))
}
fn state(obj: &Object) -> Rc<State> {
    // SAFETY: reads only the immutable handle; references never escape storage.
    unsafe {
        obj.0.with(|s| match s {
            Storage::Context(v) => v.clone(),
            _ => unreachable!("typed Context"),
        })
    }
}
fn variable<R>(obj: &Object, f: impl FnOnce(&Variable) -> R) -> R {
    // SAFETY: clone the owner before the caller can inspect a nested object.
    let v = unsafe {
        obj.0.with(|s| match s {
            Storage::ContextVar(v) => v.clone(),
            _ => unreachable!("typed ContextVar"),
        })
    };
    f(&v)
}
fn token<R>(obj: &Object, f: impl FnOnce(&Token) -> R) -> R {
    // SAFETY: no object access is live when the caller inspects a prior context.
    let v = unsafe {
        obj.0.with(|s| match s {
            Storage::ContextToken(v) => v.clone(),
            _ => unreachable!("typed Token"),
        })
    };
    f(&v)
}

pub(super) enum ReprView {
    Context(*const Object),
    Var(*const Object, Rc<Variable>),
    Tok(*const Object, Rc<Token>),
    Missing,
}
pub(super) fn repr_view(owner: *const Object, storage: &Storage) -> ReprView {
    match storage {
        Storage::Context(_) => ReprView::Context(owner),
        Storage::ContextVar(var) => ReprView::Var(owner, var.clone()),
        Storage::ContextToken(tok) => ReprView::Tok(owner, tok.clone()),
        Storage::TokenMissing => ReprView::Missing,
        _ => unreachable!("context object"),
    }
}
impl ReprView {
    /// A callback can remove a pending object's last container reference.
    pub(super) fn owner(&self) -> Option<*const Object> {
        match self {
            Self::Context(owner) | Self::Var(owner, _) | Self::Tok(owner, _) => Some(*owner),
            Self::Missing => None,
        }
    }

    /// Expand a default or token key on the formatter's checked task stack.
    ///
    /// # Safety
    /// This view's exact Context, ContextVar, or Token owner must still be rooted.
    pub(super) unsafe fn begin(
        self,
        out: &mut values::ReprWriter<'_>,
    ) -> Result<Option<(usize, WordKind, ReprEnd)>, std::fmt::Error> {
        match self {
            Self::Context(owner) => write!(out, "<_contextvars.Context object at {owner:p}>")?,
            Self::Var(owner, var) => {
                let key = owner as usize;
                let inserted = REPR.with_borrow_mut(|seen| {
                    if seen.contains(&key) {
                        return Ok(false);
                    }
                    seen.try_reserve(1).map_err(|_| std::fmt::Error)?;
                    Ok(seen.insert(key))
                })?;
                if !inserted {
                    out.write_str("...")?;
                    return Ok(None);
                }
                let tail = ReprEnd {
                    // SAFETY: this view was obtained from the rooted, exact ContextVar.
                    owner: unsafe { crate::ObjectOwner::acquire_word(owner as usize) },
                    key: Some(key),
                };
                out.write_str("<ContextVar name=")?;
                super::exceptions::write_quoted_repr(&var.name, out)?;
                if let Some(value) = &var.default {
                    out.write_str(" default=")?;
                    return Ok(Some((value.word, value.kind, tail)));
                }
                tail.write(out)?;
            }
            Self::Tok(owner, token) => {
                out.write_str(if token.used.get() {
                    "<Token used var="
                } else {
                    "<Token var="
                })?;
                let tail = ReprEnd {
                    // SAFETY: this view was obtained from the rooted, exact Token.
                    owner: unsafe { crate::ObjectOwner::acquire_word(owner as usize) },
                    key: None,
                };
                return Ok(Some((
                    crate::ObjectOwner::as_ptr(&token.key) as usize,
                    WordKind::new(5),
                    tail,
                )));
            }
            Self::Missing => out.write_str("<Token.MISSING>")?,
        }
        Ok(None)
    }
}

pub(super) struct ReprEnd {
    // Keeps the default or token key alive until its representation is complete.
    owner: crate::ObjectOwner,
    key: Option<usize>,
}

impl ReprEnd {
    pub(super) fn write(self, out: &mut values::ReprWriter<'_>) -> std::fmt::Result {
        write!(out, " at {:p}>", crate::ObjectOwner::as_ptr(&self.owner))
    }
}

impl Drop for ReprEnd {
    fn drop(&mut self) {
        if let Some(key) = self.key {
            REPR.with_borrow_mut(|seen| seen.remove(&key));
        }
    }
}

fn write_description(obj: &Object, out: &mut values::ReprWriter<'_>) -> std::fmt::Result {
    // SAFETY: the caller roots the exact ContextVar or Token throughout formatting.
    unsafe {
        values::try_write_unchecked_repr(out, obj as *const Object as usize, WordKind::new(5))
    }
}
fn description(obj: &Object) -> Result<String, Error> {
    let mut out = String::new();
    write_description(obj, &mut values::ReprWriter(&mut out)).map_err(|_| Error::allocation())?;
    Ok(out)
}
fn missing(key: &Object, kind: Kind) -> Error {
    // SAFETY: the exception independently owns the variable passed as its only argument.
    let word = unsafe { WordKind::new(5).acquire(key as *const Object as usize) };
    if kind == Kind::KeyError {
        return Error::key(word);
    }
    match description(key) {
        Ok(text) => Error::object_argument(kind, text, word),
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_contextvars_new(copy_current: bool) -> *const Object {
    wrap(Storage::Context(if copy_current {
        current().copy()
    } else {
        Rc::default()
    }))
}
/// # Safety
/// The caller roots an exact Context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_copy(ctx: *const Object) -> *const Object {
    // SAFETY: the caller supplies the rooted context.
    wrap(Storage::Context(state(unsafe { object(ctx) }).copy()))
}
/// # Safety
/// The name and optional default are rooted in their compiler-proven layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_var(
    name: *const Str,
    kind: u32,
    value: usize,
) -> *const Object {
    // SAFETY: u32::MAX denotes absence; otherwise this word matches the declared kind.
    let default = (kind != u32::MAX).then(|| unsafe { WordKind::new(kind).acquire(value) });
    // SAFETY: the name is an owned native str rooted by the caller.
    let name = unsafe { StrOwner::acquire_word(name as usize) };
    wrap(Storage::ContextVar(Rc::new(Variable { name, default })))
}
/// # Safety
/// The key is a rooted variable; a nonnull context is an exact Context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_contains(ctx: *const Object, key: *const Object) -> bool {
    let ctx = if ctx.is_null() {
        current()
    } else {
        // SAFETY: the nonnull argument roots an exact Context.
        state(unsafe { object(ctx) })
    };
    ctx.bindings.borrow().contains_key(&(key as usize))
}
/// # Safety
/// The key is a rooted variable of the statically requested value type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_read(ctx: *const Object, key: *const Object) -> usize {
    // SAFETY: a null context selects ContextVar.get(); explicit mapping reads ignore defaults.
    let obj = unsafe { object(key) };
    let value = if ctx.is_null() {
        current()
            .get(key)
            .or_else(|| variable(obj, |v| v.default.clone()))
    } else {
        // SAFETY: the nonnull argument roots an exact Context.
        state(unsafe { object(ctx) }).get(key)
    };
    finish(
        value.map(OwnedWord::into_word).ok_or_else(|| {
            missing(
                obj,
                if ctx.is_null() {
                    Kind::LookupError
                } else {
                    Kind::KeyError
                },
            )
        }),
        0,
    )
}
/// # Safety
/// The value and variable remain rooted and have the range's checked value layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_set(
    key: *const Object,
    kind: u32,
    value: usize,
) -> *const Object {
    // SAFETY: the caller's rooted arguments outlive both acquisitions.
    let (key, value) = unsafe {
        (
            crate::ObjectOwner::acquire_word(key as usize),
            WordKind::new(kind).acquire(value),
        )
    };
    let context = current();
    let old = context.replace(crate::ObjectOwner::clone(&key), Some(value));
    wrap(Storage::ContextToken(Rc::new(Token {
        key,
        context,
        old,
        used: Cell::new(false),
    })))
}

/// # Safety
/// The token and optional receiver remain rooted. A null receiver selects the
/// token's own variable for its context-manager exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_reset(key: *const Object, tok: *const Object) {
    // SAFETY: the compiler checks the token layout at the call site.
    let owner = unsafe { object(tok) };
    let result = token(owner, |t| {
        let context = current();
        let error = if t.used.get() {
            Some((Kind::RuntimeError, "has already been used once"))
        } else if !key.is_null() && !std::ptr::eq(key, crate::ObjectOwner::as_ptr(&t.key)) {
            Some((Kind::ValueError, "was created by a different ContextVar"))
        } else if !Rc::ptr_eq(&context, &t.context) {
            Some((Kind::ValueError, "was created in a different Context"))
        } else {
            None
        };
        if let Some((kind, reason)) = error {
            return Err(Error::new(
                kind,
                format!("{} {reason}", description(owner)?),
            ));
        }
        t.used.set(true);
        context.replace(t.key.clone(), t.old.clone());
        Ok(())
    });
    // Formatting an invalid token can raise while rendering its variable's default.
    if super::exceptions::imre_error_pending() == 0 {
        finish(result, ());
    }
}
/// # Safety
/// An exact variable roots its immutable name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_name(key: *const Object) -> *const Str {
    // SAFETY: the source attribute is checked against the receiver type.
    variable(unsafe { object(key) }, |v| {
        StrOwner::into_raw(v.name.clone())
    })
}
/// # Safety
/// An exact token roots its originating variable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_token_var(tok: *const Object) -> *const Object {
    // SAFETY: the source attribute is checked against the receiver type.
    token(unsafe { object(tok) }, |t| {
        crate::ObjectOwner::into_raw(t.key.clone())
    })
}
/// # Safety
/// The caller roots an exact token for the immutable presence test.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_was_missing(tok: *const Object) -> bool {
    // SAFETY: token is a sealed compiler-proven handle.
    token(unsafe { object(tok) }, |t| t.old.is_none())
}
/// # Safety
/// A nonnull token roots its old word; a null token returns Token.MISSING.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_old(tok: *const Object) -> *const Object {
    let missing = || MISSING.with(|m| crate::ObjectOwner::into_raw(m.clone()));
    if tok.is_null() {
        return missing();
    }
    // SAFETY: a typed token owns this old word for the whole operation.
    token(unsafe { object(tok) }, |t| match &t.old {
        None => missing(),
        Some(v) => {
            // Only scalars, tuples and existing ObjectValue views reach this ABI.
            // For kind 5 preserve that existing view, including module attributes.
            if v.kind.raw() == 5 {
                // SAFETY: the token roots the checked view.
                return crate::ObjectOwner::into_raw(unsafe {
                    crate::ObjectOwner::acquire_word(v.word)
                });
            }
            // SAFETY: the token retains this compiler-checked scalar or tuple.
            unsafe { super::boxed_values::imre_box_value(v.kind.raw(), v.word) }
        }
    })
}
/// # Safety
/// The receiver is a rooted exact context. A successful call returns the prior
/// binding context, which the caller must restore in a finally block.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_enter(ctx: *const Object) -> *const Object {
    // SAFETY: the ABI requires a live native Context throughout entry.
    let state = state(unsafe { object(ctx) });
    if state.entered.replace(true) {
        return Error::new(
            Kind::RuntimeError,
            format!(
                "cannot enter context: <_contextvars.Context object at {ctx:p}> is already entered"
            ),
        )
        .returning(std::ptr::null());
    }
    let previous = current();
    CURRENT.replace(Some(state));
    wrap(Storage::Context(previous))
}
/// # Safety
/// Both handles are rooted and originate from the same successful entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_leave(ctx: *const Object, previous: *const Object) {
    // SAFETY: both words retain their checked Context allocations.
    let (ctx, prev) = unsafe { (state(object(ctx)), state(object(previous))) };
    debug_assert!(Rc::ptr_eq(&ctx, &current()));
    ctx.entered.set(false);
    CURRENT.replace(Some(prev));
}
/// # Safety
/// The provided object is an exact rooted Context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_contextvars_len(ctx: *const Object) -> usize {
    // SAFETY: the ABI roots this Context for the binding-count read.
    super::Int::from_usize(state(unsafe { object(ctx) }).bindings.borrow().len()).into_raw()
}
