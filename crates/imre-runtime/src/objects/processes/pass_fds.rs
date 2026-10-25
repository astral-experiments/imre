//! Copy caller-supplied descriptors before fork; change flags only in the child.

use super::*;
use crate::objects::records::descriptor;

fn push(output: &mut Vec<RawFd>, number: &Int) -> Result<()> {
    let fd = number
        .as_small()
        .and_then(|n| i32::try_from(n).ok())
        .filter(|fd| *fd >= 0)
        .ok_or_else(|| Error::new(Kind::ValueError, "bad value(s) in fds_to_keep"))?;
    output.try_reserve(1).map_err(|_| Error::allocation())?;
    output.push(fd);
    Ok(())
}

/// Test the keep collection before converting its elements or inspecting redirected files.
///
/// # Safety
/// The caller roots the collection and provides its exact storage kind.
pub(super) unsafe fn nonempty(word: usize, kind: u32) -> bool {
    // SAFETY: each branch borrows the storage identified by kind.
    unsafe {
        match kind {
            0 => false,
            4 => (&*(word as *const crate::List))
                .0
                .with(|values| !values.is_empty()),
            5 => object(word as *const Object).0.with(|s| match s {
                Storage::Sequence(list) => !list.values.is_empty(),
                Storage::Set(set) => set.integers().next().is_some(),
                _ => fail("SystemError", "unexpected pass_fds collection"),
            }),
            6 => descriptor(word as *const usize).length != 0,
            _ => fail("SystemError", "unexpected pass_fds representation"),
        }
    }
}

/// # Safety
/// The native list, set, tuple or None and its elements must be rooted.
pub(super) unsafe fn copy(word: usize, kind: u32) -> Result<Vec<RawFd>> {
    unsafe fn scalar(fds: &mut Vec<RawFd>, word: usize, kind: WordKind) -> Result<()> {
        if kind.raw() == 1 {
            push(fds, &Int::small(i64::from(word != 0)).unwrap())
        } else {
            // SAFETY: the compiler only admits integers and bools.
            let number = unsafe { Int::borrow(word) };
            push(fds, &number)
        }
    }
    let mut fds = Vec::new();
    // SAFETY: each collection remains owned and immutable during conversion.
    unsafe {
        match kind {
            0 => {}
            4 => (&*(word as *const crate::List))
                .0
                .with(|values| values.iter().try_for_each(|number| push(&mut fds, number)))?,
            5 => object(word as *const Object).0.with(|s| match s {
                Storage::Sequence(list) => list
                    .values
                    .iter()
                    .try_for_each(|&n| scalar(&mut fds, n, list.kind)),
                Storage::Set(set) => set.integers().try_for_each(|n| push(&mut fds, n)),
                _ => fail("SystemError", "unexpected pass_fds collection"),
            })?,
            6 => {
                let tuple = word as *const usize;
                let layout = descriptor(tuple);
                for i in 0..layout.length {
                    let kind = WordKind::new(*layout.kinds.add(i));
                    scalar(&mut fds, kind.read_record_field(tuple.add(i)), kind)?;
                }
            }
            _ => fail("SystemError", "unexpected pass_fds representation"),
        }
    }
    fds.sort_unstable();
    fds.dedup();
    Ok(fds)
}

/// # Safety
/// Call in the forked child after stdio and the bulk CLOEXEC operation.
pub(super) unsafe fn inherit(fds: &[RawFd]) -> io::Result<()> {
    for &fd in fds {
        // SAFETY: the forked child owns its table, and fcntl is async-signal-safe.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0
                || (flags & libc::FD_CLOEXEC != 0
                    && libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0)
            {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}
