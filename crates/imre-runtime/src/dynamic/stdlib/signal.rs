//! Signal numbers and pending signals from the host POSIX ABI.
use super::{Context, Error, Id, Result, Value, builtins};
use std::sync::OnceLock;

macro_rules! signals {
    ($($symbol:ident),+ $(,)?) => {
        vec![$((stringify!($symbol), libc::$symbol)),+]
    };
}

/// CPython orders Signals by numeric value, then by name to choose each canonical alias.
fn signal_numbers() -> &'static [(&'static str, i32)] {
    static NUMBERS: OnceLock<Vec<(&str, i32)>> = OnceLock::new();
    NUMBERS.get_or_init(|| {
        let mut numbers = signals!(
            SIGHUP, SIGINT, SIGQUIT, SIGILL, SIGTRAP, SIGABRT, SIGIOT, SIGBUS, SIGFPE, SIGKILL,
            SIGUSR1, SIGSEGV, SIGUSR2, SIGPIPE, SIGALRM, SIGTERM, SIGCHLD, SIGCONT, SIGSTOP,
            SIGTSTP, SIGTTIN, SIGTTOU, SIGURG, SIGXCPU, SIGXFSZ, SIGVTALRM, SIGPROF, SIGWINCH,
            SIGIO, SIGSYS
        );
        #[cfg(target_os = "linux")]
        {
            numbers.extend(signals!(SIGPOLL, SIGPWR));
            // musl omits the legacy SIGCLD spelling. Linux MIPS and SPARC have SIGEMT instead of SIGSTKFLT.
            #[cfg(any(target_env = "gnu", target_env = "uclibc"))]
            numbers.push(("SIGCLD", libc::SIGCHLD));
            #[cfg(any(
                target_arch = "mips",
                target_arch = "mips32r6",
                target_arch = "mips64",
                target_arch = "mips64r6",
                target_arch = "sparc",
                target_arch = "sparc64"
            ))]
            numbers.push(("SIGEMT", libc::SIGEMT));
            #[cfg(not(any(
                target_arch = "mips",
                target_arch = "mips32r6",
                target_arch = "mips64",
                target_arch = "mips64r6",
                target_arch = "sparc",
                target_arch = "sparc64"
            )))]
            numbers.push(("SIGSTKFLT", libc::SIGSTKFLT));
            numbers.push(("SIGRTMIN", libc::SIGRTMIN()));
            numbers.push(("SIGRTMAX", libc::SIGRTMAX()));
        }
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        numbers.extend(signals!(SIGINFO, SIGEMT));
        numbers.sort_unstable_by_key(|&(name, number)| (number, name));
        numbers
    })
}

/// Populate the module dictionary so imports, attribute changes, and dir share the same bindings.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    bind_numbers(ctx, scope);
    let nsig = ctx.int(i64::from(crate::signals::nsig()));
    ctx.bind(scope, "NSIG", nsig);
    let alarm = ctx.builtin("_signal.alarm");
    ctx.bind(scope, "alarm", alarm);
    let pending = ctx.builtin("_signal.sigpending");
    ctx.bind(scope, "sigpending", pending);
}

fn bind_numbers(ctx: &mut Context, scope: Id) {
    for &(name, number) in signal_numbers() {
        let value = ctx.int(i64::from(number));
        ctx.bind(scope, name, value);
    }
}

/// Build signal.Signals in target-ABI order using the ordinary IntEnum allocator.
fn make_signals(ctx: &mut Context) -> Result<Id> {
    let scope = ctx.scope("signal", None);
    let module = ctx.text("signal");
    ctx.bind(scope, "__module__", module);
    let name = ctx.text("Signals");
    ctx.bind(scope, "__qualname__", name);
    let doc = ctx.text("An enumeration.");
    ctx.bind(scope, "__doc__", doc);
    bind_numbers(ctx, scope);
    let base = ctx.builtin("enum.IntEnum");
    let class = ctx.allocate_class("Signals".into(), scope, vec![base])?;
    ctx.populate_enum(class, Some(base))?;
    Ok(class)
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
    if name == "_thread.interrupt_main" {
        let signum = match args {
            [] => libc::SIGINT,
            [number] => super::index_c_int(ctx, *number)?,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    "interrupt_main() takes at most one argument",
                ));
            }
        };
        if !(1..crate::signals::nsig()).contains(&signum) {
            return Err(Error::new("ValueError", "signal number out of range"));
        }
        crate::signals::interrupt_main(signum);
        return Ok(super::NONE);
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no arguments ({} given)", args.len()),
        ));
    }
    if name == "_imre_signal.make_signals" {
        return make_signals(ctx);
    }
    pending(ctx)
}

/// Inspect the calling thread's pending set, including blocked process-directed signals.
fn pending(ctx: &mut Context) -> Result<Id> {
    let mut mask = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigpending writes a sigset_t into caller-owned storage on success.
    if unsafe { libc::sigpending(mask.as_mut_ptr()) } != 0 {
        return Err(super::os_error(std::io::Error::last_os_error()));
    }
    let set = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    for number in 1..crate::signals::nsig() {
        // SAFETY: sigpending initialized mask, and the signal is in the host ABI range.
        if unsafe { libc::sigismember(mask.as_ptr(), number) } == 1 {
            let value = ctx.int(i64::from(number));
            builtins::set_add(ctx, set, value)?;
        }
    }
    Ok(set)
}
