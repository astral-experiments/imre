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

pub(super) fn launch(
    output: &Path,
    directory: TempDir,
    source: &Path,
    args: impl Iterator<Item = OsString>,
) -> io::Result<ExitStatus> {
    let (mut control, peer) = UnixStream::pair()?;
    let fd = peer.as_raw_fd();
    // The runtime reads the kernel's executable name. argv[0] remains the source.
    let name = format!(".imre-run-fd-{fd}");
    let executable = directory.path().join(&name);
    std::fs::rename(output, &executable)?;
    control.write_all(name.as_bytes())?;
    control.set_read_timeout(Some(Duration::from_millis(100)))?;

    let mut program = Command::new(executable);
    program.arg0(source).args(args);
    // SAFETY: the peer is live through spawn. fcntl is async-signal-safe and
    // changes only the forked child's descriptor flags.
    unsafe {
        program.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    };
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
