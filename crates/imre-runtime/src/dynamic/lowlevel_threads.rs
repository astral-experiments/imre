//! Count only the Python callbacks launched by this interpreter.
use super::{Context, Error, Id, Result, Value, thread_callback::Callback};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) fn function_name(symbol: &str) -> Option<&'static str> {
    match symbol {
        "_thread.start_new_thread" => Some("start_new_thread"),
        "_thread._count" => Some("_count"),
        _ => None,
    }
}

pub(super) fn doc(function: &str) -> &'static str {
    match function {
        "start_new_thread" => {
            "Start a new thread and return its identifier.\n\nThe thread will call the function with positional arguments from the\ntuple args and keyword arguments taken from the optional dictionary\nkwargs.  The thread exits when the function returns; the return value\nis ignored.  The thread will also exit when the function raises an\nunhandled exception; a stack trace will be printed unless the exception\nis SystemExit."
        }
        "_count" => {
            "Return the number of currently running Python threads, excluding\nthe main thread. The returned number comprises all threads created\nthrough `start_new_thread()` as well as `threading.Thread`, and not\nyet finished.\n\nThis function is meant for internal and specialized purposes only.\nIn most applications `threading.enumerate()` should be used instead."
        }
        _ => unreachable!(),
    }
}

#[derive(Default)]
pub(super) struct Workers {
    pub(super) count: Arc<AtomicUsize>,
    pub(super) private_context: bool,
    // Startup may resume once the Python frame has bound its only captured receiver.
    pub(super) acquire_bound: Option<std::sync::mpsc::Sender<()>>,
    // Thread errors use the original sys module when its registry entry changes.
    pub(super) system_module: Option<Id>,
    pub(super) default_stderr: Option<Id>,
    pub(super) default_unraisable_hook: Option<Id>,
    running: Vec<(Arc<AtomicBool>, Vec<Id>)>,
}

impl Workers {
    pub(super) fn reap(&mut self, heap: &mut super::value::Heap) {
        self.running.retain(|(active, roots)| {
            if active.load(Ordering::Acquire) {
                return true;
            }
            for &value in roots {
                heap.weak_edge_removed(value);
            }
            false
        });
    }

    // The bootstrap tuple and bound objects remain rooted until the callback exits.
    pub(super) fn roots(&self) -> impl Iterator<Item = Id> + '_ {
        self.system_module
            .into_iter()
            .chain(self.default_stderr)
            .chain(self.default_unraisable_hook)
            .chain(
                self.running
                    .iter()
                    .flat_map(|(_, roots)| roots.iter().copied()),
            )
    }
}

impl Context {
    /// The worker cannot route an error through an object on the caller's heap.
    pub(super) fn check_thread_reporter_mutable(&self) -> Result<()> {
        if self
            .threads
            .running
            .iter()
            .any(|(active, _)| active.load(Ordering::Acquire))
        {
            return Err(Error::new(
                "NotImplementedError",
                "changing thread exception reporting while a worker runs requires the boxed Python heap",
            ));
        }
        Ok(())
    }

    pub(super) fn check_thread_reporting_namespace(&self, scope: Id) -> Result<()> {
        if self.threads.system_module.is_some_and(|sys| {
            matches!(self.heap.get(sys), Value::Module { scope: sys_scope, .. } if *sys_scope == scope)
        }) {
            self.check_thread_reporter_mutable()?;
        }
        Ok(())
    }

    pub(super) fn check_thread_reporting_binding(&self, obj: Id, name: &str) -> Result<()> {
        if matches!(name, "unraisablehook" | "stderr") && self.threads.system_module == Some(obj) {
            self.check_thread_reporter_mutable()?;
        }
        Ok(())
    }
}

pub(super) struct Running {
    count: Arc<AtomicUsize>,
    roots_active: Arc<AtomicBool>,
}
impl Running {
    pub(super) fn enter(count: Arc<AtomicUsize>, roots_active: Arc<AtomicBool>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self {
            count,
            roots_active,
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        // Release bootstrap references before publishing the lower count, even
        // when the detached OS thread is still destroying its private context.
        self.roots_active.store(false, Ordering::Release);
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
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
            format!(
                "{}() takes no keyword arguments",
                if name == "_thread._count" {
                    name
                } else {
                    "start_new_thread"
                }
            ),
        ));
    }
    if name == "_thread._count" {
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("_thread._count() takes no arguments ({} given)", args.len()),
            ));
        }
        let count = ctx.threads.count.load(Ordering::SeqCst);
        ctx.threads.reap(&mut ctx.heap);
        ctx.reap_thread_operands();
        return Ok(ctx.int(count as i64));
    }
    let [function, positional, rest @ ..] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "start_new_thread expected at least 2 arguments, got {}",
                args.len()
            ),
        ));
    };
    if rest.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "start_new_thread expected at most 3 arguments, got {}",
                args.len()
            ),
        ));
    }
    if !ctx.callable(*function) {
        return Err(Error::new("TypeError", "first arg must be callable"));
    }
    let Some(values) = ctx.tuple_items(*positional)? else {
        return Err(Error::new("TypeError", "2nd arg must be a tuple"));
    };
    let keyword = rest.first().copied();
    if keyword.is_some_and(|id| ctx.dict_storage(id).is_none()) {
        return Err(Error::new(
            "TypeError",
            "optional 3rd arg must be a dictionary",
        ));
    }
    super::stdlib::audit::event(
        ctx,
        "_thread.start_new_thread",
        &[*function, *positional, keyword.unwrap_or(super::NONE)],
    )?;
    super::thread_worker::check_start(ctx)?;
    let Some((callback, roots)) = Callback::capture(ctx, *function, *positional, &values, keyword)?
    else {
        return Err(Error::new(
            "NotImplementedError",
            "this thread callback requires the boxed Python heap",
        ));
    };
    if let Some(sys) = ctx.threads.system_module
        && let Value::Module { scope, .. } = ctx.heap.get(sys)
        // Once exposed, the live dict can be changed through an arbitrary alias.
        && (ctx.scope_data(*scope).namespace.is_some()
            || ctx.scope_name(*scope, "unraisablehook").is_some_and(|hook| Some(hook) != ctx.threads.default_unraisable_hook)
            || ctx.scope_name(*scope, "stderr") != ctx.threads.default_stderr
            || ctx.threads.default_stderr.is_some_and(|stderr| {
                matches!(ctx.heap.get(stderr), Value::TextFile(file) if file.closed())
            }))
    {
        return Err(Error::new(
            "NotImplementedError",
            "custom thread exception reporting requires the boxed Python heap",
        ));
    }
    ctx.threads.reap(&mut ctx.heap);
    ctx.threads
        .running
        .try_reserve(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let label = ctx.repr(*function);
    let (ident, roots_active) = super::thread_worker::start_detached(ctx, callback, label)?;
    ctx.threads.running.push((roots_active, roots));
    Ok(ctx.int(ident as i64))
}
