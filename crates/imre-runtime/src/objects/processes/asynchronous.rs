//! Async subprocess handles live on the language thread. Only the direct child
//! and owned native pipe buffers cross into its reaping worker.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use super::*;
use crate::StorageCell;

type Captured = (ExitStatus, Vec<u8>, Vec<u8>);

fn ready(handle: &Object) -> bool {
    // SAFETY: polling the native channel does not execute language callbacks.
    unsafe {
        handle.0.with_mut(|storage| {
            let Storage::AsyncProcess(process) = storage else {
                fail("SystemError", "expected an async process");
            };
            !matches!(process.completed(false), Ok(None))
        })
    }
}

/// Drop every runtime-object borrow before another language task can run.
unsafe fn wait_for_process(handle: *const Object) -> bool {
    use crate::objects::asyncio;
    if !asyncio::in_task() {
        return true;
    }
    // SAFETY: the compiler roots this exact process through the await.
    let owner: crate::ObjectOwner = unsafe { crate::ObjectOwner::acquire_word(handle as usize) };
    ready(&owner) || asyncio::suspend(Rc::new(move || ready(&owner))).is_none()
}

pub(in crate::objects) struct AsyncProcess {
    receiver: Receiver<io::Result<Captured>>,
    output: Option<io::Result<Captured>>,
    pipes: [bool; 2],
}

impl AsyncProcess {
    fn spawn(launch: Launch, cwd: Option<&CString>, stdout: u32, stderr: u32) -> Result<Self> {
        if stdout > 2 || stderr > 2 {
            fail("SystemError", "unknown async subprocess output mode");
        }
        let child = launch
            .spawn(cwd, stdout | (stderr << 4))
            .map_err(os_error)?;
        let guard = ChildGuard {
            child,
            reaped: false,
        };
        let (sender, receiver) = mpsc::channel();
        let event_thread = thread::current();
        thread::Builder::new()
            .name("imre-subprocess".into())
            .spawn(move || {
                let _ = sender.send(capture_guard(guard));
                event_thread.unpark();
            })
            .map_err(os_error)?;
        Ok(Self {
            receiver,
            output: None,
            pipes: [stdout == 1, stderr == 1],
        })
    }

    /// Polling leaves the handle and buffered output reusable. A wait consumes
    /// only the child status and a communicate consumes each requested pipe.
    fn completed(&mut self, wait: bool) -> Result<Option<&mut Captured>> {
        if self.output.is_none() {
            let stopped = || Error::new(Kind::RuntimeError, "async subprocess worker stopped");
            self.output = Some(if wait {
                self.receiver.recv().map_err(|_| stopped())?
            } else {
                match self.receiver.try_recv() {
                    Ok(output) => output,
                    Err(TryRecvError::Empty) => return Ok(None),
                    Err(TryRecvError::Disconnected) => return Err(stopped()),
                }
            });
        }
        match self.output.as_mut().expect("worker output was received") {
            Ok(output) => Ok(Some(output)),
            Err(error) if capture_allocation_failed(error) => Err(Error::allocation()),
            Err(error) => Err(os_error(match error.raw_os_error() {
                Some(code) => io::Error::from_raw_os_error(code),
                None => io::Error::new(error.kind(), error.to_string()),
            })),
        }
    }
}

/// # Safety
/// The exact string argv list, nullable cwd string and nullable string
/// environment remain rooted during argument conversion and child creation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_process_spawn(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    stdout: u32,
    stderr: u32,
) -> *const Object {
    // SAFETY: the exact argv stays rooted during this arity check.
    let empty = unsafe {
        object(args)
            .0
            .with(|s| s.sequence().map(|words| words.values.is_empty()))
    };
    if match empty {
        Ok(empty) => empty,
        Err(error) => return error.returning(std::ptr::null()),
    } {
        return Error::new(
            Kind::TypeError,
            "create_subprocess_exec() missing 1 required positional argument: 'program'",
        )
        .returning(std::ptr::null());
    }
    // SAFETY: these arguments have the same rooting and types as subprocess.run.
    let result = unsafe { prepare_process(args, cwd, env) }
        .and_then(|(launch, cwd)| AsyncProcess::spawn(launch, cwd.as_ref(), stdout, stderr))
        .map(|process| {
            crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                Storage::AsyncProcess(Box::new(process)),
            ))))
        });
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots this exact async process throughout the status operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_process_status(handle: *const Object, wait: bool) -> usize {
    // SAFETY: the rooted handle stays alive during the cooperative wait.
    if wait && !unsafe { wait_for_process(handle) } {
        return 0;
    }
    // SAFETY: the native worker never accesses this language-owned handle.
    let result = unsafe {
        object(handle).0.with_mut(|storage| {
            let Storage::AsyncProcess(process) = storage else {
                fail("SystemError", "expected an async process");
            };
            process.completed(wait).map(|output| {
                output.map_or(0, |(status, _, _)| {
                    exit_word(
                        status
                            .code()
                            .unwrap_or_else(|| -status.signal().unwrap_or(0)),
                    )
                })
            })
        })
    };
    finish(result, 0)
}

/// # Safety
/// The handle stays rooted, and the descriptor describes a pair whose piped
/// slots own bytes and whose uncaptured slots are None. The result transfers
/// both buffers into the new record on the language thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_process_communicate(
    handle: *const Object,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the rooted handle stays alive during the cooperative wait.
    if !unsafe { wait_for_process(handle) } {
        return std::ptr::null_mut();
    }
    // SAFETY: no other language operation reenters this borrow; child IO owns
    // native data and the static descriptor describes these exact two slots.
    let result = unsafe {
        object(handle).0.with_mut(|storage| -> Result<_> {
            let Storage::AsyncProcess(process) = storage else {
                fail("SystemError", "expected an async process");
            };
            let [has_stdout, has_stderr] = process.pipes;
            let (_, stdout, stderr) = process.completed(true)?.expect("blocking wait finished");
            let tuple = imre_record_new(descriptor);
            for (slot, (captured, bytes)) in [(has_stdout, stdout), (has_stderr, stderr)]
                .into_iter()
                .enumerate()
            {
                let owner: *const Bytes = if captured {
                    crate::BytesOwner::into_raw(crate::BytesOwner::new(std::mem::take(bytes)))
                } else {
                    std::ptr::null()
                };
                tuple.add(slot).cast::<*const Bytes>().write(owner);
            }
            Ok(tuple)
        })
    };
    finish(result, std::ptr::null_mut())
}
