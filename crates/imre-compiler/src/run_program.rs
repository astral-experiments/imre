//! Keep the named image available to its loader, then unlink it before user code.

use std::ffi::OsString;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

use tempfile::TempDir;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) mod fatal_signals;

/// Keep the compiler's own children waitable, then restore Python's inherited action.
pub(super) struct CompileChildren(Option<libc::sigaction>);

impl CompileChildren {
    pub(super) fn prepare() -> io::Result<Self> {
        // SAFETY: sigaction initializes the output buffer without changing the disposition.
        let mut inherited = unsafe { std::mem::zeroed::<libc::sigaction>() };
        // SAFETY: a null action only queries SIGCHLD. inherited is writable for the call.
        if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut inherited) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if inherited.sa_sigaction != libc::SIG_IGN && inherited.sa_flags & libc::SA_NOCLDWAIT == 0 {
            return Ok(Self(None));
        }
        // SAFETY: a default handler, an empty mask and zero flags form a valid action.
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = libc::SIG_DFL;
        // SAFETY: sigemptyset initializes the writable mask field of this local action.
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        // SAFETY: action holds SIG_DFL, an empty mask and zero flags. A null output
        // pointer tells sigaction to discard the previous disposition.
        if unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(Some(inherited)))
    }

    pub(super) fn restore(mut self) -> io::Result<()> {
        self.reset()
    }

    fn reset(&mut self) -> io::Result<()> {
        if let Some(action) = &self.0 {
            // SAFETY: action was returned by sigaction for this signal.
            if unsafe { libc::sigaction(libc::SIGCHLD, action, std::ptr::null_mut()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            self.0 = None;
        }
        Ok(())
    }
}

impl Drop for CompileChildren {
    fn drop(&mut self) {
        let _ = self.reset();
    }
}

/// Only participating images can exchange cleanup status and reap an exec helper.
fn supports_exec(path: &Path) -> bool {
    use object::{Object, ObjectSection};

    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let reader = object::read::ReadCache::new(file);
    let Ok(image) = object::File::parse(&reader) else {
        return false;
    };
    let section = if cfg!(target_os = "macos") {
        "__imre_exec_v1"
    } else {
        ".imre.exec.v1"
    };
    let prefix = b".imre-exec-fd-";
    image.section_by_name(section).is_some_and(|section| {
        section.size() == prefix.len() as u64 && section.data().ok() == Some(prefix.as_slice())
    })
}

pub(super) fn launch(
    output: &Path,
    directory: TempDir,
    source: &Path,
    args: impl Iterator<Item = OsString>,
    python: Option<&imre_compiler::CommandFlags>,
    boxed_imports: bool,
) -> io::Result<ExitStatus> {
    let (mut control, peer) = UnixStream::pair()?;
    let fd = peer.as_raw_fd();
    // The runtime reads the kernel's executable name. argv[0] remains the source.
    let direct = cfg!(any(target_os = "linux", target_os = "macos"))
        && python.is_some_and(|flags| flags.faulthandler || supports_exec(output));
    // Older and custom images retain the wrapper. Its wait must succeed, while
    // the child still inherits the original SIGCHLD disposition.
    let legacy_wait = (python.is_some() && !direct)
        .then(CompileChildren::prepare)
        .transpose()?;
    let inherited = legacy_wait.as_ref().and_then(|guard| guard.0);
    let name = if direct {
        format!(".imre-exec-fd-{fd}")
    } else {
        format!(".imre-run-fd-{fd}")
    };
    let executable = directory.path().join(&name);
    std::fs::rename(output, &executable)?;
    if python.is_none() && boxed_imports {
        // Native run shares its image with build, which resolves installed sources
        // beside the program. Make the launcher's archive visible at that location.
        let archive = super::installation_file("imre-stdlib.zip")?;
        let target = directory.path().join("imre-stdlib.zip");
        if archive.is_file() {
            std::os::unix::fs::symlink(archive, target)?;
        } else {
            std::fs::write(target, super::STDLIB_ARCHIVE)?;
        }
    }

    let mut program = Command::new(executable);
    program.arg0(source).args(args);
    // SAFETY: the peer is live through exec. fcntl only makes this descriptor
    // inheritable in the process about to become the compiled program.
    unsafe {
        program.pre_exec(move || {
            if let Some(action) = &inherited
                && libc::sigaction(libc::SIGCHLD, action, std::ptr::null_mut()) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    };
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if direct {
        return fatal_signals::exec(program, directory, name, control, peer);
    }

    control.write_all(name.as_bytes())?;
    control.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut child = program.spawn()?;
    drop(peer);
    let started = wait_for_start(&mut control, &mut child);
    let cleanup = if matches!(started, Ok(Some(_))) {
        directory.close()
    } else {
        // A custom launcher or an older runtime can still use the original path.
        Ok(())
    };
    // Older runtimes only acknowledge startup. New ones request the original
    // vector when sys.orig_argv is reachable; argv[0] in the child remains source.
    let sent = if matches!(started, Ok(Some(2))) {
        let mut writer = io::BufWriter::new(&control);
        std::env::args_os()
            .try_for_each(|arg| {
                writer.write_all(arg.as_encoded_bytes())?;
                writer.write_all(&[0])
            })
            .and_then(|()| writer.flush())
    } else {
        Ok(())
    };
    drop(control); // EOF releases the runtime after removal.
    let status = child.wait();
    cleanup?;
    started?;
    sent?;
    status
}

fn wait_for_start(control: &mut UnixStream, child: &mut Child) -> io::Result<Option<u8>> {
    let mut ready = [0];
    loop {
        match control.read(&mut ready) {
            Ok(n) => return Ok((n == 1 && matches!(ready[0], 1 | 2)).then_some(ready[0])),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                // A nonparticipating executable can leave its socket with a
                // grandchild. Its own exit must still finish imre run.
                if child.try_wait()?.is_some() {
                    return Ok(None);
                }
            }
            Err(e) if e.kind() == ErrorKind::ConnectionReset => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}
