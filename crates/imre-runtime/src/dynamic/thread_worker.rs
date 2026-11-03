//! Joinable OS threads for work that can execute independently of the boxed heap.

use super::lowlevel_threads::Running;
use super::thread_callback::Callback;
use std::io::Write;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::builtins::boolean;
use super::stdlib::hashlib::digest::SharedHash;
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::locks::{PrimitiveLock, thread_ident, timeout_float, timeout_int};

type WorkerStatus = std::result::Result<(), String>;

#[derive(Default)]
struct NativeJoin {
    thread: Option<JoinHandle<WorkerStatus>>,
    failure: Option<(&'static str, String)>,
}

#[derive(Default)]
pub(super) struct Handle {
    running: Mutex<bool>,
    exiting: Condvar,
    native: Mutex<NativeJoin>,
}

// A detached worker must not keep its own join handle alive.
struct WorkerExit(Weak<Handle>);

impl Drop for WorkerExit {
    fn drop(&mut self) {
        if let Some(handle) = self.0.upgrade() {
            *handle.running.lock().unwrap() = false;
            handle.exiting.notify_all();
        }
    }
}

impl Handle {
    fn spawn(worker: impl FnOnce() -> WorkerStatus + Send + 'static) -> std::io::Result<Arc<Self>> {
        let handle = Arc::new(Self {
            running: Mutex::new(true),
            ..Self::default()
        });
        let exit = WorkerExit(Arc::downgrade(&handle));
        let thread = thread::Builder::new().spawn(move || {
            // Publish exit after the worker drops its Context, also when unwinding.
            let _exit = exit;
            worker()
        })?;
        handle.native.lock().unwrap().thread = Some(thread);
        Ok(handle)
    }

    fn running(&self) -> bool {
        *self.running.lock().unwrap()
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
        // An is_alive() caller may join first. Keep any failure for the next explicit join.
        drop(self.joined());
        false
    }

    fn join(&self, timeout: Option<Duration>) -> Result<()> {
        let running = self.running.lock().unwrap();
        let running = match timeout {
            Some(limit) => {
                self.exiting
                    .wait_timeout_while(running, limit, |r| *r)
                    .unwrap()
                    .0
            }
            None => self.exiting.wait_while(running, |r| *r).unwrap(),
        };
        if *running {
            return Ok(());
        }
        drop(running);
        if let Some((kind, message)) = self.joined().failure.take() {
            return Err(Error::new(kind, message));
        }
        Ok(())
    }
}

struct Entry {
    handle: Arc<Handle>,
    daemon: bool,
    owner: Option<Id>,
    ident: usize,
    active: Arc<AtomicBool>,
    operands: Vec<Id>,
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
    fn thread_operands(&self, owner: Id) -> Result<[Id; 3]> {
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

    /// The worker owns no parent-heap pointers. Complete the default Thread.run
    /// cleanup on the interpreter thread, also for a finished but unjoined worker.
    pub(super) fn reap_thread_operands(&mut self) {
        for index in 0..self.workers.entries.len() {
            let entry = &self.workers.entries[index];
            let Some(owner) = entry.owner else { continue };
            if entry.active.load(Ordering::Acquire) {
                continue;
            }
            let mut done = true;
            for name in ["_target", "_args", "_kwargs"] {
                let Value::Instance { fields, .. } = self.heap.get_mut(owner) else {
                    unreachable!("a registered Thread owns its original instance")
                };
                if let Some(value) = fields.shift_remove(name) {
                    self.heap.weak_edge_removed(value);
                }
                let namespace = match self.heap.get(owner) {
                    Value::Instance { namespace, .. } => *namespace,
                    _ => unreachable!(),
                };
                if let Some(dict) = namespace.and_then(|dict| self.dict_storage(dict))
                    && let Some(position) = self.dict_entries(dict).and_then(|items| {
                        items.iter().position(|&(key,_)| {
                            matches!(self.heap.get(key), Value::Str(text) if text == name)
                        })
                    })
                {
                    let (Value::Dict(entries) | Value::DefaultDict { entries, .. }) =
                        self.heap.get_mut(dict)
                    else {
                        unreachable!("dictionary payload")
                    };
                    // Deletion may need the dictionary's first set of iterator
                    // positions. Retain the roots and retry on the next checkpoint.
                    let Ok((key, value)) = entries.remove(position) else {
                        done = false;
                        break;
                    };
                    self.heap.weak_edge_removed(key);
                    self.heap.weak_edge_removed(value);
                }
            }
            if done {
                let entry = &mut self.workers.entries[index];
                entry.owner = None;
                self.heap.weak_edge_removed(owner);
                for operand in entry.operands.drain(..) {
                    self.heap.weak_edge_removed(operand);
                }
            }
        }
        self.workers
            .entries
            .retain(|entry| entry.owner.is_some() || entry.handle.running());
    }
}

impl Registry {
    // A completed worker can never become live again. Its Python IDs may
    // already have been collected, so read them only after checking the handle.
    fn live(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|entry| entry.active.load(Ordering::Acquire))
    }

    pub(super) fn roots(&self) -> impl Iterator<Item = &Id> {
        self.entries
            .iter()
            .flat_map(|entry| entry.owner.iter().chain(&entry.operands))
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
    fn join_non_daemon(&self) {
        for worker in &self.entries {
            if !worker.daemon {
                let _ = worker.handle.join(None);
            }
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
        lock: Arc<PrimitiveLock>,
        release: Option<Arc<PrimitiveLock>>,
    },
}

struct Started {
    handle: Arc<Handle>,
    active: Arc<AtomicBool>,
    ident: usize,
    native_id: Option<u64>,
}

/// Match the interpreter's lookup of an already imported threading module at exit.
pub(super) fn shutdown(ctx: &mut Context) {
    // An exception in the Python handler can deliberately prevent a non-daemon join.
    // Do not repeat that join in Drop or include workers started later by ordinary atexit.
    ctx.workers.joined_at_shutdown = true;
    let mark = ctx.heap.temporary_mark();
    let result = (|| -> Result<()> {
        let key = ctx.text("threading");
        if let Some(module) = ctx.registered_module(key)? {
            ctx.heap.keep(module);
            if module != NONE {
                if ctx.module_initializing(module)? {
                    let wait = ctx.lookup_attr(ctx.bootstrap_module, "_lock_unlock_module")?;
                    ctx.call(wait, vec![key], Vec::new())?;
                }
                // Fetching the spec or waiting for the import can replace the original entry.
                if ctx.registered_module(key)? != Some(module) {
                    return Ok(());
                }
            }
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
    if ctx.finalizing.load(Ordering::Acquire) {
        return Err(Error::new(
            "PythonFinalizationError",
            "can't create new thread at interpreter shutdown",
        ));
    }
    Ok(())
}

/// Both public launch paths use the same bootstrap, count guard and native startup handshake.
fn launch(ctx: &Context, operation: Operation, detached_label: Option<String>) -> Result<Started> {
    let (ready, started) = mpsc::sync_channel(0);
    let (operand_bound, wait_for_operand) = match &operation {
        Operation::Python(callback) if callback.awaits_receiver() => {
            let (tx, rx) = mpsc::channel();
            (Some(tx), Some(rx))
        }
        _ => (None, None),
    };
    let count = Arc::clone(&ctx.threads.count);
    let active = Arc::new(AtomicBool::new(true));
    let worker_active = Arc::clone(&active);
    let fault_workers =
        matches!(operation, Operation::Python(_)).then(|| ctx.heap.begin_fault_worker());
    let handle = Handle::spawn(move || {
        // A failed bootstrap publishes no identity and contributes nothing to the live count.
        let mut prefix = None;
        let mut update = None;
        let (python, acquire) = match operation {
            Operation::Python(callback) => match callback.load() {
                Ok((mut worker, function, arguments)) => {
                    worker
                        .heap
                        .observe_fault_workers(fault_workers.as_ref().expect("Python worker"));
                    worker.threads.count = Arc::clone(&count);
                    worker.threads.private_context = true;
                    worker.threads.acquire_bound = operand_bound;
                    (Some((worker, function, arguments)), None)
                }
                Err(error) => {
                    let _ = ready.send(Err(error.to_string()));
                    return Ok(());
                }
            },
            Operation::Empty => (None, None),
            Operation::UpdateHash { hash, data } => {
                update = Some((hash, data));
                (None, None)
            }
            Operation::Acquire { lock, release } => {
                prefix = release;
                (None, Some(lock))
            }
        };
        let _running = Running::enter(count, worker_active);
        // Consume the nonblocking prefix and bind the final receiver before the
        // parent resumes and can rebind either closure cell.
        if let Some(lock) = prefix
            && let Err(error) = lock.release()
        {
            let _ = ready.send(Err(Error::from(error).to_string()));
            return Ok(());
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let native_id = Some(crate::objects::locks::native_thread_id());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let native_id = None;
        if ready.send(Ok((thread_ident(), native_id))).is_ok() {
            if let Some((mut worker, function, arguments)) = python {
                if let Err(error) = worker.call(function, arguments, Vec::new()) {
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
            } else if let Some(lock) = acquire {
                lock.acquire(true, None);
            } else if let Some((hash, data)) = update {
                hash.lock()
                    .update(&data)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })
    .map_err(|_| Error::new("RuntimeError", "can't start new thread"))?;
    let (ident, native_id) = match started.recv() {
        Ok(Ok(identity)) => identity,
        failure => {
            let _ = handle.join(None);
            let reason = match failure {
                Ok(Err(message)) => message,
                _ => "thread initialization failed".into(),
            };
            return Err(Error::new("RuntimeError", reason));
        }
    };
    if let Some(received) = wait_for_operand {
        // The caller is the only Python evaluator that can change this cell. An
        // early worker error also ends the wait and follows the usual reporting path.
        let _ = received.recv();
    }
    Ok(Started {
        handle,
        active,
        ident,
        native_id,
    })
}

/// A low-level caller owns only the bootstrap roots after the native worker is detached.
pub(super) fn start_detached(
    ctx: &Context,
    callback: Operation,
    label: String,
) -> Result<(usize, Arc<AtomicBool>)> {
    let worker = launch(ctx, callback, Some(label))?;
    Ok((worker.ident, worker.active))
}

/// Thread.run expands *args and **kwargs before calling the target. A bound method
/// can keep an exact input tuple during the call. An ordinary function binds its
/// arguments into a frame and releases the call containers before its body runs.
pub(super) fn expanded_call_roots(ctx: &Context, target: Id, args: Id) -> Vec<Id> {
    if matches!(ctx.heap.get(target), Value::Method { .. })
        && matches!(ctx.heap.get(args), Value::Tuple(_))
    {
        vec![target, args]
    } else {
        vec![target]
    }
}

// Validate the actual bound method and its operands before starting an OS
// thread. No Python handle, callback, or pointer into Context may cross here.
fn worker_target(ctx: &Context, target: Id, args: Id, kwargs: Id) -> Result<(Operation, Vec<Id>)> {
    if target == NONE {
        return Ok((Operation::Empty, Vec::new()));
    }
    if let Some((callback, roots)) = Callback::capture_for_thread(ctx, target, args, kwargs)? {
        return Ok((callback, roots));
    }
    if let Value::Method {
        callable, receiver, ..
    } = *ctx.heap.get(target)
        && let Value::Tuple(pos) | Value::List(pos) = ctx.heap.get(args)
        && matches!(ctx.heap.get(kwargs), Value::Dict(entries) if entries.is_empty())
    {
        let (symbol, storage) = match ctx.heap.get(callable) {
            Value::Native(symbol) => (symbol.as_str(), receiver),
            Value::HeapTypeMethod {
                owner,
                name: "update",
            } if ctx.heap.get(receiver).instance_class() == Some(*owner) => {
                ("hash.update", ctx.scalar_value(receiver))
            }
            _ => ("", receiver),
        };
        let operation = match (ctx.heap.get(storage), pos.as_slice()) {
            (Value::PrimitiveLock(lock), []) if symbol == "_thread.lock.acquire" => {
                Some(Operation::Acquire {
                    lock: Arc::clone(lock),
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
            let mut roots = expanded_call_roots(ctx, target, args);
            roots.extend_from_slice(pos);
            return Ok((operation, roots));
        }
    }
    Err(Error::new(
        "NotImplementedError",
        "this thread target cannot run independently of the boxed Python heap",
    ))
}

fn timeout(ctx: &mut Context, value: Id) -> Result<Option<Duration>> {
    if value == NONE {
        return Ok(None);
    }
    if let Value::Float(seconds) = *ctx.heap.get(value) {
        return Ok(timeout_float(true, seconds)?);
    }
    let seconds = ctx.index_value(value)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer or float",
                ctx.type_name(value)
            ),
        )
    })?;
    Ok(timeout_int(true, &seconds)?)
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
        ("start", &[owner, daemon] | &[owner, daemon, _]) => {
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
            let [target, pos, kw] = ctx.thread_operands(owner)?;
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
            ctx.workers
                .entries
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            let Started {
                handle,
                active,
                ident,
                native_id: tid,
            } = match launch(ctx, operation, None) {
                Ok(worker) => worker,
                Err(error) => {
                    // Once native startup is attempted CPython consumes the handle,
                    // even if the OS refuses to create a thread. No identity is published.
                    let consumed = ctx.alloc(Value::ThreadHandle(Arc::new(Handle::default())));
                    ctx.set_attr(owner, "_handle", consumed)?;
                    return Err(error);
                }
            };
            ctx.workers.entries.push(Entry {
                handle: Arc::clone(&handle),
                daemon,
                owner: Some(owner),
                operands: roots,
                active,
                ident,
            });
            let handle = ctx.alloc(Value::ThreadHandle(handle));
            let ident = ctx.alloc(Value::Int(crate::Int::from_usize(ident)));
            let tid = tid.map_or(NONE, |id| {
                ctx.alloc(Value::Int(crate::Int::from_usize(id as usize)))
            });
            Ok(ctx.alloc(Value::Tuple(vec![handle, ident, tid])))
        }
        ("shutdown", &[]) => {
            ctx.workers.join_non_daemon();
            ctx.reap_thread_operands();
            Ok(NONE)
        }
        ("join", &[handle, limit]) => {
            let limit = timeout(ctx, limit)?;
            let Value::ThreadHandle(worker) = ctx.heap.get(handle) else {
                return Err(Error::new("TypeError", "expected a thread handle"));
            };
            if ctx.finalizing.load(Ordering::Acquire)
                && ctx
                    .workers
                    .live()
                    .any(|entry| Arc::ptr_eq(&entry.handle, worker))
            {
                return Err(Error::new(
                    "PythonFinalizationError",
                    "cannot join thread at interpreter shutdown",
                ));
            }
            let worker = Arc::clone(worker);
            worker.join(limit)?;
            ctx.reap_thread_operands();
            Ok(NONE)
        }
        ("alive", &[handle]) => {
            let Value::ThreadHandle(worker) = ctx.heap.get(handle) else {
                return Err(Error::new("TypeError", "expected a thread handle"));
            };
            let worker = Arc::clone(worker);
            Ok(boolean(worker.alive()))
        }
        ("current", &[main]) => {
            let ident = thread_ident();
            if ctx.workers.main == Some((ident, main)) {
                return Ok(main);
            }
            ctx.workers
                .live()
                .find(|entry| entry.ident == ident)
                .and_then(|entry| entry.owner)
                .ok_or_else(|| {
                    Error::new(
                        "NotImplementedError",
                        "current thread has no owner in this interpreter",
                    )
                })
        }
        ("threads", []) => {
            let owners = ctx.workers.live().filter_map(|entry| entry.owner).collect();
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
