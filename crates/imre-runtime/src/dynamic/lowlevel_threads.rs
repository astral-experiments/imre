//! Count only the Python callbacks launched by this interpreter.
use super::{Context, Error, Id, Result, Value, thread_callback::Callback};
use std::sync::atomic::Ordering;

pub(super) fn function_name(symbol: &str) -> Option<&'static str> {
    match symbol {
        "_thread.start_new_thread" => Some("start_new_thread"),
        "_thread._count" => Some("_count"),
        "_thread._shutdown" => Some("_shutdown"),
        "_thread.exit" => Some("exit"),
        "_thread.exit_thread" => Some("exit_thread"),
        _ => None,
    }
}

pub(super) fn doc(function: &str) -> &'static str {
    match function {
        "start_new_thread" => {
            "Start a new thread and return its identifier.\n\nThe thread will call the function with positional arguments from the\ntuple args and keyword arguments taken from the optional dictionary\nkwargs.  The thread exits when the function returns; the return value\nis ignored.  The thread will also exit when the function raises an\nunhandled exception; a stack trace will be printed unless the exception\nis SystemExit."
        }
        "exit" => {
            "This is synonymous to ``raise SystemExit''.  It will cause the current\nthread to exit silently unless the exception is caught."
        }
        "exit_thread" => "An obsolete synonym of exit().",
        "_shutdown" => "Wait for all non-daemon threads (other than the calling thread) to stop.",
        "_count" => {
            "Return the number of currently running Python threads, excluding\nthe main thread. The returned number comprises all threads created\nthrough `start_new_thread()` as well as `threading.Thread`, and not\nyet finished.\n\nThis function is meant for internal and specialized purposes only.\nIn most applications `threading.enumerate()` should be used instead."
        }
        _ => unreachable!(),
    }
}

#[derive(Default)]
pub(super) struct Workers {
    pub(super) private_context: bool,
    // Thread errors use the original sys module when its registry entry changes.
    pub(super) system_module: Option<Id>,
    pub(super) default_unraisable_hook: Option<Id>,
    pub(super) asyncgen_hooks: [Id; 2],
    pub(super) coroutine_origin_depth: u32,
}

impl Workers {
    pub(super) fn roots(&self) -> impl Iterator<Item = Id> + '_ {
        self.system_module
            .into_iter()
            .chain(self.default_unraisable_hook)
            .chain(self.asyncgen_hooks)
    }
}

impl Context {
    /// The worker cannot route an error through an object on the caller's heap.
    pub(super) fn check_thread_reporter_mutable(&self) -> Result<()> {
        if self.workers.detached_active() {
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
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no arguments ({} given)", args.len()),
        ));
    }
    match name {
        "_thread.exit" | "_thread.exit_thread" => Err(ctx.exception("SystemExit", &[])),
        "_thread._shutdown" => super::thread_worker::call(ctx, "shutdown", args, kwargs),
        "_thread._count" => {
            let count = ctx.interpreter.worker_count.load(Ordering::SeqCst);
            ctx.workers.reap_finished(&mut ctx.heap);
            Ok(ctx.int(count as i64))
        }
        _ => unreachable!("thread startup is prepared by the registered caller"),
    }
}

pub(super) fn prepare_start(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<super::thread_worker::Start> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "start_new_thread() takes no keyword arguments",
        ));
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
    if ctx.tuple_elements(*positional).is_none() {
        return Err(Error::new("TypeError", "2nd arg must be a tuple"));
    }
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
    // The registered caller owns the original target, positional tuple and optional dict.
    let pos = ctx
        .tuple_elements(*positional)
        .expect("validated thread arguments");
    let Some((callback, roots)) = Callback::capture(ctx, *function, args, pos, keyword, false)?
    else {
        return Err(super::thread_callback::unsupported());
    };
    if let Some(sys) = ctx.threads.system_module
        && let Value::Module { scope, .. } = ctx.heap.get(sys)
        // Once exposed, the live dict can be changed through an arbitrary alias.
        && (ctx.scope_data(*scope).namespace.is_some()
            || ctx.scope_name(*scope, "unraisablehook").is_some_and(|hook| Some(hook) != ctx.threads.default_unraisable_hook)
            || ctx.heap.standard_outputs[1].is_none_or(|stderr| {
                ctx.scope_name(*scope, "stderr") != Some(stderr)
                    || matches!(ctx.heap.get(stderr), Value::Instance {
                            memory_stream: Some(super::stdlib::io::MemoryStream::Wrapper(wrapper)), ..
                        } if wrapper.buffer_id().is_none_or(|buffer| {
                            matches!(ctx.heap.get(buffer), Value::File(file) if file.closed())
                        }))
            }))
    {
        return Err(Error::new(
            "NotImplementedError",
            "custom thread exception reporting requires the boxed Python heap",
        ));
    }
    super::thread_worker::prepare_detached(ctx, callback, *function, roots)
}
