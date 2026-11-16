//! Signal numbers and sets from the host POSIX ABI.
use super::{Context, Error, Id, Result, Value, builtins};

/// Populate the module dictionary so imports, attribute changes, and dir share the same bindings.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    bind_numbers(ctx, scope);
    let nsig = ctx.int(i64::from(crate::signals::nsig()));
    ctx.bind(scope, "NSIG", nsig);
    let alarm = ctx.builtin("_signal.alarm");
    ctx.bind(scope, "alarm", alarm);
    let pending = ctx.builtin("_signal.sigpending");
    ctx.bind(scope, "sigpending", pending);
    let valid = ctx.builtin("_signal.valid_signals");
    ctx.bind(scope, "valid_signals", valid);
    for name in ["pause", "set_wakeup_fd", "siginterrupt"] {
        let function = ctx.builtin(&format!("_signal.{name}"));
        ctx.bind(scope, name, function);
    }
}

fn bind_numbers(ctx: &mut Context, scope: Id) {
    for &(name, number) in crate::signals::numbers() {
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

fn set_wakeup_fd(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() + kwargs.len() <= 2 && args.len() != 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "set_wakeup_fd() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    }
    let [Some(fd), warn] = builtins::bind_method_arguments(
        "set_wakeup_fd",
        args,
        kwargs,
        ["", "warn_on_full_buffer"],
        1,
    )?
    else {
        unreachable!("one positional descriptor is required")
    };
    let warn = warn
        .map(|value| ctx.truth(value))
        .transpose()?
        .unwrap_or(true);
    let fd = super::index_c_int(ctx, fd)?;
    if !crate::signals::is_main() {
        return Err(Error::new(
            "ValueError",
            "set_wakeup_fd only works in main thread of the main interpreter",
        ));
    }
    if fd != -1 {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstat validates the borrowed descriptor and writes to this buffer.
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
            return Err(super::os_error(std::io::Error::last_os_error()));
        }
        if crate::objects::os::BLOCKING.enabled(fd)? {
            return Err(Error::new(
                "ValueError",
                format!("the fd {fd} must be in non-blocking mode"),
            ));
        }
    }
    Ok(ctx.int(i64::from(crate::signals::replace_wakeup_fd(fd, warn))))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "_signal.set_wakeup_fd" {
        return set_wakeup_fd(ctx, args, kwargs);
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    if name == "_signal.siginterrupt" {
        let [number, flag] = args else {
            return Err(Error::new(
                "TypeError",
                format!("siginterrupt expected 2 arguments, got {}", args.len()),
            ));
        };
        let number = super::index_c_int(ctx, *number)?;
        let flag = super::index_c_int(ctx, *flag)?;
        if !(1..crate::signals::nsig()).contains(&number) {
            return Err(Error::new("ValueError", "signal number out of range"));
        }
        crate::signals::set_interrupt(number, flag != 0).map_err(super::os_error)?;
        return Ok(super::NONE);
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
    if name == "_signal.pause" {
        crate::signals::imre_signal_pause();
        ctx.dispatch_signals()?;
        return Ok(super::NONE);
    }
    if name == "_imre_signal.make_signals" {
        return make_signals(ctx);
    }
    let mut mask = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: both functions initialize the caller's writable sigset_t on success.
    let status = unsafe {
        match name {
            "_signal.sigpending" => libc::sigpending(mask.as_mut_ptr()),
            "_signal.valid_signals" => libc::sigfillset(mask.as_mut_ptr()),
            _ => unreachable!(),
        }
    };
    if status != 0 {
        return Err(super::os_error(std::io::Error::last_os_error()));
    }
    let set = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    for number in 1..crate::signals::nsig() {
        // SAFETY: the mask was initialized above, and the signal is in the host ABI range.
        if unsafe { libc::sigismember(mask.as_ptr(), number) } == 1 {
            let value = ctx.int(i64::from(number));
            builtins::set_add(ctx, set, value)?;
        }
    }
    Ok(set)
}
