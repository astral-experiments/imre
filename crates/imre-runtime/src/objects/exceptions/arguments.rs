//! The argument tuple exposed by BaseException, independent of its display message.
use super::*;
use crate::objects::records::{self, RecordDescriptor, RecordSemantics};
use crate::objects::words::Words;
use smallvec::{SmallVec, smallvec};

/// A task group can be a member's cause. Its projected tuple must not retain that cycle.
#[derive(Clone)]
pub(super) enum Cached {
    Owned(crate::ObjectOwner),
    Group {
        tuple: crate::WeakObject,
        items: crate::WeakObject,
    },
}

impl Cached {
    fn get(&self) -> Option<crate::ObjectOwner> {
        match self {
            Self::Owned(view) => Some(view.clone()),
            Self::Group { tuple, .. } => tuple.upgrade(),
        }
    }

    fn caused_group(view: &crate::ObjectOwner) -> Self {
        // SAFETY: group projections have the exact shape (str, list[exception]).
        let items: crate::ObjectOwner = unsafe {
            view.0.with(|s| {
                let Storage::ObjectValue(tuple) = s else {
                    unreachable!()
                };
                let (kind, word) = super::super::boxed_values::tuple_item(tuple.word, 1);
                debug_assert_eq!(kind.raw(), 5);
                crate::ObjectOwner::acquire_word(word)
            })
        };
        Self::Group {
            tuple: crate::ObjectOwner::downgrade(view),
            items: crate::ObjectOwner::downgrade(&items),
        }
    }
}

impl Exception {
    pub(super) fn clear_args(&mut self) {
        if self.details.as_deref().is_some_and(|d| d.args.is_some()) {
            self.details_mut().args = None;
        }
    }

    /// A child raised from its own group acquires its parent. Release the opposing
    /// edge before installing that cause, preserving every still-live argument view.
    pub(super) fn detach_group_args(&mut self) {
        if let Some(Cached::Owned(view)) = self.details.as_deref().and_then(|d| d.args.as_ref()) {
            let weak = Cached::caused_group(view);
            self.details_mut().args = Some(weak);
        }
    }
}

type Fields = SmallVec<[OwnedWord; 5]>;

const TUPLE: RecordSemantics = RecordSemantics {
    kind: 1,
    flags: 3,
    name: std::ptr::null(),
    name_len: 0,
    qualified_name: std::ptr::null(),
    qualified_name_len: 0,
    fields: std::ptr::null(),
};

/// Runtime-originated errors have at most five arguments; source tuples reuse their own layout.
/// Using erased fields shares the normal tuple representation without interning arbitrary shapes.
fn layout<const N: usize>() -> &'static RecordDescriptor {
    const {
        &RecordDescriptor {
            length: N,
            kinds: [5; N].as_ptr(),
            semantics: &TUPLE,
        }
    }
}

fn view(field: OwnedWord) -> crate::ObjectOwner {
    crate::ObjectOwner::new(Object(StorageCell::new(Storage::ObjectValue(field))))
}

fn string(text: crate::StrOwner) -> OwnedWord {
    // SAFETY: the pointer transfers exactly one owned native string.
    unsafe { OwnedWord::from_owned(StrOwner::into_raw(text) as usize, WordKind::new(3)) }
}

fn tuple(fields: Fields) -> Result<OwnedWord> {
    let descriptor = match fields.len() {
        0 => layout::<0>(),
        1 => layout::<1>(),
        2 => layout::<2>(),
        3 => layout::<3>(),
        4 => layout::<4>(),
        5 => layout::<5>(),
        _ => unreachable!("source arguments already have an exact tuple"),
    };
    // SAFETY: the static tuple layout describes one owned object view per slot.
    unsafe {
        let raw = records::allocate_record(descriptor);
        if raw.is_null() {
            return Err(Error::allocation());
        }
        for (i, field) in fields.into_iter().enumerate() {
            raw.add(i)
                .write(crate::ObjectOwner::into_raw(view(field)) as usize);
        }
        Ok(OwnedWord::from_owned(raw as usize, WordKind::new(6)))
    }
}

fn empty() -> Result<crate::ObjectOwner> {
    thread_local! { static EMPTY: std::cell::OnceCell<crate::ObjectOwner> = const { std::cell::OnceCell::new() }; }
    EMPTY.with(|entry| {
        if let Some(empty) = entry.get() {
            return Ok(empty.clone());
        }
        let value = view(tuple(Fields::new())?);
        entry.set(value.clone()).unwrap_or_else(|_| unreachable!());
        Ok(value)
    })
}

/// The exception owns every source value, including mutable command lists and undecoded bytes.
fn project(exception: &Exception, owner: &crate::ObjectOwner) -> Result<crate::ObjectOwner> {
    let fields = match &exception.payload {
        Payload::Empty | Payload::Exit(None) => return empty(),
        Payload::Message(text) => smallvec![string(text.clone())],
        Payload::Key(key) | Payload::Exit(Some(key)) => smallvec![key.clone()],
        Payload::Arguments(arguments) => arguments.args.clone(),
        Payload::Import(import) => return Ok(view(import.args.0.clone())),
        Payload::Positional(args) | Payload::ExitArguments(args) => {
            return Ok(view(args.0.clone()));
        }
        Payload::Tree(errors) => return Ok(view(errors.args(layout::<1>())?)),
        Payload::Json(error) => {
            // JSONDecodeError calls ValueError.__init__ with this fully rendered string.
            let text = format!(
                "{}: line {} column {} (char {})",
                error.msg, error.lineno, error.colno, error.pos
            );
            smallvec![string(StrOwner::new(text.into()))]
        }
        Payload::Process(error) => error.arguments(),
        Payload::Timeout(error) => error.arguments(),
        Payload::Group(members) => {
            // A caller may keep args[1] without keeping the outer tuple.
            let saved_items = exception.details.as_deref().and_then(|d| match &d.args {
                Some(Cached::Group { items, .. }) => items.upgrade(),
                _ => None,
            });
            let list = if let Some(items) = saved_items {
                items
            } else {
                let mut values = Words::new(5);
                values
                    .values
                    .try_reserve(members.len())
                    .map_err(|_| Error::allocation())?;
                for member in members {
                    values
                        .values
                        .push(crate::ObjectOwner::into_raw(member.get(owner)) as usize);
                }
                values.update_header();
                crate::ObjectOwner::new(Object(StorageCell::new(Storage::Sequence(values))))
            };
            smallvec![
                Error::string_arg("unhandled errors in a TaskGroup"),
                OwnedWord {
                    kind: WordKind::new(5),
                    word: crate::ObjectOwner::into_raw(list) as usize
                }
            ]
        }
    };
    Ok(view(tuple(fields)?))
}

/// Return an independently owned, immutable view of the original positional arguments.
///
/// # Safety
/// The caller roots an actual exception and checks pending state on allocation failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_args(value: *const Object) -> *const Object {
    // SAFETY: the input is a rooted exception object throughout the operation.
    let owner: crate::ObjectOwner = unsafe { crate::ObjectOwner::acquire_word(value as usize) };
    // SAFETY: projection retains only sealed fields and cannot call user code.
    let result = unsafe {
        owner.0.with_mut(|s| {
            let Storage::Exception(e) = s else {
                unreachable!("exception operand")
            };
            if let Some(v) = e
                .details
                .as_deref()
                .and_then(|d| d.args.as_ref())
                .and_then(Cached::get)
            {
                return Ok(v);
            }
            let v = project(e, &owner)?;
            let caused = match &e.payload {
                Payload::Group(members) => {
                    members.iter().any(|m| matches!(m, GroupMember::Caused(_)))
                }
                _ => false,
            };
            let cached = if caused {
                Cached::caused_group(&v)
            } else {
                Cached::Owned(v.clone())
            };
            e.details_mut().args = Some(cached);
            Ok(v)
        })
    };
    finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// Format a structured error using the same arguments returned by its public getter.
pub(super) unsafe fn write_projected_repr(
    owner: *const Object,
    output: &mut super::super::values::ReprWriter<'_>,
) -> std::fmt::Result {
    // SAFETY: the caller roots an actual exception; the getter transfers a new view.
    unsafe {
        let view = imre_exception_args(owner);
        if view.is_null() {
            return Err(std::fmt::Error);
        }
        let view = crate::ObjectOwner::from_raw(view);
        // End the storage borrow before a member's repr can run.
        let tuple = view.0.with(|s| match s {
            Storage::ObjectValue(tuple) => imports::Positional(tuple.clone()),
            _ => unreachable!("argument tuple view"),
        });
        tuple.write_repr_args(output)
    }
}

/// Compare the underlying tuple owners, including CPython's singleton empty tuple.
///
/// # Safety
/// Both object views are rooted and at least one is known to hold an actual tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_tuple_identity(
    left: *const Object,
    right: *const Object,
) -> u32 {
    fn tuple(view: *const Object) -> Option<(usize, bool)> {
        // SAFETY: the caller roots the view and its original record through this comparison.
        unsafe {
            if super::super::errno::with_nonscalar_member(object(view), |_| ()).is_some() {
                return None;
            }
            let (kind, word) = exit_word(view);
            if kind.raw() != 6 {
                return None;
            }
            let layout = records::descriptor(word as *const usize);
            let meta = layout.semantics.as_ref()?;
            if meta.kind != 1 {
                return None;
            }
            Some((word, layout.length == 0 && meta.name_len == 0))
        }
    }
    match (tuple(left), tuple(right)) {
        (Some((a, empty_a)), Some((b, empty_b))) => u32::from(a == b || (empty_a && empty_b)),
        _ => 0,
    }
}

/// SystemExit(*args) stores the same tuple as its code when there is more than one argument.
///
/// # Safety
/// The argument is a rooted, exact immutable tuple with at least two items.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_from_tuple(args: *const usize) -> *const Object {
    // SAFETY: the exception owns its original argument vector as its exit code.
    let tuple = unsafe { WordKind::new(6).acquire(args as usize) };
    crate::ObjectOwner::into_raw(new_exception(
        Kind::SystemExit,
        Payload::ExitArguments(imports::Positional(tuple)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(exception: &crate::ObjectOwner) -> crate::ObjectOwner {
        // SAFETY: the exception is rooted; the accessor transfers one view.
        unsafe {
            crate::ObjectOwner::from_raw(imre_exception_args(crate::ObjectOwner::as_ptr(exception)))
        }
    }

    #[test]
    fn repeated_arguments_keep_identity_and_the_empty_tuple_is_shared() {
        let first = new_exception(Kind::ValueError, Payload::Empty);
        let second = new_exception(Kind::TypeError, Payload::Empty);
        assert!(crate::ObjectOwner::ptr_eq(&args(&first), &args(&second)));
        let text = StrOwner::new(Str::from("keep me"));
        let weak = StrOwner::downgrade(&text);
        let error = new_exception(Kind::ValueError, Payload::Message(text));
        let saved = args(&error);
        assert!(crate::ObjectOwner::ptr_eq(&saved, &args(&error)));
        drop(error);
        assert!(weak.upgrade().is_some());
        drop(saved);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn retained_group_items_survive_the_tuple_without_a_parent_child_cycle() {
        let child = new_exception(Kind::ValueError, Payload::Empty);
        let group = task_group_errors(vec![child.clone()]);
        let weak_child = crate::ObjectOwner::downgrade(&child);
        let weak_group = crate::ObjectOwner::downgrade(&group);
        let saved = args(&group);
        let Cached::Group { items, .. } = Cached::caused_group(&saved) else {
            unreachable!()
        };
        let list = items.upgrade().unwrap();
        // SAFETY: this is the actual child of the still-rooted parent.
        let raised = unsafe {
            crate::ObjectOwner::from_raw(imre_async_member_cause(
                crate::ObjectOwner::as_ptr(&child),
                crate::ObjectOwner::as_ptr(&group),
            ))
        };
        assert!(crate::ObjectOwner::ptr_eq(&saved, &args(&group)));
        drop(saved);
        let again = args(&group);
        let Cached::Group { items, .. } = Cached::caused_group(&again) else {
            unreachable!()
        };
        assert!(crate::ObjectOwner::ptr_eq(&list, &items.upgrade().unwrap()));
        drop((again, group, raised, child));
        assert!(weak_group.upgrade().is_some());
        drop(list);
        assert!(weak_group.upgrade().is_none() && weak_child.upgrade().is_none());
    }
}
