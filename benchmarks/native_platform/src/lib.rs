//! Benchmark-only native OS/TOML boundaries shared by Codon and the Rust oracle.
//!
//! This crate never loads Python or Relar and contains no report computation.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct Error {
    pub kind: &'static str,
    pub message: String,
}

impl Error {
    pub fn value(message: impl Into<String>) -> Self {
        Self {
            kind: "ValueError",
            message: message.into(),
        }
    }

    pub fn io(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            std::io::ErrorKind::AlreadyExists => "FileExistsError",
            std::io::ErrorKind::IsADirectory => "IsADirectoryError",
            std::io::ErrorKind::NotADirectory => "NotADirectoryError",
            _ => "OSError",
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Deserialize, Serialize)]
pub struct TestGroup {
    pub name: String,
    pub href: String,
}

/// Parse the concrete typing test-group schema with a maintained TOML parser.
pub fn test_groups(text: &str) -> Result<BTreeMap<String, TestGroup>> {
    toml::from_str(text).map_err(|error| Error::value(error.to_string()))
}

#[derive(Deserialize)]
pub struct ProcessRequest {
    pub argv: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub timeout: f64,
    pub check: bool,
}

#[derive(Serialize)]
pub struct CompletedProcess {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

fn text(bytes: Vec<u8>) -> Result<String> {
    let result = String::from_utf8(bytes).map_err(|error| Error {
        kind: "UnicodeDecodeError",
        message: error.to_string(),
    })?;
    Ok(result.replace("\r\n", "\n").replace('\r', "\n"))
}

fn nonblocking(stream: &impl AsRawFd) -> Result<()> {
    let descriptor = stream.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(Error::io(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn drain<R: Read>(stream: &mut Option<R>, output: &mut Vec<u8>) -> Result<()> {
    let mut eof = false;
    if let Some(reader) = stream {
        // Limit each turn so a continuously writing stream cannot starve the
        // other pipe, process reaping, or the monotonic deadline.
        let mut buffer = [0; 16 * 1024];
        for _ in 0..4 {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(Error::io(error)),
            }
        }
    }
    if eof {
        *stream = None;
    }
    Ok(())
}

/// Pipe EOF and child exit can arrive in either order; neither extends the deadline.
fn capture(
    child: &mut Child,
    timeout: Duration,
    seconds: f64,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    let mut out = Some(child.stdout.take().unwrap());
    let mut err = Some(child.stderr.take().unwrap());
    nonblocking(out.as_ref().unwrap())?;
    nonblocking(err.as_ref().unwrap())?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut status = None;
    let started = Instant::now();
    loop {
        drain(&mut out, &mut stdout)?;
        drain(&mut err, &mut stderr)?;
        // Draining can observe the child's final writes and EOF. Check status
        // afterward so a completed child does not incur another polling delay.
        if status.is_none() {
            status = child.try_wait().map_err(Error::io)?;
        }
        if out.is_none() && err.is_none() {
            if let Some(status) = status {
                return Ok((status, stdout, stderr));
            }
        }
        if started.elapsed() >= timeout {
            // Closing these owned descriptors is bounded even when a
            // descendant inherited a pipe and the direct child already exited.
            return Err(Error {
                kind: "TimeoutExpired",
                message: format!("command exceeded {seconds} seconds"),
            });
        }
        let mut descriptors = Vec::new();
        if let Some(stream) = &out {
            descriptors.push(libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        if let Some(stream) = &err {
            descriptors.push(libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        // Closed streams cannot wake poll; keep the child-exit race inexpensive.
        let maximum_wait = if descriptors.is_empty() { 1 } else { 20 };
        let milliseconds = remaining.as_millis().clamp(1, maximum_wait) as i32;
        let polled = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                milliseconds,
            )
        };
        if polled < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                return Err(Error::io(error));
            }
        }
    }
}

/// Capture both pipes without blocking, enforce a monotonic timeout, and reap.
pub fn run(request: ProcessRequest) -> Result<CompletedProcess> {
    if request.argv.is_empty() {
        return Err(Error::value("argv must not be empty"));
    }
    if !request.timeout.is_finite() || request.timeout <= 0.0 {
        return Err(Error::value("timeout must be finite and positive"));
    }
    let timeout = Duration::try_from_secs_f64(request.timeout)
        .map_err(|_| Error::value("timeout is out of range"))?;
    let mut child = Command::new(&request.argv[0])
        .args(&request.argv[1..])
        .current_dir(&request.cwd)
        .env_clear()
        .envs(request.env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::io)?;
    let captured = capture(&mut child, timeout, request.timeout);
    if captured.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let (status, stdout, stderr) = captured?;
    let stdout = text(stdout)?;
    let stderr = text(stderr)?;
    use std::os::unix::process::ExitStatusExt;
    let returncode = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(1));
    if request.check && returncode != 0 {
        return Err(Error {
            kind: "CalledProcessError",
            message: format!("command exited with status {returncode}"),
        });
    }
    Ok(CompletedProcess {
        returncode,
        stdout,
        stderr,
    })
}

/// Allocate a sibling staging directory; the caller owns explicit cleanup.
pub fn temporary_directory(directory: &Path, prefix: &str) -> Result<PathBuf> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(directory)
        .map(|directory| directory.keep())
        .map_err(Error::io)
}

/// Remove the staging tree without following directory symlinks.
pub fn remove_directory(directory: &Path) -> Result<()> {
    std::fs::remove_dir_all(directory).map_err(Error::io)
}

/// Resolve existing symlinks while retaining nonexisting final components.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    use std::collections::VecDeque;
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(Error::io)?.join(path)
    };
    let mut pending: VecDeque<_> = absolute
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let mut resolved = PathBuf::new();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match Path::new(&part).components().next().unwrap() {
            Component::RootDir => resolved = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                let candidate = resolved.join(&part);
                match std::fs::symlink_metadata(&candidate) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        links += 1;
                        if links > 40 {
                            return Err(Error {
                                kind: "OSError",
                                message: "too many symbolic links".into(),
                            });
                        }
                        let target = std::fs::read_link(candidate).map_err(Error::io)?;
                        for component in target.components().rev() {
                            pending.push_front(component.as_os_str().to_owned());
                        }
                    }
                    Ok(_) => resolved.push(part),
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) =>
                    {
                        resolved.push(part)
                    }
                    Err(error) => return Err(Error::io(error)),
                }
            }
        }
    }
    Ok(resolved)
}

#[derive(Deserialize)]
struct TempRequest {
    directory: String,
    prefix: String,
}

#[derive(Serialize)]
struct Reply {
    kind: &'static str,
    message: String,
    data: String,
}

fn call(operation: i32, request: &str) -> Result<String> {
    fn decode<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
        serde_json::from_str(text).map_err(|error| Error::value(error.to_string()))
    }
    match operation {
        1 => serde_json::to_string(&test_groups(request)?)
            .map_err(|error| Error::value(error.to_string())),
        2 => serde_json::to_string(&run(decode(request)?)?)
            .map_err(|error| Error::value(error.to_string())),
        3 => {
            let request: TempRequest = decode(request)?;
            Ok(
                temporary_directory(Path::new(&request.directory), &request.prefix)?
                    .to_string_lossy()
                    .into_owned(),
            )
        }
        4 => {
            remove_directory(Path::new(request))?;
            Ok(String::new())
        }
        _ => Err(Error::value("unknown platform operation")),
    }
}

/// Execute a benchmark boundary call. The returned UTF-8 JSON is owned by Rust.
///
/// # Safety
/// `request` must point to a valid NUL-terminated UTF-8 C string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_benchmark_platform_call(
    operation: i32,
    request: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if request.is_null() {
            return Err(Error::value("null platform request"));
        }
        let request = unsafe { CStr::from_ptr(request) }
            .to_str()
            .map_err(|_| Error::value("platform request is not UTF-8"))?;
        call(operation, request)
    })
    .unwrap_or_else(|_| Err(Error::value("native platform panic")));
    let reply = match result {
        Ok(data) => Reply {
            kind: "",
            message: String::new(),
            data,
        },
        Err(error) => Reply {
            kind: error.kind,
            message: error.message,
            data: String::new(),
        },
    };
    CString::new(serde_json::to_string(&reply).unwrap())
        .unwrap()
        .into_raw()
}

/// Release a result allocated by [`relar_benchmark_platform_call`].
///
/// # Safety
/// `result` must be a live returned pointer and must be freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_benchmark_platform_free(result: *mut c_char) {
    if !result.is_null() {
        drop(unsafe { CString::from_raw(result) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(script: &str) -> ProcessRequest {
        ProcessRequest {
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            cwd: "/tmp".into(),
            env: std::env::vars().collect(),
            timeout: 3.0,
            check: true,
        }
    }

    #[test]
    fn drains_stdout_and_stderr_beyond_pipe_capacity() {
        let completed = run(request(
            "head -c 131072 /dev/zero; head -c 131072 /dev/zero >&2",
        ))
        .unwrap();
        assert_eq!(completed.stdout.len(), 131072);
        assert_eq!(completed.stderr.len(), 131072);
        assert_eq!(completed.returncode, 0);
    }

    #[test]
    fn waits_for_child_after_both_streams_close() {
        let completed = run(request("exec 1>&- 2>&-; sleep 0.02")).unwrap();
        assert_eq!(completed.returncode, 0);
        assert!(completed.stdout.is_empty());
        assert!(completed.stderr.is_empty());
    }

    #[test]
    fn inherited_pipe_cannot_extend_the_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("descendant.pid");
        let mut request = request("sleep 5 & echo $! > \"$INHERITED_PID\"; exit 0");
        request.env.insert(
            "INHERITED_PID".into(),
            pid_file.to_string_lossy().into_owned(),
        );
        request.timeout = 0.1;
        let started = Instant::now();
        let result = run(request);
        let elapsed = started.elapsed();
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The subprocess contract owns the direct child. Explicitly clean up
        // this deliberately detached descendant created only by the test.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        assert!(matches!(
            result,
            Err(Error {
                kind: "TimeoutExpired",
                ..
            })
        ));
        assert!(
            elapsed < Duration::from_secs(1),
            "deadline extended by inherited pipe: {elapsed:?}"
        );
    }
}
