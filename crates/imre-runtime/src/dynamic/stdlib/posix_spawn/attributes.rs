use std::io;

use crate::dynamic::stdlib::{fixed_structseq, os, os_error};
use crate::dynamic::{Context, Error, Id, NONE, Result, Value};
use crate::objects::exceptions::Kind;

/// Own initialized attributes at a stable address through configuration, spawn, and destruction.
pub(super) struct Attributes(Box<libc::posix_spawnattr_t>);

impl Attributes {
    pub(super) fn new(
        ctx: &mut Context,
        setpgroup: Id,
        resetids: bool,
        setsid: bool,
        setsigmask: Option<Id>,
        setsigdef: Option<Id>,
        scheduler: Id,
    ) -> Result<Self> {
        let mut storage = Box::new_zeroed();
        // SAFETY: init receives writable aligned storage, owned here through the call.
        check(unsafe { libc::posix_spawnattr_init(storage.as_mut_ptr()) })?;
        // SAFETY: successful init initialized the attributes. Drop now handles all exit paths.
        let mut attributes = Self(unsafe { storage.assume_init() });
        let attr = &mut *attributes.0;
        let mut flags = 0;
        if setpgroup != NONE {
            let group = os::c_int(ctx, setpgroup)?;
            // SAFETY: attr refers to initialized exclusively owned attributes.
            check(unsafe { libc::posix_spawnattr_setpgroup(attr, group) })?;
            flags |= libc::POSIX_SPAWN_SETPGROUP;
        }
        if resetids {
            flags |= libc::POSIX_SPAWN_RESETIDS;
        }
        if setsid {
            flags |= libc::POSIX_SPAWN_SETSID;
        }
        if let Some(value) = setsigmask {
            let mask = signal_set(ctx, value)?;
            // SAFETY: the setter copies the initialized signal set into attr.
            check(unsafe { libc::posix_spawnattr_setsigmask(attr, &mask) })?;
            flags |= libc::POSIX_SPAWN_SETSIGMASK;
        }
        if let Some(value) = setsigdef {
            let defaults = signal_set(ctx, value)?;
            // SAFETY: the setter copies the initialized signal set into attr.
            check(unsafe { libc::posix_spawnattr_setsigdefault(attr, &defaults) })?;
            flags |= libc::POSIX_SPAWN_SETSIGDEF;
        }
        if scheduler != NONE {
            let items = ctx.tuple_items(scheduler)?.unwrap_or_default();
            let [policy, param] = items.as_slice() else {
                return Err(Error::new(
                    "TypeError",
                    "A scheduler tuple must have two elements",
                ));
            };
            let Value::NamedTuple {
                class: Some(fixed_structseq::SCHED_PARAM),
                values,
                ..
            } = ctx.heap.get(*param)
            else {
                return Err(Error::new("TypeError", "must have a sched_param object"));
            };
            let priority = values[0];
            let priority = ctx.index_c_long(priority)?;
            // SAFETY: Linux sched_param consists of integer fields; leave unused fields zero.
            let mut param = unsafe { std::mem::zeroed::<libc::sched_param>() };
            param.sched_priority = i32::try_from(priority)
                .map_err(|_| Error::new("OverflowError", "sched_priority out of range"))?;
            if *policy != NONE {
                let policy = os::c_int(ctx, *policy)?;
                // SAFETY: attr is initialized and libc validates policy.
                check(unsafe { libc::posix_spawnattr_setschedpolicy(attr, policy) })?;
                flags |= libc::POSIX_SPAWN_SETSCHEDULER;
            }
            // SAFETY: param is fully initialized and copied into attr.
            check(unsafe { libc::posix_spawnattr_setschedparam(attr, &param) })?;
            flags |= libc::POSIX_SPAWN_SETSCHEDPARAM;
        }
        // SAFETY: attr is initialized and all flags are system spawn flags.
        check(unsafe { libc::posix_spawnattr_setflags(attr, flags) })?;
        Ok(attributes)
    }

    pub(super) fn as_raw(&self) -> &libc::posix_spawnattr_t {
        &self.0
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: this sole owner destroys exactly once at the initialization address.
        unsafe { libc::posix_spawnattr_destroy(&mut *self.0) };
    }
}

/// POSIX spawn attribute routines return the error number directly.
fn check(code: i32) -> Result<()> {
    if code != 0 {
        return Err(os_error(io::Error::from_raw_os_error(code)));
    }
    Ok(())
}

fn signal_set(ctx: &mut Context, value: Id) -> Result<libc::sigset_t> {
    // SAFETY: Linux sigset_t contains only integer words; zero even the bits libc reserves.
    let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    // SAFETY: mask is writable caller-owned storage.
    if unsafe { libc::sigemptyset(&mut mask) } < 0 {
        return Err(os_error(io::Error::last_os_error()));
    }
    let iterator = ctx.iterator(value)?;
    ctx.heap.keep(iterator);
    let step = ctx.heap.temporary_mark();
    let nsig = crate::signals::nsig();
    while let Some(item) = ctx.next_item(iterator)? {
        // PyLong_AsLongAndOverflow returns -1 for an integer outside the C long range.
        let number = ctx.index_integer(item)?.to_i64().unwrap_or(-1);
        ctx.discard_truth(step);
        if !(1..i64::from(nsig)).contains(&number) {
            return Err(Error::new(
                "ValueError",
                format!("signal number {number} out of range [1; {}]", nsig - 1),
            ));
        }
        // SAFETY: mask is initialized, and number is in the host's signal range.
        if unsafe { libc::sigaddset(&mut mask, number as i32) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINVAL) {
                return Err(os_error(error));
            }
            ctx.emit_builtin_warning(
                Kind::RuntimeWarning,
                &format!("invalid signal number {number}, please use valid_signals()"),
                1,
            )?;
        }
    }
    Ok(mask)
}
