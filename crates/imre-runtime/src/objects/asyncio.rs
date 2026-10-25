//! Cooperative tasks run generated code on its owning thread. Native subprocess
//! workers wake that thread when captured output is ready.

use crate::ObjectOwner;
mod event_loop;
pub(super) use event_loop::EventLoop;

use std::cell::{Cell, RefCell};
use std::mem;
use std::rc::Weak;

use corosensei::{Coroutine, CoroutineResult, Yielder};

use super::exceptions::{replace_pending, swap_active, take_pending};
use super::records::imre_record_retain;
use super::words::{OwnedWord, WordKind};
use super::*;

type Ready = Box<dyn Fn() -> bool>;
type Fiber = Coroutine<(), Ready, Outcome>;
type Callback = unsafe extern "C" fn(*const usize) -> usize;
type Outcome = std::result::Result<OwnedWord, ObjectOwner>;

struct Call {
    callback: Callback,
    frame: OwnedWord,
    kind: WordKind,
}

impl Call {
    fn run(self) -> Outcome {
        // SAFETY: the compiler paired this thunk with its exact argument record;
        // the frame roots its borrowed parameters through the whole native call.
        let word = unsafe { (self.callback)(self.frame.word as *const usize) };
        match take_pending() {
            Some(error) => Err(error),
            // SAFETY: successful generated calls transfer an exact owned result.
            None => Ok(unsafe { OwnedWord::from_owned(word, self.kind) }),
        }
    }
}

pub(super) struct Task {
    call: RefCell<Option<Call>>,
    fiber: RefCell<Option<Fiber>>,
    ready: RefCell<Option<Ready>>,
    active: RefCell<Option<ObjectOwner>>,
    outcome: RefCell<Option<Outcome>>,
    scheduled: Cell<bool>,
    cancelled: Cell<bool>,
    finished: Cell<u64>,
    shutdown: Cell<bool>,
    event_loop: RefCell<Weak<EventLoop>>,
    group: RefCell<Weak<Group>>,
}

#[derive(Default)]
pub(super) struct Group {
    children: RefCell<Vec<Rc<Task>>>,
    error: RefCell<Option<ObjectOwner>>,
    parent: RefCell<Weak<Task>>,
    state: Cell<GroupState>,
    aborting: Cell<bool>,
}

#[derive(Default, Copy, Clone, Eq, PartialEq)]
enum GroupState {
    #[default]
    New,
    Entered,
    Joining,
    Finished,
}

impl Group {
    fn abort(&self) {
        if self.aborting.replace(true) {
            return;
        }
        for task in self.children.borrow().iter() {
            if !task.done() {
                task.cancelled.set(true);
            }
        }
        if self.state.get() == GroupState::Entered
            && let Some(parent) = self.parent.borrow().upgrade()
        {
            parent.cancelled.set(true);
        }
    }
}

impl Task {
    fn new(call: Call) -> Rc<Self> {
        Rc::new(Self {
            call: RefCell::new(Some(call)),
            fiber: RefCell::new(None),
            ready: RefCell::new(None),
            active: RefCell::new(None),
            outcome: RefCell::new(None),
            scheduled: Cell::new(false),
            cancelled: Cell::new(false),
            finished: Cell::new(0),
            shutdown: Cell::new(false),
            event_loop: RefCell::default(),
            group: RefCell::default(),
        })
    }

    fn schedule(self: &Rc<Self>) {
        if self.scheduled.replace(true) {
            return;
        }
        let call = self.call.take().expect("a new task owns its call");
        *self.fiber.borrow_mut() = Some(Coroutine::new(move |yielder, ()| {
            CURRENT.set(yielder);
            let result = if CANCELLED.replace(false) {
                cancelled();
                Err(take_pending().expect("cancellation set an exception"))
            } else {
                call.run()
            };
            CURRENT.set(std::ptr::null());
            result
        }));
        let loop_ = event_loop::selected();
        *self.event_loop.borrow_mut() = Rc::downgrade(&loop_);
        loop_.queue.borrow_mut().push(self.clone());
    }

    fn done(&self) -> bool {
        self.outcome.borrow().is_some()
    }

    fn resume(self: &Rc<Self>) -> bool {
        if !self.cancelled.get() && self.ready.borrow().as_ref().is_some_and(|ready| !ready()) {
            return false;
        }
        self.ready.take();
        swap_active(&mut self.active.borrow_mut());
        CANCELLED.set(self.cancelled.replace(false));
        CURRENT_TASK.replace(Rc::downgrade(self));
        let step = self
            .fiber
            .borrow_mut()
            .as_mut()
            .expect("scheduled fiber")
            .resume(());
        CURRENT_TASK.take();
        swap_active(&mut self.active.borrow_mut());
        match step {
            CoroutineResult::Yield(ready) => {
                *self.ready.borrow_mut() = Some(ready);
            }
            CoroutineResult::Return(outcome) => {
                self.finished.set(FINISH_ORDER.get());
                FINISH_ORDER.set(FINISH_ORDER.get() + 1);
                if let Err(error) = &outcome
                    && super::exceptions::kind_of(error) != Kind::CancelledError
                    && let Some(group) = self.group.borrow().upgrade()
                    && let Some(loop_) = self.event_loop.borrow().upgrade()
                {
                    loop_.failures.borrow_mut().push(Rc::downgrade(&group));
                }
                *self.outcome.borrow_mut() = Some(outcome);
                self.fiber.take();
            }
        }
        true
    }

    fn result(&self) -> usize {
        match self.outcome.borrow().as_ref() {
            Some(Ok(result)) => {
                // SAFETY: the completed task keeps this exact return value rooted.
                unsafe { result.kind.acquire(result.word) }.into_word()
            }
            Some(Err(error)) => {
                replace_pending(Some(error.clone()));
                0
            }
            None => Error::new(Kind::RuntimeError, "Result is not set.").returning(0),
        }
    }
}

thread_local! {
    static CURRENT_TASK: RefCell<Weak<Task>> = const { RefCell::new(Weak::new()) };
    static FINISH_ORDER: Cell<u64> = const { Cell::new(0) };
    static CURRENT: Cell<*const Yielder<(), Ready>> = const { Cell::new(std::ptr::null()) };
    static CANCELLED: Cell<bool> = const { Cell::new(false) };
}

fn cancelled() {
    Error::new(Kind::CancelledError, "").raise();
}

pub(super) fn in_task() -> bool {
    !CURRENT.get().is_null()
}

/// The yielder stays on the fiber's native stack for its entire lifetime.
pub(super) fn suspend(ready: Ready) -> bool {
    let yielder = CURRENT.replace(std::ptr::null());
    assert!(!yielder.is_null(), "suspension outside an asyncio task");
    // SAFETY: only the currently running fiber can reach this TLS pointer. It
    // is cleared during suspension so a sibling cannot observe another stack.
    unsafe { &*yielder }.suspend(ready);
    CURRENT.set(yielder);
    if CANCELLED.replace(false) {
        cancelled();
        false
    } else {
        true
    }
}

fn drive(loop_: &Rc<EventLoop>, root: Option<&Rc<Task>>, shutdown: bool) {
    let mut waiting = Vec::new();
    loop {
        let mut work = mem::take(&mut waiting);
        work.append(&mut loop_.queue.borrow_mut());
        if work.is_empty() {
            break;
        }
        let finished = root.is_none_or(|task| task.done());
        if finished && !shutdown {
            *loop_.queue.borrow_mut() = work;
            break;
        }
        loop_.dispatch_signals();
        // Task completion callbacks run after the current batch. In particular,
        // two ready children may both fail before their group cancels peers.
        for group in loop_
            .failures
            .take()
            .into_iter()
            .filter_map(|group| group.upgrade())
        {
            group.abort();
        }
        let mut progress = false;
        for task in work {
            if finished && !task.shutdown.replace(true) {
                task.cancelled.set(true);
            }
            progress |= task.resume();
            if !task.done() {
                waiting.push(task);
            }
        }
        if !progress && loop_.queue.borrow().is_empty() {
            loop_.park();
        }
    }
}

fn await_task(task: &Rc<Task>) -> usize {
    if !task.scheduled.get() {
        // Directly awaiting a cold coroutine first enters its body on this
        // task, before queued peers run, just as Python's immediate await does.
        task.scheduled.set(true);
        let call = task.call.take().expect("cold coroutine owns its call");
        let outcome = call.run();
        *task.outcome.borrow_mut() = Some(outcome);
    } else if !task.done() {
        if CURRENT_TASK.with_borrow(|current| {
            current
                .upgrade()
                .is_some_and(|owner| Rc::ptr_eq(&owner, task))
        }) {
            return Error::new(Kind::RuntimeError, "Task cannot await on itself").returning(0);
        }
        while !task.done() {
            let owner = task.clone();
            if !suspend(Box::new(move || owner.done())) {
                take_pending();
                task.cancelled.set(true);
            }
        }
    }
    task.result()
}

fn task_of(object: &Object) -> Rc<Task> {
    // SAFETY: cloning a language-thread owner never reenters object storage.
    unsafe {
        object.0.with(|storage| match storage {
            Storage::AsyncTask(task) => task.clone(),
            _ => fail("SystemError", "expected typed asyncio task"),
        })
    }
}

fn resource(storage: Storage) -> *const Object {
    ObjectOwner::into_raw(ObjectOwner::new(Object(StorageCell::new(storage))))
}

/// # Safety
/// The callback accepts the given layout, whose borrowed fields are held for
/// every suspension, and returns one owned word of the given exact kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_coroutine(
    callback: Callback,
    frame: *const usize,
    kind: u32,
) -> *const Object {
    // SAFETY: the caller roots the frame. Its owned reference drops with Call.
    unsafe { imre_record_retain(frame) };
    // SAFETY: kind 6 owns the fixed record retained directly above.
    let frame = unsafe { OwnedWord::from_owned(frame as usize, WordKind::new(6)) };
    resource(Storage::AsyncTask(Task::new(Call {
        callback,
        frame,
        kind: WordKind::new(kind),
    })))
}

/// # Safety
/// The borrowed object roots one coroutine or scheduled task, with the
/// statically known result representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_await(
    handle: *const Object,
    run: bool,
    is_task: bool,
) -> usize {
    // SAFETY: the generated program supplies this exact rooted handle.
    let task = task_of(unsafe { object(handle) });
    if !is_task && task.scheduled.get() {
        let message = if task.done() {
            "cannot reuse already awaited coroutine"
        } else {
            "coroutine is being awaited already"
        };
        return Error::new(Kind::RuntimeError, message).returning(0);
    }
    if run {
        if in_task() {
            return Error::new(
                Kind::RuntimeError,
                "asyncio.run() cannot be called from a running event loop",
            )
            .returning(0);
        }
        let loop_ = Rc::new(EventLoop::default());
        let prior = event_loop::enter(loop_.clone());
        task.schedule();
        drive(&loop_, Some(&task), true);
        event_loop::restore(prior);
        task.result()
    } else {
        await_task(&task)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_async_group() -> *const Object {
    resource(Storage::TaskGroup(Rc::new(Group::default())))
}

/// # Safety
/// The exact group and the current task remain rooted through entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_group_enter(group: *const Object) {
    // SAFETY: compiler roots the sealed task group throughout this call.
    unsafe {
        object(group).0.with(|storage| {
            let Storage::TaskGroup(group) = storage else {
                fail("SystemError", "expected TaskGroup");
            };
            if group.state.get() != GroupState::New {
                Error::new(Kind::RuntimeError, "TaskGroup has already been entered").raise();
            } else {
                group.state.set(GroupState::Entered);
                group.parent.replace(CURRENT_TASK.with_borrow(Clone::clone));
            }
        })
    };
}

/// # Safety
/// Both exact handles stay rooted; the group holds its tasks until join.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_create_task(
    group: *const Object,
    coroutine: *const Object,
) -> *const Object {
    // SAFETY: neither borrowed object is retained across a language callback.
    let task = task_of(unsafe { object(coroutine) });
    if task.scheduled.get() {
        return Error::new(Kind::RuntimeError, "cannot reuse already awaited coroutine")
            .returning(std::ptr::null());
    }
    // SAFETY: a compiler-created group contains a private list of task owners.
    unsafe {
        object(group).0.with(|storage| {
            let Storage::TaskGroup(group) = storage else {
                fail("SystemError", "expected TaskGroup");
            };
            if group.state.get() != GroupState::Entered || group.aborting.get() {
                let state = match group.state.get() {
                    GroupState::New => "has not been entered",
                    GroupState::Finished => "is finished",
                    _ => "is shutting down",
                };
                return Error::new(Kind::RuntimeError, format!("TaskGroup {state}"))
                    .returning(std::ptr::null());
            }
            task.schedule();
            task.group.replace(Rc::downgrade(group));
            group.children.borrow_mut().push(task.clone());
            resource(Storage::AsyncTask(task))
        })
    }
}

/// # Safety
/// The typed group remains rooted until all registered children have finished.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_join(group: *const Object) {
    // SAFETY: extract the children before suspending the language fiber.
    let owner = unsafe {
        object(group).0.with(|storage| {
            let Storage::TaskGroup(group) = storage else {
                fail("SystemError", "expected TaskGroup");
            };
            group.clone()
        })
    };
    owner.state.set(GroupState::Joining);
    let children = owner.children.borrow().clone();
    let mut body_error = owner.error.take();
    while children.iter().any(|task| !task.done()) {
        let failed = body_error.is_some()
            || children.iter().any(|task| {
                task.outcome.borrow().as_ref().is_some_and(|r| {
                    r.as_ref()
                        .err()
                        .is_some_and(|err| super::exceptions::kind_of(err) != Kind::CancelledError)
                })
            });
        if failed {
            owner.abort();
        }
        let remaining: Vec<_> = children
            .iter()
            .filter(|task| !task.done())
            .cloned()
            .collect();
        if !suspend(Box::new(move || remaining.iter().any(|task| task.done()))) {
            body_error = take_pending();
        }
    }
    owner.state.set(GroupState::Finished);
    owner.children.take();
    let mut errors: Vec<_> = children
        .iter()
        .filter_map(|task| {
            task.outcome
                .borrow()
                .as_ref()
                .and_then(|outcome| outcome.as_ref().err().cloned())
                .map(|error| (task.finished.get(), error))
        })
        .collect();
    errors.sort_by_key(|(when, _)| *when);
    let cancellation = body_error
        .as_ref()
        .filter(|err| super::exceptions::kind_of(err) == Kind::CancelledError)
        .cloned();
    let mut errors: Vec<_> = errors
        .into_iter()
        .map(|(_, error)| error)
        .chain(body_error)
        .filter(|err| super::exceptions::kind_of(err) != Kind::CancelledError)
        .collect();
    if let Some(index) = errors
        .iter()
        .position(|err| super::exceptions::kind_of(err) == Kind::SystemExit)
    {
        replace_pending(Some(errors.remove(index)));
    } else if !errors.is_empty() {
        replace_pending(Some(super::exceptions::task_group_errors(errors)));
    } else if cancellation.is_some() {
        replace_pending(cancellation);
    }
}

/// # Safety
/// The group and the exception escaping its body remain rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_abort_group(group: *const Object, error: *const Object) {
    // SAFETY: retaining the escaping exception never runs language code.
    unsafe {
        object(group).0.with(|storage| {
            let Storage::TaskGroup(group) = storage else {
                fail("SystemError", "expected TaskGroup");
            };
            group
                .error
                .replace(Some(ObjectOwner::acquire_word(error as usize)));
        })
    };
}

/// # Safety
/// The typed task is rooted. Its immutable result can be read multiple times.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_result(handle: *const Object) -> usize {
    // SAFETY: group.create_task produces this exact task type.
    task_of(unsafe { object(handle) }).result()
}

pub(super) struct Semaphore {
    free: Cell<usize>,
    waiters: RefCell<std::collections::VecDeque<Rc<Cell<bool>>>>,
}

impl Semaphore {
    fn release(&self) {
        if let Some(next) = self.waiters.borrow_mut().pop_front() {
            // Reserve this permit before scheduling its waiter. A task that
            // releases and reacquires immediately cannot jump the queue.
            next.set(true);
        } else {
            self.free.set(self.free.get().saturating_add(1));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_async_semaphore(permits: usize) -> *const Object {
    // SAFETY: the compiler passes a rooted canonical integer for this call.
    let permits = unsafe { Int::borrow(permits) };
    let count = if let Some(n) = permits.as_small() {
        usize::try_from(n).ok()
    } else {
        use num_traits::Signed;
        let n = permits.big();
        (!n.is_negative()).then(|| n.to_usize().unwrap_or(usize::MAX))
    };
    match count {
        Some(free) => resource(Storage::Semaphore(Rc::new(Semaphore {
            free: Cell::new(free),
            waiters: RefCell::default(),
        }))),
        None => Error::new(Kind::ValueError, "Semaphore initial value must be >= 0")
            .returning(std::ptr::null()),
    }
}

/// # Safety
/// The semaphore stays rooted across acquisition and release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_permit(handle: *const Object, acquire: bool) {
    // SAFETY: retain the language-thread semaphore before suspending.
    let semaphore = unsafe {
        object(handle).0.with(|storage| {
            let Storage::Semaphore(semaphore) = storage else {
                fail("SystemError", "expected Semaphore");
            };
            semaphore.clone()
        })
    };
    if !acquire {
        semaphore.release();
        return;
    }
    let free = semaphore.free.get();
    if free > 0 {
        semaphore.free.set(free - 1);
        return;
    }
    let waiter = Rc::new(Cell::new(false));
    semaphore.waiters.borrow_mut().push_back(waiter.clone());
    let ready = waiter.clone();
    if !suspend(Box::new(move || ready.get())) {
        if waiter.get() {
            semaphore.release();
        } else {
            semaphore
                .waiters
                .borrow_mut()
                .retain(|other| !Rc::ptr_eq(other, &waiter));
        }
    }
}

/// # Safety
/// The input list roots typed task handles, all returning the same kind. For
/// a closed record/exception union, success is tag 4 and error is tag 3.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_gather(
    input: *const Object,
    return_errors: bool,
    kind: u32,
    tagged: bool,
) -> *const () {
    // SAFETY: acquire independent task owners before running other language code.
    let tasks: Vec<_> = unsafe {
        object(input).0.with(|storage| {
            storage
                .sequence()
                .values
                .iter()
                .map(|handle| task_of(object(*handle as *const Object)))
                .collect()
        })
    };
    for task in &tasks {
        task.schedule();
    }
    let mut cancellation = None;
    loop {
        if !return_errors
            && let Some((_, error)) = tasks
                .iter()
                .filter_map(|task| {
                    task.outcome.borrow().as_ref().and_then(|result| {
                        result
                            .as_ref()
                            .err()
                            .map(|error| (task.finished.get(), error.clone()))
                    })
                })
                .min_by_key(|(when, _)| *when)
        {
            replace_pending(Some(error));
            return std::ptr::null();
        }
        let unfinished: Vec<_> = tasks.iter().filter(|task| !task.done()).cloned().collect();
        if unfinished.is_empty() {
            break;
        }
        if !suspend(Box::new(move || unfinished.iter().any(|task| task.done()))) {
            cancellation = take_pending();
            for task in &tasks {
                if !task.done() {
                    task.cancelled.set(true);
                }
            }
        }
    }
    if cancellation.is_some() {
        replace_pending(cancellation);
        return std::ptr::null();
    }
    let output: *const () = if kind == 2 {
        crate::imre_list_new().cast()
    } else {
        imre_seq_new(kind).cast()
    };
    for task in tasks {
        let outcome = task.outcome.borrow();
        let word = match outcome.as_ref().expect("all gather inputs completed") {
            Ok(value) if tagged && value.word != 0 => value.word | 4,
            Ok(value) => value.word,
            Err(error) if kind & 0xff == 9 => ObjectOwner::as_ptr(error) as usize | 3,
            Err(error) => ObjectOwner::as_ptr(error) as usize,
        };
        // SAFETY: the outcome roots its word and the exact list acquires a
        // reference for the returned sequence before that owner is released.
        unsafe {
            if kind == 2 {
                crate::imre_list_append(output.cast(), word);
            } else {
                imre_seq_append(output.cast(), word, kind);
            }
        }
    }
    output
}

/// # Safety
/// The borrowed task is rooted throughout this cancellation request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_cancel(handle: *const Object) -> u32 {
    // SAFETY: the typed handle never leaves its language thread.
    let task = task_of(unsafe { object(handle) });
    if task.done() {
        return 0;
    }
    task.cancelled.set(true);
    1
}
