//! POSIX terminal attributes for borrowed file descriptors.
use std::io;
use std::mem::MaybeUninit;

use crate::dynamic::value::NativeModuleState;

use super::{Context, Error, Id, NONE, Result, Value, os, select};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn imre_platform_tcsasoft() -> libc::c_int;
}

/// Bind each function to the module that owns its exception type.
pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("termios", None);
    let name = ctx.text("termios");
    ctx.bind(fields, "__module__", name);
    let name = ctx.text("error");
    ctx.bind(fields, "__qualname__", name);
    ctx.bind(fields, "__doc__", NONE);
    let base = ctx.builtin("Exception");
    let class = ctx.allocate_class("error".into(), fields, vec![base])?;
    ctx.install_weakref_descriptor(class, fields);
    let Value::Module { module_state, .. } = ctx.heap.get_mut(module) else {
        unreachable!()
    };
    *module_state = Some(NativeModuleState::TermiosError(class));
    ctx.bind(scope, "error", class);

    for (name, symbol) in [
        ("tcgetattr", "termios.tcgetattr"),
        ("tcsetattr", "termios.tcsetattr"),
    ] {
        let callable = ctx.builtin(symbol);
        let method = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, method);
    }

    for &(name, _, value) in imre_ast::providers::termios::CONSTANTS {
        let value = ctx.int(value);
        ctx.bind(scope, name, value);
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the accessor returns the action modifier from the target termios header.
        let soft = ctx.int(i64::from(unsafe { imre_platform_tcsasoft() }));
        ctx.bind(scope, "TCSASOFT", soft);
    }
    Ok(())
}

/// Preserve errno before running a pending signal handler, which can raise its own exception.
fn checked(ctx: &mut Context, class: Id, result: libc::c_int) -> Result<()> {
    if result != -1 {
        return Ok(());
    }
    let errno = io::Error::last_os_error()
        .raw_os_error()
        .expect("syscall errno");
    if errno == libc::EINTR {
        ctx.dispatch_signals()?;
    }
    let number = ctx.int(i64::from(errno));
    let message = ctx.text_codepoints(crate::sys::strerror(errno)?);
    let mut error = ctx.exception("termios.error", &[number, message]);
    error.class = class;
    Err(error)
}

fn get_mode(ctx: &mut Context, class: Id, fd: libc::c_int) -> Result<libc::termios> {
    let mut mode = MaybeUninit::zeroed();
    // SAFETY: tcgetattr writes to a valid, zero-initialized termios; it borrows fd.
    checked(ctx, class, unsafe {
        libc::tcgetattr(fd, mode.as_mut_ptr())
    })?;
    // SAFETY: the call succeeded, including on systems that leave unused fields untouched.
    Ok(unsafe { mode.assume_init() })
}

fn get_attributes(ctx: &mut Context, mode: &libc::termios) -> Id {
    // SAFETY: both functions read a valid termios structure.
    let (ispeed, ospeed) = unsafe { (libc::cfgetispeed(mode), libc::cfgetospeed(mode)) };
    let cc = mode
        .c_cc
        .iter()
        .enumerate()
        .map(|(index, &byte)| {
            if mode.c_lflag & libc::ICANON == 0 && (index == libc::VMIN || index == libc::VTIME) {
                ctx.int(i64::from(byte))
            } else {
                ctx.alloc(Value::Bytes(vec![byte]))
            }
        })
        .collect();
    let mut fields = vec![
        ctx.int(mode.c_iflag as i64),
        ctx.int(mode.c_oflag as i64),
        ctx.int(mode.c_cflag as i64),
        ctx.int(mode.c_lflag as i64),
        ctx.int(ispeed as i64),
        ctx.int(ospeed as i64),
    ];
    fields.push(ctx.alloc(Value::List(cc)));
    ctx.alloc(Value::List(fields))
}

fn list(ctx: &Context, value: Id) -> Option<&[Id]> {
    match ctx.heap.get(ctx.scalar_value(value)) {
        Value::List(items) => Some(items),
        _ => None,
    }
}

fn set_attributes(
    ctx: &mut Context,
    class: Id,
    fd: libc::c_int,
    when: libc::c_int,
    attributes: Id,
) -> Result<Id> {
    if list(ctx, attributes).is_none_or(|items| items.len() != 7) {
        return Err(Error::new(
            "TypeError",
            "tcsetattr, arg 3: must be 7 element list",
        ));
    }
    // Preserve native-only fields and check the descriptor before converting elements.
    let mut mode = get_mode(ctx, class, fd)?;
    let item = |ctx: &Context, index: usize| {
        list(ctx, attributes)
            .and_then(|items| items.get(index))
            .copied()
            .ok_or_else(|| Error::new("IndexError", "list index out of range"))
    };
    // Each conversion can call __index__ and replace later list elements.
    let mut fields = [0; 6];
    for (i, field) in fields.iter_mut().enumerate() {
        *field = ctx.index_c_long(item(ctx, i)?)?;
    }
    let [iflag, oflag, cflag, lflag, ispeed, ospeed] = fields;
    mode.c_iflag = iflag as libc::tcflag_t;
    mode.c_oflag = oflag as libc::tcflag_t;
    mode.c_cflag = cflag as libc::tcflag_t;
    mode.c_lflag = lflag as libc::tcflag_t;
    let cc = list(ctx, item(ctx, 6)?)
        .filter(|cc| cc.len() == libc::NCCS)
        .ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "tcsetattr: attributes[6] must be {} element list",
                    libc::NCCS
                ),
            )
        })?;
    for (byte, &value) in mode.c_cc.iter_mut().zip(cc) {
        *byte = match ctx.heap.get(ctx.scalar_value(value)) {
            Value::Bytes(bytes) if bytes.len() == 1 => bytes[0],
            Value::Int(n) => n.to_i64().ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C long")
            })? as libc::cc_t,
            Value::Bool(value) => u8::from(*value),
            _ => {
                return Err(Error::new(
                    "TypeError",
                    "tcsetattr: elements of attributes must be bytes objects of length 1 or integers",
                ));
            }
        };
    }
    // SAFETY: mode was read from this descriptor. libc validates both speeds and when.
    unsafe {
        checked(
            ctx,
            class,
            libc::cfsetispeed(&mut mode, ispeed as libc::speed_t),
        )?;
        checked(
            ctx,
            class,
            libc::cfsetospeed(&mut mode, ospeed as libc::speed_t),
        )?;
        checked(ctx, class, libc::tcsetattr(fd, when, &mode))?;
    }
    Ok(NONE)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&module, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "termios function requires a module receiver",
        ));
    };
    let Value::Module {
        module_state: Some(NativeModuleState::TermiosError(class)),
        ..
    } = *ctx.heap.get(module)
    else {
        return Err(Error::new(
            "TypeError",
            "termios function requires a termios module",
        ));
    };
    let (file, change) = match name {
        "tcgetattr" => (
            super::builtins::one_positional_arg("termios.tcgetattr", args, kwargs)?,
            None,
        ),
        "tcsetattr" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "termios.tcsetattr() takes no keyword arguments",
                ));
            }
            let [fd, when, attributes] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!("tcsetattr expected 3 arguments, got {}", args.len()),
                ));
            };
            (*fd, Some((*when, *attributes)))
        }
        _ => unreachable!(),
    };
    os::stat::warn_bool_fd(ctx, file)?;
    let fd = select::fd(ctx, file)?;
    if let Some((when, attributes)) = change {
        let when = super::index_c_int(ctx, when)?;
        set_attributes(ctx, class, fd, when, attributes)
    } else {
        let mode = get_mode(ctx, class, fd)?;
        Ok(get_attributes(ctx, &mode))
    }
}
