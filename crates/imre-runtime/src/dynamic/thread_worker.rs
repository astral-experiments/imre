//! Native worker startup and creator-heap ownership for joinable and detached calls.

use super::calls::RegisteredCall;
use super::thread_callback::Callback;
use super::thread_lock::NativeWait;
use super::value::{Heap, ParkedExecution};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak, atomic::Ordering, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::builtins::boolean;
use super::stdlib::hashlib::digest::SharedHash;
use super::{Context, Error, Id, InterpreterState, NONE, Result, Value};
use crate::objects::locks::{PrimitiveLock, thread_ident};
use crate::objects::time::Deadline;

pub(super) type WorkerStatus = std::result::Result<(), String>;

#[derive(Default)]
struct NativeJoin {
    thread: Option<JoinHandle<WorkerStatus>>,
    failure: Option<(&'static str, String)>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum State {
    #[default]
    New,
    Running,
    Detached,
    Done,
}

impl State {
    fn join_pending(self) -> bool {
        self == Self::Running
    }

    fn worker_pending(self) -> bool {
        matches!(self, Self::Running | Self::Detached)
    }
}

#[derive(Default)]
pub(super) struct Handle {
    state: Mutex<State>,
    ident: std::sync::OnceLock<Option<usize>>,
    exiting: Condvar,
    native: Mutex<NativeJoin>,
}

/// The OS wait owns no Python values. The caller and its operands remain on the heap's frame stack.
pub(super) struct Join {
    handle: Arc<Handle>,
    deadline: Option<Deadline>,
}

#[derive(Clone, Copy)]
pub(super) enum Completion {
    Join,
    Shutdown,
}

impl Join {
    /// # Safety
    /// The registered caller owns this context on the current thread through wait and resumption.
    #[inline(never)]
    pub(super) unsafe fn run(self, context: *mut Context) -> Result<Id> {
        // SAFETY: the registered caller owns the interrupted frame through signal dispatch.
        let result =
            unsafe { NativeWait::Join(self.deadline, &self.handle, Completion::Join).run(context) };
        // SAFETY: the native wait and any signal callback have returned to their owner.
        let ctx = unsafe { &mut *context };
        result.map_err(|error| super::stdlib::io::qualify_error(ctx, error, None))?;
        ctx.reap_thread_operands();
        Ok(NONE)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Status {
    IsDone,
    Alive,
}

impl Status {
    fn result(self, ctx: &mut Context, state: State) -> Id {
        match self {
            Self::IsDone => {
                let done = matches!(state, State::Done | State::Detached);
                if done {
                    ctx.reap_thread_operands();
                }
                boolean(done)
            }
            Self::Alive => boolean(state == State::Running),
        }
    }

    pub(super) fn prepare(
        self,
        ctx: &mut Context,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<RegisteredCall> {
        let receiver = match self {
            Self::IsDone => super::builtins::noargs_method_receiver(
                ctx,
                super::thread_handle::CLASS,
                "is_done",
                args,
                kwargs,
            )?,
            Self::Alive => {
                if !kwargs.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        "alive() takes no keyword arguments",
                    ));
                }
                ctx.reap_thread_operands();
                let &[receiver] = args else {
                    return Err(Error::new("TypeError", "invalid arguments for alive()"));
                };
                receiver
            }
        };
        let Value::ThreadHandle(handle) = ctx.heap.get(receiver) else {
            return Err(Error::new("TypeError", "expected a thread handle"));
        };
        let state = *handle.state.lock().unwrap();
        if state == State::Done {
            return Ok(RegisteredCall::Observe(Arc::clone(handle), self));
        }
        Ok(RegisteredCall::Returned(self.result(ctx, state)))
    }

    /// # Safety
    /// The caller owns the observing execution through native teardown and resumption.
    pub(super) unsafe fn wait(self, context: *mut Context, handle: Arc<Handle>) -> Result<Id> {
        // SAFETY: the OS join touches only the selected native handle and leaves its failure for join().
        unsafe { ParkedExecution::wait(context, move || drop(handle.joined())) }?;
        // SAFETY: the observer is active again and may now release its creator-side roots.
        Ok(self.result(unsafe { &mut *context }, State::Done))
    }

    pub(super) fn call(
        self,
        ctx: &mut Context,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let call = self.prepare(ctx, args, kwargs)?;
        // SAFETY: direct runtime callers execute synchronously in the context that prepared the observation.
        unsafe { call.run(ctx) }
    }
}

// A detached worker must not keep its own join handle alive.
struct WorkerExit {
    handle: Weak<Handle>,
    interpreter: Option<Arc<InterpreterState>>,
}

impl WorkerExit {
    fn enter(
        &mut self,
        interpreter: &Arc<InterpreterState>,
        handle: Arc<Handle>,
        ready: &mpsc::SyncSender<std::result::Result<Started, String>>,
        release: Option<Arc<PrimitiveLock>>,
    ) -> bool {
        let (release, released) = release
            .map(|lock| {
                let (ack, released) = mpsc::channel();
                ((lock, ack), released)
            })
            .unzip();
        interpreter.worker_count.fetch_add(1, Ordering::SeqCst);
        self.interpreter = Some(Arc::clone(interpreter));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let native_id = Some(crate::objects::locks::native_thread_id());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let native_id = None;
        let start = Started {
            handle: Arc::clone(&handle),
            ident: thread_ident(),
            native_id,
            released,
        };
        if ready.send(Ok(start)).is_err() || handle.ident.wait().is_none() {
            return false;
        }
        if let Some((lock, ack)) = release {
            // The creator waits for this release before returning from start().
            let result = lock
                .release()
                .map_err(|error| Error::from(error).to_string());
            let released = result.is_ok();
            let _ = ack.send(result);
            return released;
        }
        true
    }
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        let handle = self.handle.upgrade();
        let state = handle.as_ref().map(|handle| handle.state.lock().unwrap());
        // A count observer that reaps roots must see this same exit publication.
        if let Some(interpreter) = &self.interpreter {
            interpreter.worker_count.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some((mut state, handle)) = state.zip(handle.as_ref()) {
            *state = State::Done;
            handle.exiting.notify_all();
        }
    }
}

impl Handle {
    pub(super) fn check_new(&self) -> Result<()> {
        if *self.state.lock().unwrap() == State::New {
            Ok(())
        } else {
            Err(Error::new("RuntimeError", "thread already started"))
        }
    }

    fn spawn(
        self: &Arc<Self>,
        worker: impl FnOnce(&mut WorkerExit) -> WorkerStatus + Send + 'static,
    ) -> Result<()> {
        let mut native = self.native.lock().unwrap();
        {
            let mut state = self.state.lock().unwrap();
            if *state != State::New {
                return Err(Error::new("RuntimeError", "thread already started"));
            }
            *state = State::Running;
        }
        let mut exit = WorkerExit {
            handle: Arc::downgrade(self),
            interpreter: None,
        };
        native.thread = Some(
            thread::Builder::new()
                .spawn(move || {
                    // Publish exit after the worker drops its Context, also when unwinding.
                    worker(&mut exit)
                })
                .map_err(|_| Error::new("RuntimeError", "can't start new thread"))?,
        );
        Ok(())
    }

    pub(super) fn ident(&self) -> usize {
        self.ident.get().copied().flatten().unwrap_or(0)
    }

    fn awaiting_exit(&self) -> bool {
        self.state.lock().unwrap().worker_pending()
    }

    pub(super) fn join_pending(&self) -> bool {
        self.state.lock().unwrap().join_pending()
    }

    pub(super) fn set_done(&self) -> Result<()> {
        let mut native = self.native.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        if *state == State::New {
            return Err(Error::new("RuntimeError", "thread not started"));
        }
        // Detach without waiting. The worker still owns its creator roots until actual exit.
        // Exit notification can precede the first join or detach of the OS handle.
        let detached = native.thread.take().is_some();
        if *state == State::Running {
            *state = if detached {
                State::Detached
            } else {
                State::Done
            };
        }
        self.exiting.notify_all();
        Ok(())
    }

    // After the exit signal, all observers synchronize with the same native join.
    fn joined(&self) -> MutexGuard<'_, NativeJoin> {
        let mut native = self.native.lock().unwrap();
        if let Some(thread) = native.thread.take() {
            native.failure = match thread.join() {
                Ok(status) => status.err().map(|message| ("SystemError", message)),
                Err(_) => Some(("RuntimeError", "native worker panicked".to_owned())),
            };
        }
        native
    }

    fn wait(&self, timeout: Option<Duration>, pending: fn(State) -> bool) -> bool {
        let state = self.state.lock().unwrap();
        let state = match timeout {
            Some(limit) => {
                self.exiting
                    .wait_timeout_while(state, limit, |s| pending(*s))
                    .unwrap()
                    .0
            }
            None => self.exiting.wait_while(state, |s| pending(*s)).unwrap(),
        };
        !pending(*state)
    }

    fn join(&self, timeout: Option<Duration>) -> Result<()> {
        self.wait_for_exit(timeout, Completion::Join).map(drop)
    }

    // The timeout applies to exit publication. Once published, even a zero-timeout
    // observer synchronizes with pthread teardown before returning to Python.
    pub(super) fn wait_for_exit(
        &self,
        timeout: Option<Duration>,
        completion: Completion,
    ) -> Result<bool> {
        if !self.wait(timeout, State::join_pending) {
            return Ok(false);
        }
        match completion {
            Completion::Join => {
                if let Some((kind, message)) = self.joined().failure.take() {
                    return Err(Error::new(kind, message));
                }
            }
            Completion::Shutdown => self.finish_shutdown(),
        }
        Ok(true)
    }

    fn finish_shutdown(&self) {
        // Detach releases Python joiners, but shutdown waits for actual worker exit.
        self.wait(None, State::worker_pending);
        drop(self.joined());
    }

    pub(super) fn prepare_join(
        self: Arc<Self>,
        ctx: &Context,
        timeout: Option<Duration>,
    ) -> Result<Join> {
        match *self.state.lock().unwrap() {
            State::New => return Err(Error::new("RuntimeError", "thread not started")),
            State::Running if self.ident() == thread_ident() => {
                return Err(Error::new("RuntimeError", "Cannot join current thread"));
            }
            State::Running if ctx.interpreter.finalizing.load(Ordering::Acquire) => {
                return Err(Error::new(
                    "PythonFinalizationError",
                    "cannot join thread at interpreter shutdown",
                ));
            }
            _ => {}
        }
        Ok(Join {
            handle: self,
            deadline: timeout.map(Deadline::after),
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RunMode {
    Default,
    Bound,
}

enum RunEntry {
    Default { target: Id, args: Id, kwargs: Id },
    Bound { function: Id },
}

struct ThreadOperands {
    entry: RunEntry,
    handle: Id,
}

struct RegisteredThread {
    daemon: bool,
    owner: Id,
    run: RunMode,
}

struct Entry {
    // The handle tracks actual worker exit even when Python has already detached its join.
    handle: Arc<Handle>,
    operands: Vec<Id>,
    thread: Option<RegisteredThread>,
}

#[derive(Default)]
pub(super) struct Registry {
    joined_at_shutdown: bool,
    entries: Vec<Entry>,
    default_run: Option<[Id; 3]>,
    main: Option<(usize, Id)>,
    reporting: Option<(Id, Option<Id>)>,
}

impl Context {
    /// Look up worker inputs only when the default Python attribute accesses cannot run user code.
    fn thread_operands(&self, owner: Id) -> Result<ThreadOperands> {
        let unsupported = || {
            Error::new(
                "NotImplementedError",
                "native workers require an ordinary Thread with a supported run method",
            )
        };
        let Value::Instance {
            class,
            fields,
            namespace,
            ..
        } = self.heap.get(owner)
        else {
            return Err(unsupported());
        };
        let Some([thread_class, run, code]) = self.heap.workers.default_run else {
            return Err(unsupported());
        };
        if !self.direct_subclass(*class, thread_class)
            || !matches!(*self.heap.get(run), Value::Function { code: current, .. } if code == current)
        {
            return Err(unsupported());
        }
        for name in [
            "__getattribute__",
            "__getattr__",
            "__setattr__",
            "__delattr__",
            "_context",
            "_target",
            "_args",
            "_kwargs",
            "_handle",
            "_os_thread_handle",
            "_ident",
            "_native_id",
        ] {
            if self.class_attr(*class, name).is_some() {
                return Err(unsupported());
            }
        }
        // A user-supplied dictionary key could execute __eq__ in an ordinary attribute lookup.
        let dictionary = namespace.and_then(|dict| self.dict_entries(dict));
        if dictionary.is_some_and(|entries| {
            entries
                .iter()
                .any(|(key, _)| !matches!(self.heap.get(*key), Value::Str(_)))
        }) {
            return Err(unsupported());
        }
        let field = |name: &str| match dictionary {
            Some(entries) => entries.iter().find_map(|(key, value)| {
                matches!(self.heap.get(*key), Value::Str(text) if text == name).then_some(*value)
            }),
            None => fields.get(name).copied(),
        };
        if field("_handle").is_some_and(|id| matches!(self.heap.get(id), Value::ThreadHandle(_))) {
            return Err(Error::new("RuntimeError", "thread already started"));
        }
        if field("run").is_some()
            || field("_context") != Some(NONE)
            || field("_handle") != Some(NONE)
        {
            return Err(unsupported());
        }
        // Inspect the class slot directly: binding a plain function here cannot invoke a
        // descriptor or an instance attribute hook. Admission still belongs to Callback.
        let function = self.class_attr(*class, "run").ok_or_else(unsupported)?;
        let entry = if function == run {
            RunEntry::Default {
                target: field("_target").ok_or_else(unsupported)?,
                args: field("_args").ok_or_else(unsupported)?,
                kwargs: field("_kwargs").ok_or_else(unsupported)?,
            }
        } else if matches!(self.heap.get(function), Value::Function { .. }) {
            RunEntry::Bound { function }
        } else {
            return Err(unsupported());
        };
        Ok(ThreadOperands {
            entry,
            handle: field("_os_thread_handle").ok_or_else(unsupported)?,
        })
    }

    /// Retain the original Python function and code after the sealed module initializes.
    pub(super) fn register_thread_run(&mut self, class: Id, main: Id) {
        if self.threads.private_context {
            return;
        }
        if let Some(callable) = self.own_class_attr(class, "run")
            && let Value::Function { code, scope, .. } = *self.heap.get(callable)
        {
            self.heap.workers.default_run = Some([class, callable, code]);
            self.heap.workers.main = Some((thread_ident(), main));
            self.heap.workers.reporting = Some((scope, self.scope_name(scope, "excepthook")));
        }
    }

    /// A surviving main thread keeps its pthread identity, but receives a new Linux TID.
    #[cfg(target_os = "linux")]
    pub(super) fn refresh_main_thread_after_fork(&mut self) {
        if let Some((_, main)) = self.heap.workers.main {
            let tid = self.int(crate::objects::locks::native_thread_id() as i64);
            if let Err(error) = self.set_attr(main, "_native_id", tid) {
                super::unraisable::report(
                    self,
                    error,
                    false,
                    None,
                    "Exception ignored updating the main thread after fork",
                );
            }
        }
    }

    pub(super) fn reap_thread_operands(&mut self) {
        self.heap.reap_workers(|entry| entry.thread.is_some());
    }
}

impl RegisteredThread {
    /// Complete default Thread.run cleanup on the creator heap before releasing its bound operands.
    fn clear_target(owner: Id, heap: &mut Heap) -> bool {
        let Value::Instance { namespace, .. } = *heap.get(owner) else {
            unreachable!("a registered Thread owns its original instance")
        };
        let dictionary = namespace.and_then(|dict| heap.dict_storage(dict));
        for name in ["_target", "_args", "_kwargs"] {
            heap.remove_instance_field(owner, name);
            if let Some(dict) = dictionary
                && let Some(position) = heap.dict_entries(dict).and_then(|items| {
                    items.iter().position(
                        |&(key, _)| matches!(heap.get(key), Value::Str(text) if text == name),
                    )
                })
                // The first removal may need to allocate iterator positions. Keep the
                // registration intact so the next checkpoint can finish the cleanup.
                && heap.remove_table_entry(dict, position).is_err()
            {
                return false;
            }
        }
        true
    }
}

impl Entry {
    fn shutdown_handle(&self, caller: usize) -> Option<&Arc<Handle>> {
        (!self.thread.as_ref()?.daemon && self.handle.ident() != caller).then_some(&self.handle)
    }

    fn roots(&self) -> impl Iterator<Item = &Id> {
        self.thread
            .iter()
            .map(|thread| &thread.owner)
            .chain(&self.operands)
    }
}

impl Heap {
    /// Release completed registrations in startup order. Keep every other record visible to GC.
    fn reap_workers(&mut self, include: impl Fn(&Entry) -> bool) {
        let mut kept = 0;
        for index in 0..self.workers.entries.len() {
            let entry = &self.workers.entries[index];
            if include(entry) && !entry.handle.awaiting_exit() {
                let owner = entry.thread.as_ref().map(|thread| thread.owner);
                let cleanup_owner = entry
                    .thread
                    .as_ref()
                    .filter(|thread| thread.run == RunMode::Default)
                    .map(|thread| thread.owner);
                // Only default Thread.run performs target cleanup. A failed field removal
                // leaves the registration intact for the next checkpoint.
                if entry.handle.ident() == 0
                    || cleanup_owner.is_none_or(|owner| RegisteredThread::clear_target(owner, self))
                {
                    let operands = std::mem::take(&mut self.workers.entries[index].operands);
                    for root in owner.into_iter().chain(operands) {
                        self.weak_edge_removed(root);
                    }
                    continue;
                }
            }
            self.workers.entries.swap(kept, index);
            kept += 1;
        }
        self.workers.entries.truncate(kept);
    }

    pub(super) fn reap_finished_workers(&mut self) {
        self.reap_workers(|_| true);
    }
}

impl Registry {
    fn reserve_entry(&mut self) -> Result<()> {
        self.entries
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))
    }

    /// Reaping an entry can release creator-heap roots independently of the current operands.
    pub(super) fn has_unreaped_workers(&self) -> bool {
        !self.entries.is_empty()
    }

    pub(super) fn detached_active(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.thread.is_none() && entry.handle.awaiting_exit())
    }

    fn live_threads(&self) -> impl Iterator<Item = (usize, Id)> + '_ {
        self.entries
            .iter()
            .filter_map(|entry| Some((&entry.handle, entry.thread.as_ref()?.owner)))
            .filter(|(handle, _)| handle.awaiting_exit())
            .map(|(handle, owner)| (handle.ident(), owner))
    }

    pub(super) fn roots(&self) -> impl Iterator<Item = &Id> {
        // Completion can race tracing. Keep the entire record rooted until the creator reaps it.
        self.entries
            .iter()
            .flat_map(Entry::roots)
            .chain(self.default_run.iter().flatten())
            .chain(self.main.iter().map(|(_, main)| main))
            .chain(
                self.reporting
                    .iter()
                    .flat_map(|(scope, hook)| std::iter::once(scope).chain(hook)),
            )
    }
}

impl Registry {
    fn shutdown_handles(&self) -> impl Iterator<Item = &Arc<Handle>> {
        let caller = thread_ident();
        self.entries
            .iter()
            .filter_map(move |entry| entry.shutdown_handle(caller))
    }

    fn join_non_daemon(&self) {
        for handle in self.shutdown_handles() {
            handle.finish_shutdown();
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        if !self.joined_at_shutdown {
            self.join_non_daemon();
        }
    }
}

pub(super) enum Operation {
    Python(Box<Callback>),
    Empty,
    UpdateHash {
        hash: SharedHash,
        data: Vec<u8>,
    },
    Acquire {
        wait: Arc<PrimitiveLock>,
        release: Option<Arc<PrimitiveLock>>,
    },
}

enum Registration {
    Thread {
        handle: Arc<Handle>,
        handle_id: Id,
        thread: RegisteredThread,
    },
    Detached(String),
}

/// Match the interpreter's lookup of an already imported threading module at exit.
pub(super) fn prepare_shutdown(ctx: &mut Context) -> Result<super::calls::RegisteredCall> {
    // An exception in the Python handler can deliberately prevent a non-daemon join.
    // Do not repeat that join in Drop or include workers started later by ordinary atexit.
    ctx.heap.workers.joined_at_shutdown = true;
    let key = ctx.text("threading");
    let Some(module) = ctx.initialized_registry_entry(key)? else {
        return Ok(super::calls::RegisteredCall::Returned(NONE));
    };
    let shutdown = ctx.lookup_attr(module, "_shutdown")?;
    ctx.prepare_call(shutdown, Vec::new().into(), Vec::new())
}

/// Wait for the non-daemon workers of the registered caller.
///
/// # Safety
/// The caller owns this context through signal dispatch and resumption.
pub(super) unsafe fn shutdown_workers(context: *mut Context) -> Result<Id> {
    // SAFETY: preparation returned the context to the registered caller on this thread.
    unsafe { &mut *context }.reap_thread_operands();
    // SAFETY: joins park and resume this caller before reporting signals or failures.
    let result = unsafe { join_shutdown_handles(context) };
    // SAFETY: the native joins and any signal handler returned to this caller.
    let ctx = unsafe { &mut *context };
    match result {
        Ok(()) => ctx.reap_thread_operands(),
        Err(error) => super::unraisable::report(
            ctx,
            error,
            false,
            None,
            "Exception ignored while joining a thread in _thread._shutdown()",
        ),
    }
    Ok(NONE)
}

/// # Safety
/// The shutdown caller owns the context through any signal callback between waits.
unsafe fn join_shutdown_handles(context: *mut Context) -> Result<()> {
    loop {
        // SAFETY: only this thread accesses its registry. Clone the handle so the
        // borrow ends here, before a signal handler can change the registry.
        let handle = unsafe { &*context }
            .heap
            .workers
            .shutdown_handles()
            .find(|handle| handle.awaiting_exit())
            .cloned();
        let Some(handle) = handle else { break };
        // SAFETY: the registered caller owns the context through signal dispatch.
        // Only the owned native handle crosses the wait.
        unsafe { NativeWait::Join(None, &handle, Completion::Shutdown).run(context) }?;
    }
    // Other workers may have published completion during an interruptible wait.
    // Finish their native joins, which can still block in native thread teardown.
    // SAFETY: the registered caller is back in its context. Snapshot the native
    // handles so the registry borrow ends before the first join.
    let handles: Vec<_> = unsafe { &*context }
        .heap
        .workers
        .shutdown_handles()
        .cloned()
        .collect();
    if !handles.is_empty() {
        // SAFETY: the snapshot owns only native handles. No registry borrow crosses the joins.
        unsafe {
            ParkedExecution::wait(context, move || {
                for handle in handles {
                    drop(handle.joined());
                }
            })
        }?;
    }
    Ok(())
}

/// Reject new work before binding or moving its Python callback operands.
pub(super) fn check_start(ctx: &Context) -> Result<()> {
    if ctx.interpreter.finalizing.load(Ordering::Acquire) {
        return Err(Error::new(
            "PythonFinalizationError",
            "can't create new thread at interpreter shutdown",
        ));
    }
    Ok(())
}

/// The worker waits until its creator publishes the rooted receiver and native identity.
struct Started {
    handle: Arc<Handle>,
    ident: usize,
    native_id: Option<u64>,
    released: Option<mpsc::Receiver<WorkerStatus>>,
}

impl Started {
    /// Publish the worker identity and wait for any promised release on the parked caller.
    ///
    /// # Safety
    /// The calling thread owns the active context through publication.
    unsafe fn publish(self, context: *mut Context) -> Result<()> {
        let waits_for_release = self.released.is_some();
        let publish = move || {
            self.handle
                .ident
                .set(Some(self.ident))
                .expect("unstarted handle");
            if let Some(released) = &self.released
                && let Ok(Err(reason)) = released.recv()
            {
                return Err(Error::new("RuntimeError", reason));
            }
            Ok(())
        };
        if waits_for_release {
            // Opening the gate after parking lets Drop cancel an unpublished worker
            // if reserving a parked slot fails.
            // SAFETY: the owned gate and its acknowledgement carry only native data.
            unsafe { ParkedExecution::wait(context, publish)? }
        } else {
            publish()
        }
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        // Wake the worker even if updating the receiver failed or unwound.
        self.handle.ident.get_or_init(|| None);
    }
}

/// The worker cannot enter its callback until the receiver's native identity is published.
struct Ready {
    handle: Arc<Handle>,
    started: mpsc::Receiver<std::result::Result<Started, String>>,
}

impl Ready {
    fn wait(self) -> Result<Started> {
        match self.started.recv() {
            Ok(Ok(publication)) => Ok(publication),
            failure => {
                let _ = self.handle.join(None);
                let reason = match failure {
                    Ok(Err(message)) => message,
                    _ => "thread initialization failed".into(),
                };
                Err(Error::new("RuntimeError", reason))
            }
        }
    }
}

/// Everything needed to spawn the worker after releasing the selection borrow.
pub(super) struct Start {
    operation: Operation,
    registration: Registration,
    operands: Vec<Id>,
    interpreter: Arc<InterpreterState>,
    fault_workers: Option<Arc<()>>,
}

impl Start {
    fn new(
        ctx: &Context,
        operation: Operation,
        registration: Registration,
        operands: Vec<Id>,
    ) -> Self {
        let fault_workers =
            matches!(operation, Operation::Python(_)).then(|| ctx.heap.begin_fault_worker());
        Self {
            operation,
            registration,
            operands,
            interpreter: Arc::clone(&ctx.interpreter),
            fault_workers,
        }
    }
}

fn launch(mut request: Start) -> Result<(Ready, Entry)> {
    let (handle, thread, detached_label) = match request.registration {
        Registration::Thread { handle, thread, .. } => (handle, Some(thread), None),
        Registration::Detached(label) => (Arc::default(), None, Some(label)),
    };
    // The child's identity can arrive before the creator receives it. Dropping
    // the queued publication cancels the worker through the same gate.
    let (ready, started) = mpsc::sync_channel(1);
    let starting = Arc::clone(&handle);
    handle.spawn(move |exit| {
        let release = match &mut request.operation {
            Operation::Acquire { release, .. } => release.take(),
            _ => None,
        };
        let enter = || exit.enter(&request.interpreter, starting, &ready, release);
        let wait = match request.operation {
            Operation::Python(callback) => {
                // Preparation and cancellation both belong to this pthread.
                let _publication =
                    crate::fault_handler::FramePublication::enter(&request.interpreter.finalizing);
                return callback
                    .run(
                        Arc::clone(&request.interpreter),
                        request.fault_workers.as_ref().expect("Python worker"),
                        enter,
                        detached_label,
                    )
                    .unwrap_or_else(|error| {
                        let _ = ready.send(Err(error.to_string()));
                        Ok(())
                    });
            }
            _ if !enter() => return Ok(()),
            Operation::Empty => return Ok(()),
            Operation::UpdateHash { hash, data } => {
                return hash.lock().update(&data).map_err(|error| error.to_string());
            }
            Operation::Acquire { wait, .. } => wait,
        };
        wait.acquire(true, None);
        Ok(())
    })?;
    let entry = Entry {
        handle: Arc::clone(&handle),
        operands: request.operands,
        thread,
    };
    Ok((Ready { handle, started }, entry))
}

/// A low-level caller owns only the bootstrap roots after the native worker is detached.
pub(super) fn prepare_detached(
    ctx: &mut Context,
    callback: Operation,
    function: Id,
    operands: Vec<Id>,
) -> Result<Start> {
    ctx.heap.reap_workers(|entry| entry.thread.is_none());
    ctx.heap.workers.reserve_entry()?;
    let label = ctx.repr(function);
    Ok(Start::new(
        ctx,
        callback,
        Registration::Detached(label),
        operands,
    ))
}

// Validate the actual bound method and its operands before starting an OS
// thread. No Python handle, callback, or pointer into Context may cross here.
fn worker_target(
    ctx: &mut Context,
    target: Id,
    args: Id,
    kwargs: Id,
) -> Result<(Operation, Vec<Id>)> {
    if target == NONE {
        return Ok((Operation::Empty, Vec::new()));
    }
    let sequence = ctx.heap.get(args);
    let (Value::Tuple(pos) | Value::List(pos)) = sequence else {
        return Err(super::thread_callback::unsupported());
    };
    let Value::Dict(keywords) = ctx.heap.get(kwargs) else {
        return Err(super::thread_callback::unsupported());
    };
    // Thread.run expands the containers. A bound method can retain the original positional tuple.
    let operands = [target, args];
    let target_value = ctx.heap.get(target);
    let roots =
        if matches!(target_value, Value::Method { .. }) && matches!(sequence, Value::Tuple(_)) {
            &operands[..]
        } else {
            &operands[..1]
        };
    if let Some(captured) = Callback::capture(ctx, target_value, roots, pos, Some(kwargs), true)? {
        return captured
            .resolve(ctx)?
            .ok_or_else(super::thread_callback::unsupported_target);
    }
    if let Value::Method {
        callable, receiver, ..
    } = *target_value
        && pos.is_empty()
        && keywords.is_empty()
        && matches!(ctx.heap.get(receiver), Value::PrimitiveLock(_))
    {
        let operation =
            super::thread_callback::prepare_primitive_acquire(ctx, callable, receiver, receiver)
                .map_err(|error| {
                    if error.kind == "MemoryError" {
                        error
                    } else {
                        super::thread_callback::unsupported_target()
                    }
                })?
                .ok_or_else(super::thread_callback::unsupported_target)?;
        return Ok((operation, super::concat_ids(roots, &[])?));
    }
    if let Value::Method {
        callable, receiver, ..
    } = *target_value
        && keywords.is_empty()
    {
        let (symbol, storage) = match ctx.heap.get(callable) {
            Value::Native(symbol) => (symbol.as_str(), receiver),
            Value::HeapTypeMethod {
                owner,
                name: "update",
                ..
            } if ctx.heap.get(receiver).instance_class() == Some(*owner) => {
                ("hash.update", ctx.scalar_value(receiver))
            }
            _ => ("", receiver),
        };
        let operation = match (ctx.heap.get(storage), pos.as_slice()) {
            (value @ Value::Hash(hash), [id])
                if super::builtins::method_name(value, "update").as_deref() == Some(symbol) =>
            {
                if let Value::Bytes(data) = ctx.heap.get(*id) {
                    Some(Operation::UpdateHash {
                        hash: hash.clone(),
                        data: super::copy_ids(data)?,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(operation) = operation {
            // The bound call owns its expanded arguments even if the caller clears the list.
            return Ok((operation, super::concat_ids(roots, pos)?));
        }
    }
    Err(super::thread_callback::unsupported_target())
}

impl Start {
    /// # Safety
    /// Run immediately after preparation, before reentering Python on the owning frame.
    pub(super) unsafe fn run(self, context: *mut Context) -> Result<Id> {
        let receiver = match &self.registration {
            Registration::Thread {
                handle_id, thread, ..
            } => {
                // The call consumes this wrapper even if the receiver's field is rebound.
                // SAFETY: the creator owns this frame through startup and publication.
                let ctx = unsafe { &mut *context };
                ctx.heap.reserve_temporaries(1)?;
                ctx.heap.keep(*handle_id);
                Some((thread.owner, *handle_id))
            }
            Registration::Detached(_) => None,
        };
        let ready = match launch(self) {
            Ok((ready, entry)) => {
                // SAFETY: spawn runs no Python on the creator. Register the selected roots in
                // the reserved slot before a callback, wait or collection can run here.
                unsafe { &mut *context }.heap.workers.entries.push(entry);
                ready
            }
            Err(error) => {
                // SAFETY: the failed native spawn returned to the registered caller.
                let ctx = unsafe { &mut *context };
                // CPython consumes the handle even if the OS refuses to create a worker.
                let error = if let Some((owner, handle)) = receiver {
                    ctx.set_attr(owner, "_handle", handle)
                        .err()
                        .unwrap_or(error)
                } else {
                    error
                };
                return Err(super::stdlib::io::qualify_error(ctx, error, None));
            }
        };
        // The registry owns the selected Python operands before the creator
        // yields. A failed park drops Ready and cancels any queued publication.
        // SAFETY: readiness and its failure join use only native handles.
        let ready = unsafe { ParkedExecution::wait(context, move || ready.wait()) }.flatten();
        let (started, thread_ids) = if let Some((owner, handle)) = receiver {
            // SAFETY: the native wait has returned to the registered caller.
            let ctx = unsafe { &mut *context };
            ctx.set_attr(owner, "_handle", handle)?;
            let started = ready?;
            let ident = ctx.alloc(Value::Int(crate::Int::from_usize(started.ident)));
            let tid = started.native_id.map_or(NONE, |id| {
                ctx.alloc(Value::Int(crate::Int::from_usize(id as usize)))
            });
            ctx.set_attr(owner, "_ident", ident)?;
            ctx.set_attr(owner, "_native_id", tid)?;
            // Caller temporaries also retain these IDs if a hook redirects a write.
            (started, Some([handle, ident, tid]))
        } else {
            let started = ready?;
            // Detached callers receive only the ident. Actual exit still owns the roots.
            started.handle.set_done()?;
            (started, None)
        };
        let ident = started.ident;
        // SAFETY: the creator has installed the registration and owns this publication.
        unsafe { started.publish(context) }?;
        // SAFETY: native publication has returned to the active caller.
        let ctx = unsafe { &mut *context };
        Ok(match thread_ids {
            Some(ids) => ctx.alloc(Value::Tuple(Vec::from(ids))),
            None => ctx.int(ident as i64),
        })
    }
}

pub(super) fn prepare_start(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Start> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "start() takes no keyword arguments",
        ));
    }
    ctx.reap_thread_operands();
    let (&[owner, daemon] | &[owner, daemon, _]) = args else {
        return Err(Error::new("TypeError", "invalid arguments for start()"));
    };
    // The independent worker cannot publish CPython's actual Python bootstrap
    // callable and handle to an audit hook. Reject before consuming the handle.
    if !ctx.audit_hooks.is_empty() {
        return Err(Error::new(
            "NotImplementedError",
            "audited joinable thread startup requires the boxed Python bootstrap",
        ));
    }
    check_start(ctx)?;
    if args
        .get(2)
        .is_some_and(|&has_context| has_context != super::FALSE)
    {
        return Err(Error::new(
            "NotImplementedError",
            "native workers cannot enter a Python context",
        ));
    }
    let ThreadOperands {
        entry,
        handle: handle_id,
    } = ctx.thread_operands(owner)?;
    let Value::ThreadHandle(handle) = ctx.heap.get(handle_id) else {
        return Err(Error::new("TypeError", "'handle' must be a _ThreadHandle"));
    };
    let handle = Arc::clone(handle);
    handle.check_new()?;
    let (operation, roots, run) = match entry {
        RunEntry::Default {
            target,
            args,
            kwargs,
        } => {
            let (operation, roots) = worker_target(ctx, target, args, kwargs)?;
            (operation, roots, RunMode::Default)
        }
        RunEntry::Bound { function } => {
            // The synthetic bound value stays on the creator. Only Callback's certified
            // native operation crosses into the independent worker; the registry roots
            // the function, owner, and projected operand until actual exit.
            let method = Value::Method {
                callable: function,
                receiver: owner,
                explicit: true,
            };
            let capture = Callback::capture(ctx, &method, &[function], &[], None, true)?
                .ok_or_else(super::thread_callback::unsupported_target)?;
            let (operation, roots) = capture
                .resolve(ctx)?
                .ok_or_else(super::thread_callback::unsupported_target)?;
            (operation, roots, RunMode::Bound)
        }
    };
    let Value::Bool(daemon) = *ctx.heap.get(daemon) else {
        return Err(Error::new("TypeError", "expected a boolean daemon flag"));
    };
    if ctx
        .heap
        .workers
        .reporting
        .is_some_and(|(scope, original)| ctx.scope_name(scope, "excepthook") != original)
    {
        return Err(Error::new(
            "NotImplementedError",
            "custom threading.excepthook requires the parent Python heap",
        ));
    }
    ctx.heap.workers.reserve_entry()?;
    let thread = RegisteredThread { daemon, owner, run };
    Ok(Start::new(
        ctx,
        operation,
        Registration::Thread {
            handle,
            handle_id,
            thread,
        },
        roots,
    ))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "alive" {
        return Status::Alive.call(ctx, args, kwargs);
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    ctx.reap_thread_operands();
    match (name, args) {
        ("join", [_, _]) => super::thread_handle::method(ctx, "join", args, kwargs),
        ("attach_current", &[handle]) => {
            let Value::ThreadHandle(handle) = ctx.heap.get(handle) else {
                return Err(Error::new("TypeError", "expected a thread handle"));
            };
            let mut state = handle.state.lock().unwrap();
            if *state != State::New {
                return Err(Error::new("RuntimeError", "thread already started"));
            }
            handle
                .ident
                .set(Some(thread_ident()))
                .expect("unstarted handle");
            *state = State::Running;
            Ok(NONE)
        }
        ("current", &[main]) => {
            let ident = thread_ident();
            if ctx.heap.workers.main == Some((ident, main)) {
                return Ok(main);
            }
            ctx.heap
                .workers
                .live_threads()
                .find_map(|(id, owner)| (id == ident).then_some(owner))
                .ok_or_else(|| {
                    Error::new(
                        "NotImplementedError",
                        "current thread has no owner in this interpreter",
                    )
                })
        }
        ("threads", []) => {
            let owners = ctx
                .heap
                .workers
                .live_threads()
                .map(|(_, owner)| owner)
                .collect();
            Ok(ctx.alloc(Value::List(owners)))
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments for {name}()"),
        )),
    }
}

#[cfg(test)]
mod tests;
