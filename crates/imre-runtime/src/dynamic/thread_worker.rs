//! Joinable OS threads for work that can execute independently of the boxed heap.

use super::lowlevel_threads::Running;
use super::thread_callback::Callback;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::{
    Arc,
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
type Startup = std::result::Result<(usize, Option<u64>), String>;

#[derive(Default)]
pub(super) struct Handle {
    thread: RefCell<Option<JoinHandle<WorkerStatus>>>,
    // The worker sends its identity at startup, then closes this channel at exit.
    finished: Option<mpsc::Receiver<Startup>>,
}

impl Handle {
    fn alive(&self) -> bool {
        self.thread
            .borrow()
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    fn join(&self, timeout: Option<Duration>) -> Result<()> {
        if let Some(timeout) = timeout
            && self.alive()
            && let Some(finished) = &self.finished
            && matches!(
                finished.recv_timeout(timeout),
                Err(mpsc::RecvTimeoutError::Timeout)
            )
        {
            return Ok(());
        }
        if let Some(thread) = self.thread.borrow_mut().take() {
            thread
                .join()
                .map_err(|_| Error::new("RuntimeError", "native worker panicked"))?
                .map_err(|failure| Error::new("SystemError", failure))?;
        }
        Ok(())
    }
}

struct Entry {
    handle: Rc<Handle>,
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
            .retain(|entry| entry.owner.is_some() || entry.handle.alive());
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
    /// Only workers present before ordinary exit callbacks participate in interpreter shutdown.
    pub(super) fn join_before_exit_callbacks(&mut self) {
        self.joined_at_shutdown = true;
        self.join_non_daemon();
    }

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
    handle: Rc<Handle>,
    active: Arc<AtomicBool>,
    ident: usize,
    native_id: Option<u64>,
}

/// Both public launch paths use the same bootstrap, count guard and native startup handshake.
fn launch(ctx: &Context, operation: Operation, detached_label: Option<String>) -> Result<Started> {
    let (ready, finished) = mpsc::sync_channel(0);
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
    let thread = thread::Builder::new()
        .spawn(move || {
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
                                    "Exception ignored in thread started by {label}:\n{error}"
                                );
                            }
                        } else {
                            return Err(error.to_string());
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
    let (ident, native_id) = match finished.recv() {
        Ok(Ok(identity)) => identity,
        failure => {
            let _ = thread.join();
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
        handle: Rc::new(Handle {
            thread: RefCell::new(Some(thread)),
            finished: Some(finished),
        }),
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

// Validate the actual bound method and its operands before starting an OS
// thread. No Python handle, callback, or pointer into Context may cross here.
fn worker_target(ctx: &Context, target: Id, args: Id, kwargs: Id) -> Result<(Operation, Vec<Id>)> {
    if target == NONE {
        return Ok((Operation::Empty, vec![target, args, kwargs]));
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
            return Ok((operation, vec![target, args, kwargs]));
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
        ("start", &[owner, daemon]) => {
            // The independent worker cannot publish CPython's actual Python bootstrap
            // callable and handle to an audit hook. Reject before consuming the handle.
            if !ctx.audit_hooks.is_empty() {
                return Err(Error::new(
                    "NotImplementedError",
                    "audited joinable thread startup requires the boxed Python bootstrap",
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
                    let consumed = ctx.alloc(Value::ThreadHandle(Rc::new(Handle::default())));
                    ctx.set_attr(owner, "_handle", consumed)?;
                    return Err(error);
                }
            };
            ctx.workers.entries.push(Entry {
                handle: Rc::clone(&handle),
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
        ("join", &[handle, limit]) => {
            let limit = timeout(ctx, limit)?;
            let Value::ThreadHandle(worker) = ctx.heap.get(handle) else {
                return Err(Error::new("TypeError", "expected a thread handle"));
            };
            worker.join(limit)?;
            ctx.reap_thread_operands();
            Ok(NONE)
        }
        ("alive", &[handle]) => {
            let Value::ThreadHandle(worker) = ctx.heap.get(handle) else {
                return Err(Error::new("TypeError", "expected a thread handle"));
            };
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
