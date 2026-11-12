//! Build the native POSIX module dictionary at import time.
use super::{Context, Id, native, scandir, stat};
use crate::dynamic::stdlib::fixed_structseq;

// Share the target's descriptor names with their boxed call dispatcher.
macro_rules! descriptor_functions {
    ($( $(#[$cfg:meta])* ($($name:literal)|+); )+) => {
        const DESCRIPTOR_GROUPS: &[&[&str]] = &[$( $(#[$cfg])* &[$($name),+] ),+];

        pub(in crate::dynamic::stdlib) fn is_descriptor_function(name: &str) -> bool {
            match name {
                $( $(#[$cfg])* $($name)|+ => true, )+
                _ => false,
            }
        }
    };
}

descriptor_functions! {
    ("access" | "open" | "getsid" | "waitpid" | "rename" | "replace" |
        "readlink" | "scandir" | "remove" | "unlink" | "rmdir" | "mkdir" | "mkfifo" | "chmod" |
        "fchmod" | "chown" | "fchown" | "lchown" | "chdir" | "umask" | "utime" | "dup" |
        "dup2" | "isatty" | "lseek" | "truncate" | "ftruncate" | "pipe" | "openpty" | "read" |
        "write" | "close" | "closerange" | "get_blocking" | "set_blocking" | "get_inheritable" |
        "set_inheritable" | "link" | "symlink");
    #[cfg(unix)]
    ("getpgid" | "setpgid" | "setgroups" | "setregid" | "setreuid" | "_exit" |
        "device_encoding" | "fchdir" | "fsync" | "pread" | "pwrite" | "readinto" | "kill" |
        "getppid" | "waitstatus_to_exitcode" | "execv" | "times" | "WIFSTOPPED");
    #[cfg(target_os = "linux")]
    ("copy_file_range" | "setresgid" | "setresuid" | "waitid" | "pidfd_open" |
        "get_terminal_size" | "tcgetpgrp" | "tcsetpgrp" | "timerfd_create" | "timerfd_gettime" |
        "timerfd_gettime_ns" | "timerfd_settime" | "timerfd_settime_ns" | "ttyname" | "preadv" |
        "pwritev" | "readv" | "writev" | "splice" | "lockf" | "fdatasync" | "mknod" |
        "posix_fadvise" | "posix_fallocate" | "sendfile");
}

pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for &name in DESCRIPTOR_GROUPS.iter().copied().flatten().chain(
        [
            "stat",
            "lstat",
            "fstat",
            "fspath",
            "_create_environ",
            "putenv",
            "unsetenv",
            "_path_normpath",
            "_path_splitroot_ex",
        ]
        .iter(),
    ) {
        let value = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, value);
    }
    #[cfg(unix)]
    for name in [
        "getcwd", "getcwdb", "strerror", "listdir", "abort", "getpid", "urandom",
    ] {
        let value = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, value);
    }
    #[cfg(target_os = "linux")]
    for name in ["statvfs", "fstatvfs", "pipe2", "_spawn_exec"] {
        let value = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, value);
    }
    for symbol in [
        scandir::ENTRY,
        stat::CLASS,
        fixed_structseq::TERMINAL_SIZE,
        #[cfg(unix)]
        fixed_structseq::TIMES_RESULT,
        #[cfg(target_os = "linux")]
        stat::VFS_CLASS,
        #[cfg(target_os = "linux")]
        fixed_structseq::WAITID_RESULT,
        #[cfg(target_os = "linux")]
        fixed_structseq::UNAME_RESULT,
    ] {
        let (_, name) = symbol.rsplit_once('.').expect("qualified POSIX builtin");
        let value = ctx.builtin(symbol);
        ctx.bind(scope, name, value);
    }
    for &(name, flag) in native::DESCRIPTOR_CONSTANTS {
        let value = ctx.int(i64::from(flag));
        ctx.bind(scope, name, value);
    }
    let environ = super::environment(ctx);
    ctx.bind(scope, "environ", environ);
}
