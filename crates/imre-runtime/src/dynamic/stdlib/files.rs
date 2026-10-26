use std::io::{BufRead, Read, Seek, SeekFrom};
use std::process::{Command, Stdio};

use super::{Context, Error, Id, NONE, Result, TRUE, Value, argument, instance, option, os_error};

fn join(left: &str, right: &str) -> String {
    if right.starts_with('/') || left.is_empty() {
        return right.to_owned();
    }
    if left.ends_with('/') {
        format!("{left}{right}")
    } else {
        format!("{left}/{right}")
    }
}

fn dirname(path: &str) -> &str {
    let head = path.rfind('/').map_or("", |i| &path[..=i]);
    if head.trim_start_matches('/').is_empty() {
        head
    } else {
        head.trim_end_matches('/')
    }
}

fn abspath(path: &str) -> std::io::Result<String> {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        let cwd = std::env::current_dir()?;
        join(
            cwd.to_str()
                .ok_or_else(|| std::io::Error::other("non-UTF-8 working directory"))?,
            path,
        )
    };
    let prefix = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Ok(format!("{prefix}{}", parts.join("/")))
}

fn run_process(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let command = argument(args, 0)?;
    let items = ctx.items(command)?;
    let argv = items
        .iter()
        .map(|&id| ctx.str_value(id))
        .collect::<Result<Vec<_>>>()?;
    let (executable, rest) = argv
        .split_first()
        .ok_or_else(|| Error::new("ValueError", "empty subprocess argument list"))?;
    let named = |name| {
        kwargs
            .iter()
            .find(|(key, _)| key == name)
            .map(|&(_, id)| id)
    };
    let capture = named("capture_output") == Some(TRUE);
    let piped = |name| -> Result<bool> {
        match named(name) {
            None | Some(NONE) => Ok(capture),
            Some(_) if capture => Err(Error::new(
                "ValueError",
                "stdout and stderr arguments may not be used with capture_output",
            )),
            Some(id) if ctx.as_integer(id)? == -1 => Ok(true),
            _ => Err(Error::new("ValueError", "expected subprocess.PIPE")),
        }
    };
    let stdout = piped("stdout")?;
    let stderr = piped("stderr")?;
    let stdio = |captured| {
        if captured {
            Stdio::piped()
        } else {
            Stdio::inherit()
        }
    };
    let output = Command::new(executable)
        .args(rest)
        .stdout(stdio(stdout))
        .stderr(stdio(stderr))
        .output()
        .map_err(os_error)?;
    let exit_code = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        -output.status.signal().unwrap_or(1)
    });
    if named("check") == Some(TRUE) && exit_code != 0 {
        return Err(Error::new(
            "CalledProcessError",
            format!("subprocess exited with status {exit_code}"),
        ));
    }
    let text = named("text") == Some(TRUE);
    let mut stream = |captured: bool, bytes: Vec<u8>| -> Result<Id> {
        if !captured {
            return Ok(NONE);
        }
        if text {
            let text = String::from_utf8(bytes)
                .map_err(|err| Error::new("UnicodeDecodeError", err.to_string()))?;
            Ok(ctx.text(text.replace("\r\n", "\n").replace('\r', "\n")))
        } else {
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
    };
    let stdout = stream(stdout, output.stdout)?;
    let stderr = stream(stderr, output.stderr)?;
    let status = ctx.int(i64::from(exit_code));
    Ok(instance(
        ctx,
        "subprocess.CompletedProcess",
        &[
            ("args", command),
            ("returncode", status),
            ("stdout", stdout),
            ("stderr", stderr),
        ],
    ))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let first = argument(args, 0)?;
    match name {
        "os.path.join" => {
            let mut path = ctx.str_value(first)?.to_owned();
            for &part in &args[1..] {
                path = join(&path, ctx.str_value(part)?);
            }
            return Ok(ctx.text(path));
        }
        "os.path.abspath" => return Ok(ctx.text(abspath(ctx.str_value(first)?).map_err(os_error)?)),
        "os.path.dirname" => return Ok(ctx.text(dirname(ctx.str_value(first)?).to_owned())),
        "subprocess.run" => return run_process(ctx, args, kwargs),
        _ => {}
    }
    let Value::File(handle) = ctx.heap.get(first) else {
        return Err(Error::new("TypeError", "expected a file"));
    };
    let handle = handle.clone();
    let mut slot = handle.borrow_mut();
    if matches!(name, "file.__exit__" | "file.close") {
        slot.take();
        return Ok(NONE);
    }
    let f = slot
        .as_mut()
        .ok_or_else(|| Error::new("ValueError", "I/O operation on closed file"))?;
    match name {
        "file.__enter__" => Ok(first),
        "file.tell" => Ok(ctx.int(f.stream_position().map_err(os_error)? as i64)),
        "file.seek" => {
            let offset = ctx.as_integer(argument(args, 1)?)?;
            let whence = option(args, 2, kwargs, "whence")?
                .map(|id| ctx.as_integer(id))
                .transpose()?
                .unwrap_or(0);
            let pos = match whence {
                0 => SeekFrom::Start(
                    u64::try_from(offset)
                        .map_err(|_| Error::new("OSError", "invalid negative seek position"))?,
                ),
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return Err(Error::new("ValueError", "invalid seek whence")),
            };
            Ok(ctx.int(f.seek(pos).map_err(os_error)? as i64))
        }
        "file.read" | "file.readline" => {
            let length = args
                .get(1)
                .map(|&id| ctx.as_integer(id))
                .transpose()?
                .unwrap_or(-1);
            let mut bytes = Vec::new();
            let mut input = f.take(u64::try_from(length).unwrap_or(u64::MAX));
            if name == "file.readline" {
                input.read_until(b'\n', &mut bytes)
            } else {
                input.read_to_end(&mut bytes)
            }
            .map_err(os_error)?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown native file method {name}"),
        )),
    }
}
