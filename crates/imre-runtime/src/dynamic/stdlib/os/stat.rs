//! POSIX stat and its ten-element Python struct sequence.
use super::PathArg;
use super::{Context, Error, Id, NONE, Result, Value, builtins, c_int, native};
use std::io;

pub(in crate::dynamic) const CLASS: &str = "os.stat_result";
pub(in crate::dynamic) const FIELDS: [&str; 10] = [
    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid", "st_size", "st_atime",
    "st_mtime", "st_ctime",
];
const EXTRA: &[&str] = &[
    "st_atime",
    "st_mtime",
    "st_ctime",
    "st_atime_ns",
    "st_mtime_ns",
    "st_ctime_ns",
    "st_blksize",
    "st_blocks",
    "st_rdev",
    #[cfg(target_os = "macos")]
    "st_flags",
    #[cfg(target_os = "macos")]
    "st_gen",
    #[cfg(target_os = "macos")]
    "st_birthtime",
];

fn wrap(ctx: &mut Context, values: Vec<Id>, extras: Vec<Id>) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: Some(CLASS),
        names: FIELDS.map(str::to_owned).into(),
        values,
        extras: EXTRA.iter().copied().zip(extras).collect(),
    })
}

pub(in crate::dynamic) fn count_attribute(name: &str) -> Option<i64> {
    Some(match name {
        "n_fields" => (FIELDS.len() + EXTRA.len()) as i64,
        "n_sequence_fields" => FIELDS.len() as i64,
        "n_unnamed_fields" => 3,
        _ => return None,
    })
}

pub(in crate::dynamic::stdlib) fn construct(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [Some(sequence), attributes] =
        builtins::bind_builtin_arguments("structseq", args, kwargs, ["sequence", "dict"], 1)?
    else {
        unreachable!()
    };
    let mut values = ctx.items(sequence)?;
    let len = values.len();
    let max = FIELDS.len() + EXTRA.len();
    if !(FIELDS.len()..=max).contains(&len) {
        let (bound, limit) = if len < FIELDS.len() {
            ("least", FIELDS.len())
        } else {
            ("most", max)
        };
        return Err(Error::new(
            "TypeError",
            format!("{CLASS}() takes an at {bound} {limit}-sequence ({len}-sequence given)"),
        ));
    }
    let mut extras = values.split_off(FIELDS.len());
    let positional = extras.len();
    extras.resize(EXTRA.len(), NONE);
    if let Some(dict) = attributes {
        let entries = ctx.dict_entries(dict).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("{CLASS}() takes a dict as second arg, if any"),
            )
        })?;
        for &(key, value) in entries {
            let index = ctx
                .str_value(key)
                .ok()
                .and_then(|name| EXTRA.iter().position(|&field| field == name));
            let Some(index) = index.filter(|&i| i >= positional) else {
                return Err(Error::new(
                    "TypeError",
                    format!("{CLASS}() got duplicate or unexpected field name(s)"),
                ));
            };
            extras[index] = value;
        }
    }
    // Integer times remain the tuple elements and supply missing named times in a constructed result.
    for (slot, &fallback) in extras[..3].iter_mut().zip(&values[7..]) {
        if *slot == NONE {
            *slot = fallback;
        }
    }
    Ok(wrap(ctx, values, extras))
}

/// Rebuild the struct sequence from its visible tuple and all stored named-only fields.
pub(in crate::dynamic::stdlib) fn reduce(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "unbound method stat_result.__reduce__() needs an argument",
        ));
    };
    let Value::NamedTuple {
        class: Some(CLASS),
        values,
        extras,
        ..
    } = ctx.heap.get(receiver)
    else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '__reduce__' for '{CLASS}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "stat_result.__reduce__() takes no keyword arguments",
        ));
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "stat_result.__reduce__() takes no arguments ({} given)",
                rest.len()
            ),
        ));
    }

    let extras = extras.clone();
    let sequence = ctx.alloc(Value::Tuple(values.clone()));
    let fields = extras
        .into_iter()
        .map(|(name, value)| (ctx.text(name), value))
        .collect();
    let fields = ctx.dict(fields);
    let args = ctx.alloc(Value::Tuple(vec![sequence, fields]));
    let class = ctx.builtin(CLASS);
    Ok(ctx.alloc(Value::Tuple(vec![class, args])))
}

fn integer(ctx: &mut Context, value: impl Into<num_bigint::BigInt>) -> Id {
    ctx.alloc(Value::Int(crate::Int::new(value.into())))
}

pub(super) fn packed(ctx: &mut Context, stat: &libc::stat) -> Id {
    let values = vec![
        integer(ctx, stat.st_mode),
        integer(ctx, stat.st_ino),
        integer(ctx, stat.st_dev),
        integer(ctx, stat.st_nlink),
        integer(ctx, stat.st_uid),
        integer(ctx, stat.st_gid),
        integer(ctx, stat.st_size),
        integer(ctx, stat.st_atime),
        integer(ctx, stat.st_mtime),
        integer(ctx, stat.st_ctime),
    ];
    let times = [
        (stat.st_atime, stat.st_atime_nsec),
        (stat.st_mtime, stat.st_mtime_nsec),
        (stat.st_ctime, stat.st_ctime_nsec),
    ];
    let mut extras = Vec::with_capacity(EXTRA.len());
    for (seconds, nanos) in times {
        extras.push(ctx.alloc(Value::Float(native::float_seconds(seconds as f64, nanos))));
    }
    for (seconds, nanos) in times {
        extras.push(integer(
            ctx,
            i128::from(seconds) * 1_000_000_000 + i128::from(nanos),
        ));
    }
    extras.extend([
        integer(ctx, stat.st_blksize),
        integer(ctx, stat.st_blocks),
        integer(ctx, stat.st_rdev),
    ]);
    #[cfg(target_os = "macos")]
    extras.extend([
        integer(ctx, stat.st_flags),
        integer(ctx, stat.st_gen),
        ctx.alloc(Value::Float(native::float_seconds(
            // Match the unsigned birth time used by CPython's POSIX stat.
            stat.st_birthtime as u64 as f64,
            stat.st_birthtime_nsec,
        ))),
    ]);
    wrap(ctx, values, extras)
}

pub(super) fn warn_bool_fd(ctx: &mut Context, value: Id) -> Result<()> {
    if matches!(ctx.heap.get(value), Value::Bool(_)) {
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::RuntimeWarning,
            "bool is used as a file descriptor",
            1,
        )?;
    }
    Ok(())
}

pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let status = query(ctx, name, args, kwargs, warn_bool_fd)?;
    Ok(packed(ctx, &status))
}

/// Read one POSIX status record, retaining the warning location of the Python caller.
pub(super) fn query(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    warn_bool_fd: fn(&mut Context, Id) -> Result<()>,
) -> Result<libc::stat> {
    let mut output = std::mem::MaybeUninit::<libc::stat>::uninit();
    let (rc, filename) = if name == "fstat" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "posix.fstat() takes no keyword arguments",
            ));
        }
        let [fd] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.fstat() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        // SAFETY: this descriptor is borrowed and the stat buffer is writable.
        (unsafe { libc::fstat(fd, output.as_mut_ptr()) }, None)
    } else {
        let (path, dir, follow) = if name == "lstat" {
            let [path, dir] =
                builtins::bind_builtin_arguments(name, args, kwargs, ["path", "dir_fd"], 1)?;
            (path, dir, None)
        } else {
            let [path, dir, follow] = builtins::bind_builtin_arguments(
                name,
                args,
                kwargs,
                ["path", "dir_fd", "follow_symlinks"],
                1,
            )?;
            (path, dir, follow)
        };
        if args.len() > 1 {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{name}() takes exactly 1 positional argument ({} given)",
                    args.len()
                ),
            ));
        }
        let path = path.expect("path is required");
        let (arg, filename) = super::path_or_fd(ctx, name, path, name == "stat", warn_bool_fd)?;
        let dir = if let Some(id) = dir.filter(|&v| v != NONE) {
            warn_bool_fd(ctx, id)?;
            let index = ctx.index_value(id)?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "argument should be integer or None, not {}",
                        ctx.type_name(id)
                    ),
                )
            })?;
            native::directory_fd_number(&index)?
        } else {
            libc::AT_FDCWD
        };
        let follow = follow
            .map(|v| ctx.truth(v))
            .transpose()?
            .unwrap_or(name == "stat");
        let rc = match arg {
            PathArg::Fd(fd) => {
                if dir != libc::AT_FDCWD {
                    return Err(Error::new(
                        "ValueError",
                        "stat: can't specify dir_fd without matching path",
                    ));
                }
                if !follow && fd >= 0 {
                    return Err(Error::new(
                        "ValueError",
                        "stat: cannot use fd and follow_symlinks together",
                    ));
                }
                if fd == -1 {
                    let err =
                        crate::objects::errno_error(io::Error::from_raw_os_error(libc::EFAULT));
                    return Err(ctx.with_errno_filename(err.into(), filename)?);
                }
                // SAFETY: fstat borrows the descriptor and writes the stat on success.
                unsafe { libc::fstat(fd, output.as_mut_ptr()) }
            }
            PathArg::Name(path) => {
                let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
                // SAFETY: the pathname is NUL-terminated and the stat buffer is writable.
                unsafe { libc::fstatat(dir, path.as_ptr(), output.as_mut_ptr(), flags) }
            }
        };
        (rc, Some(filename))
    };
    if rc != 0 {
        let error = crate::objects::errno_error(io::Error::last_os_error()).into();
        return Err(match filename {
            Some(path) => ctx.with_errno_filename(error, path)?,
            None => error,
        });
    }
    // SAFETY: the successful syscall initialized this buffer.
    Ok(unsafe { output.assume_init() })
}

/// The shared immutable descriptor exposes the named field independently of the sequence index.
pub(in crate::dynamic) fn member(ctx: &mut Context, name: &str) -> Option<Id> {
    let &field = FIELDS[..7]
        .iter()
        .chain(EXTRA)
        .find(|&&field| field == name)?;
    Some(ctx.readonly_member(CLASS, field))
}

pub(in crate::dynamic) fn field_names() -> impl Iterator<Item = &'static str> {
    FIELDS.iter().chain(EXTRA).copied()
}
