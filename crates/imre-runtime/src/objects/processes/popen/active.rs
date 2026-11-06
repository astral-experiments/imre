//! Retain the original language object until a later launch reaps its child.

use super::*;
use crate::objects::words::Words;

thread_local! {
    static ACTIVE: crate::ObjectOwner = Words::new(5).into_object();
    static INTERNAL: RefCell<Vec<(Arc<ChildControl>, Pipes)>> = const { RefCell::new(Vec::new()) };
}

/// Called before releasing the last language owner, including a sequence or
/// record field. Resurrection must share the original object, control and pipes.
pub(in crate::objects) fn retain_live_owner(owner: &crate::ObjectOwner) {
    // SAFETY: the caller holds this object's only reference on its owner thread.
    // Finalization reads status, writes a warning and polls; none calls Python.
    let keep = unsafe {
        owner.0.with_mut(|storage| match storage {
            Storage::Popen(process) => {
                process.begin_finalization() && process.control.poll_for_cleanup().is_none()
            }
            _ => false,
        })
    };
    if keep {
        let _ = ACTIVE.try_with(|active| {
            // SAFETY: no other sequence access or language callback spans this mutation.
            unsafe {
                active.0.with_mut(|storage| {
                    let entries = storage.concrete_sequence_mut();
                    entries
                        .values
                        .push(crate::ObjectOwner::into_raw(owner.clone()) as usize);
                    entries.update_header();
                });
                // _active now owns the finalized object. Dropping it later releases its pipes.
                with_process(crate::ObjectOwner::as_ptr(owner), |process| {
                    process.drop_action = DropAction::Release;
                });
            }
        });
    }
}

/// A synchronous helper can fail before it has exposed a Popen object.
pub(super) fn defer_internal(control: Arc<ChildControl>, pipes: Pipes) {
    let _ = INTERNAL.try_with(|children| children.borrow_mut().push((control, pipes)));
}

/// Poll every retained child without blocking. Displaced owners are released
/// only after the list borrow ends; independently retained pipe aliases survive.
pub(in crate::objects::processes) fn cleanup() {
    ACTIVE.with(|active| {
        let mut reaped = Vec::new();
        // SAFETY: the list owns each Popen on this thread. Polling ChildControl
        // cannot run user code or mutate the active list.
        unsafe {
            active.0.with_mut(|storage| {
                let entries = storage.concrete_sequence_mut();
                entries.values.retain(|word| {
                    let done = object(*word as *const Object)
                        .0
                        .with(|storage| match storage {
                            Storage::Popen(process) => process.control.poll_for_cleanup().is_some(),
                            _ => false,
                        });
                    if done {
                        reaped.push(*word);
                    }
                    !done
                });
                entries.update_header();
            });
            for word in reaped {
                // Each removed list entry transferred its language-level owner.
                WordKind::new(5).release(word);
            }
        }
    });
    INTERNAL.with_borrow_mut(|children| {
        children.retain(|(control, _)| !matches!(control.poll(), Ok(Some(_))));
    });
}

/// The caller receives an owner of the same list on every module attribute read.
#[unsafe(no_mangle)]
pub extern "C" fn imre_popen_active() -> *const Object {
    ACTIVE.with(|active| crate::ObjectOwner::into_raw(active.clone()))
}

/// Apply the same nonblocking cleanup used by Popen's constructor.
#[unsafe(no_mangle)]
pub extern "C" fn imre_popen_cleanup() {
    cleanup();
}

/// The caller roots a Popen, whose native allocation keeps a stable address.
#[unsafe(no_mangle)]
pub extern "C" fn imre_popen_identity(handle: *const Object) -> usize {
    Int::from_usize(handle as usize).into_raw()
}

#[test]
fn releasing_a_finalized_owner_closes_stdin_and_leaves_the_exit_for_waitpid() {
    use crate::objects::processes::tests::args;

    let argv = args(&[
        "python3",
        "-c",
        "import os, select; raise SystemExit(37 if select.select([0], [], [], 5)[0] and not os.read(0, 1) else 1)",
    ]);
    let config = LaunchConfig::basic(
        crate::ObjectOwner::as_ptr(&argv).cast(),
        5,
        std::ptr::null(),
        std::ptr::null(),
        1 << 12,
        Int::small(-1).unwrap().0,
    );
    // SAFETY: argv stays rooted, and the configuration requests a stdin pipe.
    let raw = unsafe { imre_popen_config(&config) };
    assert!(!raw.is_null());
    // SAFETY: the constructor returned a new Popen reference.
    let owner = unsafe { crate::ObjectOwner::from_raw(raw) };
    // SAFETY: owner keeps the Popen alive while we acquire its shared child control.
    let control = unsafe { with_process(raw, |process| process.control.clone()) };
    retain_live_owner(&owner);
    // An inconclusive cleanup removes the owner, but the caller can clear its cached status.
    control.set_status(Some(ReturnCode::DeadState));
    cleanup();
    control.set_status(None);
    crate::objects::release_object_owner(owner);
    cleanup();

    let mut status = 0;
    // SAFETY: status is writable and this process is the child's parent.
    let pid = unsafe { libc::waitpid(control.pid, &mut status, 0) };
    assert_eq!(pid, control.pid);
    assert!(libc::WIFEXITED(status));
    assert_eq!(libc::WEXITSTATUS(status), 37);
}
