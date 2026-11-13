//! Native worker startup and creator-heap ownership for joinable and detached calls.

use super::thread_callback::Callback;
use super::thread_lock::Acquire;
use super::value::Heap;
use std::io::Write;
use std::ops::ControlFlow;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, Weak,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::builtins::boolean;
use super::stdlib::hashlib::digest::SharedHash;
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::locks::{PrimitiveLock, thread_ident};
use crate::objects::time::Deadline;

type WorkerStatus = std::result::Result<(), String>;

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

impl Join {
    /// # Safety
    /// The registered caller owns this context on the current thread through wait and resumption.
    #[inline(never)]
    pub(super) unsafe fn run(self, context: *mut Context) -> Result<Id> {
        let result = loop {
            if let ControlFlow::Break(done) = self.handle.wait_interruptible(self.deadline) {
                break self.handle.join_result(done);
            }
            // SAFETY: the wait released both native locks. The interrupted Python frame and
            // its exception are still current while the handler runs.
            #[cfg(unix)]
            if let Err(error) = unsafe { Context::dispatch_pending_calls(context) } {
                break Err(error);
            }
        };
        // SAFETY: the native wait and any signal callback have returned to their owner.
        let ctx = unsafe { &mut *context };
        result.map_err(|error| super::stdlib::io::qualify_error(ctx, error, None))?;
        ctx.reap_thread_operands();
        Ok(NONE)
    }
}

// A detached worker must not keep its own join handle alive.
struct WorkerExit {
    handle: Weak<Handle>,
    count: Option<Arc<AtomicUsize>>,
}

impl WorkerExit {
    fn enter(
        &mut self,
        count: Arc<AtomicUsize>,
        handle: Arc<Handle>,
        ready: &mpsc::SyncSender<std::result::Result<Started, String>>,
        released: Option<mpsc::Receiver<WorkerStatus>>,
    ) -> bool {
        count.fetch_add(1, Ordering::SeqCst);
        self.count = Some(count);
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
        ready.send(Ok(start)).is_ok() && handle.ident.wait().is_some()
    }
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        let handle = self.handle.upgrade();
        let state = handle.as_ref().map(|handle| handle.state.lock().unwrap());
        // A count observer that reaps roots must see this same exit publication.
        if let Some(count) = &self.count {
            count.fetch_sub(1, Ordering::SeqCst);
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
            count: None,
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

    fn running(&self) -> bool {
        *self.state.lock().unwrap() == State::Running
    }

    pub(super) fn is_done(&self) -> bool {
        let state = *self.state.lock().unwrap();
        if state == State::Done {
            drop(self.joined());
        }
        matches!(state, State::Done | State::Detached)
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

    fn alive(&self) -> bool {
        if self.running() {
            return true;
        }
        drop(self.joined());
        false
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
        self.join_result(self.wait(timeout, State::join_pending))
    }

    fn join_result(&self, completed: bool) -> Result<()> {
        if completed && let Some((kind, message)) = self.joined().failure.take() {
            return Err(Error::new(kind, message));
        }
        Ok(())
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

    /// The condvar can restart after EINTR. Dispatch Python outside both handle locks.
    fn wait_interruptible(&self, deadline: Option<Deadline>) -> ControlFlow<bool> {
        #[cfg(unix)]
        if crate::signals::is_main() {
            loop {
                let remaining = deadline.map(Deadline::remaining);
                let interval = Duration::from_millis(50);
                if self.wait(
                    Some(remaining.map_or(interval, |left| left.min(interval))),
                    State::join_pending,
                ) {
                    return ControlFlow::Break(true);
                }
                if deadline.is_some_and(|end| end.remaining().is_zero()) {
                    return ControlFlow::Break(false);
                }
                if crate::signals::TRIPPED.load(Ordering::Acquire) {
                    return ControlFlow::Continue(());
                }
            }
        }
        ControlFlow::Break(self.wait(deadline.map(Deadline::remaining), State::join_pending))
    }
}

struct RegisteredThread {
    daemon: bool,
    owner: Id,
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
    fn thread_operands(&self, owner: Id) -> Result<[Id; 4]> {
        let unsupported = || {
            Error::new(
                "NotImplementedError",
                "native workers require an ordinary Thread with the original run method",
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
        let Some([thread_class, run, code]) = self.workers.default_run else {
            return Err(unsupported());
        };
        if *class != thread_class
            || self.own_class_attr(*class, "run") != Some(run)
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
        Ok([
            field("_target").ok_or_else(unsupported)?,
            field("_args").ok_or_else(unsupported)?,
            field("_kwargs").ok_or_else(unsupported)?,
            field("_os_thread_handle").ok_or_else(unsupported)?,
        ])
    }

    /// Retain the original Python function and code after the sealed module initializes.
    pub(super) fn register_thread_run(&mut self, class: Id, main: Id) {
        if self.threads.private_context {
            return;
        }
        if let Some(callable) = self.own_class_attr(class, "run")
            && let Value::Function { code, scope, .. } = *self.heap.get(callable)
        {
            self.workers.default_run = Some([class, callable, code]);
            self.workers.main = Some((thread_ident(), main));
            self.workers.reporting = Some((scope, self.scope_name(scope, "excepthook")));
        }
    }

    /// A surviving main thread keeps its pthread identity, but receives a new Linux TID.
    #[cfg(target_os = "linux")]
    pub(super) fn refresh_main_thread_after_fork(&mut self) {
        if let Some((_, main)) = self.workers.main {
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
        self.workers
            .reap_where(&mut self.heap, |entry| entry.thread.is_some());
    }
}

impl RegisteredThread {
    /// Complete default Thread.run cleanup on the creator heap before releasing its bound operands.
    fn clear_target(&self, heap: &mut Heap) -> bool {
        let Value::Instance { namespace, .. } = *heap.get(self.owner) else {
            unreachable!("a registered Thread owns its original instance")
        };
        let dictionary = namespace.and_then(|dict| heap.dict_storage(dict));
        for name in ["_target", "_args", "_kwargs"] {
            heap.remove_instance_field(self.owner, name);
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
    fn roots(&self) -> impl Iterator<Item = &Id> {
        self.thread
            .iter()
            .map(|thread| &thread.owner)
            .chain(&self.operands)
    }
}

impl Registry {
    fn reserve_entry(&mut self) -> Result<()> {
        self.entries
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))
    }

    fn reap_where(&mut self, heap: &mut Heap, include: impl Fn(&Entry) -> bool) {
        self.entries.retain(|entry| {
            if !include(entry) || entry.handle.awaiting_exit() {
                return true;
            }
            // Cancellation skips Thread.run cleanup and releases the same registered roots.
            if let Some(thread) = &entry.thread
                && entry.handle.ident() != 0
                && !thread.clear_target(heap)
            {
                return true;
            }
            for &root in entry.roots() {
                heap.weak_edge_removed(root);
            }
            false
        });
    }

    pub(super) fn reap_finished(&mut self, heap: &mut Heap) {
        self.reap_where(heap, |_| true);
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
            .filter(|entry| entry.thread.as_ref().is_some_and(|worker| !worker.daemon))
            .map(|entry| &entry.handle)
            .filter(move |handle| handle.ident() != caller)
    }

    fn join_non_daemon(&self) {
        for handle in self.shutdown_handles() {
            // Explicit detach completes Python joins before the real worker exits.
            // Non-daemon shutdown still waits for its final bootstrap references.
            handle.wait(None, State::worker_pending);
            drop(handle.joined());
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
        wait: Acquire,
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
pub(super) fn shutdown(ctx: &mut Context) {
    // An exception in the Python handler can deliberately prevent a non-daemon join.
    // Do not repeat that join in Drop or include workers started later by ordinary atexit.
    ctx.workers.joined_at_shutdown = true;
    let mark = ctx.heap.temporary_mark();
    let result = (|| -> Result<()> {
        let key = ctx.text("threading");
        if let Some(module) = ctx.initialized_registry_entry(key)? {
            let shutdown = ctx.lookup_attr(module, "_shutdown")?;
            ctx.call(shutdown, Vec::new(), Vec::new())?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        super::unraisable::report(
            ctx,
            error,
            false,
            None,
            "Exception ignored on threading shutdown",
        );
    }
    ctx.heap.release_temporaries_since(mark);
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
    fn publish(self) -> Result<()> {
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
    count: Arc<AtomicUsize>,
    fault_workers: Option<(Arc<()>, Arc<AtomicBool>)>,
}

impl Start {
    fn new(
        ctx: &Context,
        operation: Operation,
        registration: Registration,
        operands: Vec<Id>,
    ) -> Self {
        let fault_workers = matches!(operation, Operation::Python(_)).then(|| {
            (
                ctx.heap.begin_fault_worker(),
                Arc::clone(&ctx.interpreter.finalizing),
            )
        });
        Self {
            operation,
            registration,
            operands,
            count: Arc::clone(&ctx.interpreter.worker_count),
            fault_workers,
        }
    }
}

fn launch(request: Start) -> Result<(Ready, Entry)> {
    let (handle, thread, detached_label) = match request.registration {
        Registration::Thread { handle, thread, .. } => (handle, Some(thread), None),
        Registration::Detached(label) => (Arc::default(), None, Some(label)),
    };
    // The child's identity can arrive before the creator receives it. Dropping
    // the queued publication cancels the worker through the same gate.
    let (ready, started) = mpsc::sync_channel(1);
    let starting = Arc::clone(&handle);
    handle.spawn(move |exit| {
        let (fault_workers, interpreter) = request.fault_workers.unzip();
        let _publication = interpreter.map(crate::fault_handler::FramePublication::enter);
        // A failed bootstrap publishes no identity and contributes nothing to the live count.
        match request.operation {
            Operation::Python(callback) => {
                let (mut worker, function, args, kwargs) = match callback.load() {
                    Ok(call) => call,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return Ok(());
                    }
                };
                worker
                    .heap
                    .observe_fault_workers(fault_workers.as_ref().expect("Python worker"));
                if !exit.enter(request.count, starting, &ready, None) {
                    return Ok(());
                }
                let result = match worker.prepare_call(function, args, kwargs) {
                    // SAFETY: the ABI pointer comes from the Context owned by this
                    // pthread. Preparation's borrow has ended and no other code can
                    // move or access the Context until this frame returns.
                    Ok(call) => unsafe { call.run(&raw mut worker) },
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    if let Some(label) = detached_label {
                        if error.kind != "SystemExit" {
                            let _ = writeln!(
                                crate::sys::stderr_writer(),
                                "Exception ignored in thread started by {label}:\n{}",
                                error.report(&worker.heap)
                            );
                        }
                    } else {
                        return Err(error.report(&worker.heap).to_string());
                    }
                }
            }
            Operation::Empty => {
                exit.enter(request.count, starting, &ready, None);
            }
            Operation::UpdateHash { hash, data } => {
                if exit.enter(request.count, starting, &ready, None) {
                    hash.lock()
                        .update(&data)
                        .map_err(|error| error.to_string())?;
                }
            }
            Operation::Acquire { wait, release } => {
                if let Some(lock) = release {
                    let (ack, released) = mpsc::channel();
                    if !exit.enter(request.count, starting, &ready, Some(released)) {
                        return Ok(());
                    }
                    let result = lock
                        .release()
                        .map_err(|error| Error::from(error).to_string());
                    let failed = result.is_err();
                    let _ = ack.send(result);
                    if failed {
                        return Ok(());
                    }
                } else if !exit.enter(request.count, starting, &ready, None) {
                    return Ok(());
                }
                wait.run();
            }
        }
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
    ctx.workers
        .reap_where(&mut ctx.heap, |entry| entry.thread.is_none());
    ctx.workers.reserve_entry()?;
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
fn worker_target(ctx: &Context, target: Id, args: Id, kwargs: Id) -> Result<(Operation, Vec<Id>)> {
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
    if let Some(captured) = Callback::capture(ctx, target, roots, pos, Some(kwargs), true)? {
        return Ok(captured);
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
            (Value::PrimitiveLock(lock), []) if symbol == "_thread.lock.acquire" => {
                Some(Operation::Acquire {
                    wait: Acquire::blocking(lock),
                    release: None,
                })
            }
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
            } => Some((thread.owner, *handle_id)),
            Registration::Detached(_) => None,
        };
        let ready = match launch(self) {
            Ok((ready, entry)) => {
                // SAFETY: spawn runs no Python on the creator. Register the selected roots in
                // the reserved slot before a callback, wait or collection can run here.
                unsafe { &mut *context }.workers.entries.push(entry);
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
        let ready = ready.wait();
        let Some((owner, handle)) = receiver else {
            let started = ready?;
            let ident = started.ident;
            // Detached callers receive only the ident. Actual exit still owns the roots.
            started.handle.set_done()?;
            started.publish()?;
            // SAFETY: publication has returned to the caller on its own thread.
            return Ok(unsafe { &mut *context }.int(ident as i64));
        };
        let (started, ident, tid) = {
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
            (started, ident, tid)
        };
        // Ordinary writes attach the IDs to the receiver. Allocation temporaries in the
        // current frame also keep them live if an attribute hook redirects a write.
        started.publish()?;
        // SAFETY: native publication has finished and the caller again owns the context.
        Ok(unsafe { &mut *context }.alloc(Value::Tuple(vec![handle, ident, tid])))
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
    let [target, pos, kw, handle_id] = ctx.thread_operands(owner)?;
    let Value::ThreadHandle(handle) = ctx.heap.get(handle_id) else {
        return Err(Error::new("TypeError", "'handle' must be a _ThreadHandle"));
    };
    let handle = Arc::clone(handle);
    handle.check_new()?;
    let (operation, roots) = worker_target(ctx, target, pos, kw)?;
    let Value::Bool(daemon) = *ctx.heap.get(daemon) else {
        return Err(Error::new("TypeError", "expected a boolean daemon flag"));
    };
    if ctx
        .workers
        .reporting
        .is_some_and(|(scope, original)| ctx.scope_name(scope, "excepthook") != original)
    {
        return Err(Error::new(
            "NotImplementedError",
            "custom threading.excepthook requires the parent Python heap",
        ));
    }
    ctx.workers.reserve_entry()?;
    let thread = RegisteredThread { daemon, owner };
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
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    ctx.reap_thread_operands();
    match (name, args) {
        ("shutdown", &[]) => {
            // A signal handler may start a worker or reap entries. Keep only the
            // native handle across dispatch, then look up the next live worker.
            loop {
                let Some(handle) = ctx
                    .workers
                    .shutdown_handles()
                    .find(|h| h.awaiting_exit())
                    .cloned()
                else {
                    break;
                };
                while handle.wait_interruptible(None).is_continue() {
                    #[cfg(unix)]
                    // SAFETY: shutdown waits synchronously on the thread that owns the context.
                    if let Err(error) = unsafe { Context::dispatch_pending_calls(ctx) } {
                        super::unraisable::report(
                            ctx,
                            error,
                            false,
                            None,
                            "Exception ignored while joining a thread in _thread._shutdown()",
                        );
                        return Ok(NONE);
                    }
                }
                // Explicit detach completes handle joins. CPython keeps this entry
                // in its shutdown list until the worker exits, without running a
                // pending Python handler while waiting on the completed handle.
                handle.wait(None, State::worker_pending);
                drop(handle.joined());
            }
            ctx.workers.join_non_daemon();
            ctx.reap_thread_operands();
            Ok(NONE)
        }
        ("join", [_, _]) => super::thread_handle::method(ctx, "join", args, kwargs),
        ("alive", &[handle]) => {
            let Value::ThreadHandle(worker) = ctx.heap.get(handle) else {
                return Err(Error::new("TypeError", "expected a thread handle"));
            };
            let worker = Arc::clone(worker);
            Ok(boolean(worker.alive()))
        }
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
            if ctx.workers.main == Some((ident, main)) {
                return Ok(main);
            }
            ctx.workers
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
            let owners = ctx.workers.live_threads().map(|(_, owner)| owner).collect();
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
