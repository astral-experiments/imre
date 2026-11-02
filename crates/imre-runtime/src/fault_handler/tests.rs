use std::fs::File;
use std::io::{self, Seek};
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::thread;

use super::{dump_traceback, frame};

fn trace_file() -> File {
    rustix::fs::memfd_create(c"fault-trace", rustix::fs::MemfdFlags::CLOEXEC)
        .unwrap()
        .into()
}

fn trace(mut file: File) -> String {
    file.rewind().unwrap();
    io::read_to_string(file).unwrap()
}

#[test]
fn fork_keeps_the_callers_frames_without_republishing() {
    let child = trace_file();
    let parent = trace_file();
    let child_fd = child.as_raw_fd();
    frame(0, "survivor.py", "caller", 27);
    let (published, ready) = mpsc::sync_channel(0);
    let (release, parked) = mpsc::channel::<()>();
    let worker = thread::spawn(move || {
        frame(0, "foreign.py", "worker", 31);
        published.send(()).unwrap();
        let _ = parked.recv();
    });
    ready.recv().unwrap();

    // SAFETY: the child only calls the signal-safe reporter on a preopened fd
    // and _exit. It never allocates, joins threads or runs Rust destructors.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        dump_traceback(child_fd, true);
        dump_traceback(child_fd, false);
        // SAFETY: leave the child without invoking process cleanup.
        unsafe { libc::_exit(0) };
    }
    assert!(pid > 0);
    let mut status = 0;
    loop {
        // SAFETY: this process owns the child and status is writable.
        if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
            break;
        }
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::Interrupted
        );
    }
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    dump_traceback(parent.as_raw_fd(), true);
    drop(release);
    worker.join().unwrap();

    let caller = r#"File "survivor.py", line 27 in caller"#;
    let foreign = r#"File "foreign.py", line 31 in worker"#;
    let parent = trace(parent);
    assert!(
        parent.contains(caller) && parent.contains(foreign),
        "{parent}"
    );
    let child = trace(child);
    let (all, current) = child
        .split_once("Stack (most recent call first):\n")
        .unwrap();
    assert!(
        all.starts_with("Current thread ") && all.contains(caller) && !all.contains(foreign),
        "{all}"
    );
    assert_eq!(current.trim(), caller);
}
