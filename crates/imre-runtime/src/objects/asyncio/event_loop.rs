//! Each loop owns its pending fibers and native cancellation handlers. Signal
//! handlers set lock-free bits; all task state is touched by the owner thread.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use super::*;

static SIGNALS: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_signal(signal: libc::c_int) {
    SIGNALS.fetch_or(1u32 << signal, Ordering::Relaxed);
}

struct Registration {
    prior: libc::sigaction,
    task: Rc<Task>,
}

#[derive(Default)]
pub(in crate::objects) struct EventLoop {
    pub(super) queue: RefCell<Vec<Rc<Task>>>,
    pub(super) failures: RefCell<Vec<Weak<Group>>>,
    closed: Cell<bool>,
    handlers: RefCell<BTreeMap<i32, Registration>>,
}

impl EventLoop {
    pub(super) fn dispatch_signals(&self) {
        let signals = SIGNALS.swap(0, Ordering::Relaxed);
        if signals != 0 {
            for (&number, handler) in self.handlers.borrow().iter() {
                if signals & (1u32 << number) != 0 {
                    handler.task.cancelled.set(true);
                }
            }
        }
    }

    pub(super) fn park(&self) {
        if self.handlers.borrow().is_empty() {
            std::thread::park();
        } else {
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }

    fn restore(&self) {
        for (signal, handler) in self.handlers.take() {
            // SAFETY: each saved action came from sigaction for this signal.
            unsafe { libc::sigaction(signal, &handler.prior, std::ptr::null_mut()) };
            SIGNALS.fetch_and(!(1u32 << signal), Ordering::Relaxed);
        }
    }

    fn register(&self, signal: i32, task: Rc<Task>) -> crate::Result<()> {
        if !matches!(signal, libc::SIGINT | libc::SIGTERM) {
            return Err(Error::new(
                Kind::ValueError,
                "unsupported cancellation signal",
            ));
        }
        if let Some(handler) = self.handlers.borrow_mut().get_mut(&signal) {
            handler.task = task;
            return Ok(());
        }
        // SAFETY: initialize the POSIX signal mask and install a stateless
        // async-signal-safe handler. No task pointers enter the signal handler.
        let prior = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut prior = std::mem::MaybeUninit::uninit();
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_sigaction = on_signal as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            if libc::sigaction(signal, &action, prior.as_mut_ptr()) == -1 {
                return Err(Error::new(
                    Kind::OSError,
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            prior.assume_init()
        };
        self.handlers
            .borrow_mut()
            .insert(signal, Registration { prior, task });
        Ok(())
    }
}

impl Drop for EventLoop {
    fn drop(&mut self) {
        self.restore();
    }
}

thread_local! {
    static SELECTED: RefCell<Option<Rc<EventLoop>>> = const { RefCell::new(None) };
}

pub(super) fn selected() -> Rc<EventLoop> {
    SELECTED.with_borrow_mut(|current| {
        current
            .get_or_insert_with(|| Rc::new(EventLoop::default()))
            .clone()
    })
}

pub(super) fn enter(loop_: Rc<EventLoop>) -> Option<Rc<EventLoop>> {
    SELECTED.replace(Some(loop_))
}

pub(super) fn restore(prior: Option<Rc<EventLoop>>) {
    SELECTED.replace(prior);
}

fn loop_of(handle: &Object) -> Rc<EventLoop> {
    // SAFETY: a sealed loop object contains only the owning-thread handle.
    unsafe {
        handle.0.with(|storage| match storage {
            Storage::EventLoop(value) => value.clone(),
            _ => fail("SystemError", "expected an asyncio event loop"),
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_async_loop_new() -> *const Object {
    resource(Storage::EventLoop(Rc::new(EventLoop::default())))
}

/// # Safety
/// The compiler roots this exact event-loop handle for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_loop_set(handle: *const Object) {
    // SAFETY: only an owned loop handle escapes the storage borrow.
    let loop_ = loop_of(unsafe { object(handle) });
    SELECTED.replace(Some(loop_));
}

/// # Safety
/// The statically typed awaitable stays rooted during scheduling.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_ensure(handle: *const Object) -> *const Object {
    // SAFETY: the caller roots a typed coroutine or task.
    let task = for_scheduling(unsafe { object(handle) });
    if selected().closed.get() {
        return Error::new(Kind::RuntimeError, "Event loop is closed").returning(std::ptr::null());
    }
    task.schedule();
    resource(Storage::AsyncTask(task))
}

/// # Safety
/// Output uses the result kind of this typed awaitable. Both handles stay rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_loop_run(
    handle: *const Object,
    future: *const Object,
) -> usize {
    // SAFETY: extract owned handles before suspending any generated code.
    let (loop_, task) = unsafe { (loop_of(object(handle)), for_scheduling(object(future))) };
    if loop_.closed.get() {
        return Error::new(Kind::RuntimeError, "Event loop is closed").returning(0);
    }
    if in_task() {
        return Error::new(Kind::RuntimeError, "This event loop is already running").returning(0);
    }
    if let Some(owner) = task.event_loop.borrow().upgrade()
        && !Rc::ptr_eq(&owner, &loop_)
    {
        return Error::new(Kind::ValueError, "The future belongs to a different loop").returning(0);
    }
    let prior = enter(loop_.clone());
    task.schedule();
    drive(&loop_, Some(&task), false);
    restore(prior);
    task.result()
}

/// # Safety
/// Called by the owner thread with the exact loop and task produced by asyncio.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_loop_signal(
    handle: *const Object,
    number: usize,
    future: *const Object,
) {
    // SAFETY: the borrowed canonical signal integer and handles remain rooted.
    let (loop_, number, task) = unsafe {
        (
            loop_of(object(handle)),
            Int::borrow(number).as_small(),
            task_of(object(future)),
        )
    };
    finish(
        loop_.register(
            number.and_then(|n| i32::try_from(n).ok()).unwrap_or(-1),
            task,
        ),
        (),
    );
}

/// # Safety
/// Closing this exact loop releases its registered signal handlers and tasks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_loop_close(handle: *const Object) {
    // SAFETY: no storage borrow survives the cancellation cleanup.
    let loop_ = loop_of(unsafe { object(handle) });
    if in_task() {
        Error::new(Kind::RuntimeError, "Cannot close a running event loop").raise();
        return;
    }
    if loop_.closed.replace(true) {
        return;
    }
    let prior = enter(loop_.clone());
    drive(&loop_, None, true);
    restore(prior);
    loop_.restore();
}
