//! Names for OS descriptor operations and constants supplied by the target runtime.
use super::Intrinsic;
use crate::ir::FdOperation;

/// Positional binding and protocol discovery use the same POSIX signature.
pub(crate) fn credential_arity(name: &str) -> usize {
    if name.starts_with("get") {
        0
    } else if name.starts_with("setres") {
        3
    } else if name.starts_with("setre") {
        2
    } else {
        1
    }
}

pub(super) fn symbol(name: &str) -> Option<Intrinsic> {
    #[cfg(target_os = "linux")]
    {
        let policy = match name {
            "SCHED_OTHER" => Some(libc::SCHED_OTHER),
            "SCHED_NORMAL" => Some(libc::SCHED_NORMAL),
            "SCHED_FIFO" => Some(libc::SCHED_FIFO),
            "SCHED_RR" => Some(libc::SCHED_RR),
            "SCHED_BATCH" => Some(libc::SCHED_BATCH),
            "SCHED_IDLE" => Some(libc::SCHED_IDLE),
            "SCHED_DEADLINE" => Some(libc::SCHED_DEADLINE),
            "SCHED_RESET_ON_FORK" => Some(libc::SCHED_RESET_ON_FORK),
            _ => None,
        };
        if let Some(policy) = policy {
            return Some(Intrinsic::IntConstant(i64::from(policy)));
        }
    }
    if cfg!(unix) {
        let common = [
            "getuid", "geteuid", "getgid", "getegid", "setuid", "seteuid", "setgid", "setegid",
            "setreuid", "setregid",
        ];
        let saved = ["getresuid", "getresgid", "setresuid", "setresgid"];
        if let Some(known) = common
            .into_iter()
            .chain(saved.into_iter().filter(|_| cfg!(target_os = "linux")))
            .find(|known| *known == name)
        {
            return Some(Intrinsic::Credential(known));
        }
    }
    #[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
    if name.starts_with("RTLD_") {
        let flag = match name {
            "RTLD_LAZY" => libc::RTLD_LAZY,
            "RTLD_NOW" => libc::RTLD_NOW,
            "RTLD_GLOBAL" => libc::RTLD_GLOBAL,
            "RTLD_LOCAL" => libc::RTLD_LOCAL,
            "RTLD_NODELETE" => libc::RTLD_NODELETE,
            "RTLD_NOLOAD" => libc::RTLD_NOLOAD,
            #[cfg(target_env = "gnu")]
            "RTLD_DEEPBIND" => libc::RTLD_DEEPBIND,
            _ => return None,
        };
        return Some(Intrinsic::IntConstant(i64::from(flag)));
    }
    Some(match name {
        "access" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.access"),
        "urandom" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.urandom"),
        "utime" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.utime"),
        "chdir" if cfg!(unix) => Intrinsic::Chdir,
        "chmod" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.chmod"),
        "fchmod" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.fchmod"),
        "mkdir" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.mkdir"),
        "mkfifo" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.mkfifo"),
        "sched_param" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_param")
        }
        "sched_getscheduler" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_getscheduler")
        }
        "sendfile" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.sendfile"),
        "readlink" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.readlink"),
        "getppid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getppid"),
        "kill" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.kill"),
        "getsid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getsid"),
        "waitpid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.waitpid"),
        "WNOHANG" if cfg!(unix) => Intrinsic::DescriptorConstant("wnohang"),
        "getpid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getpid"),
        "scandir" if cfg!(any(target_os = "linux", target_os = "macos")) => {
            Intrinsic::ObjectRuntimeExport("posix.scandir")
        }
        "DirEntry" if cfg!(any(target_os = "linux", target_os = "macos")) => {
            Intrinsic::ObjectRuntimeExport("posix.DirEntry")
        }
        "strerror" => Intrinsic::Strerror,
        "stat_result" => Intrinsic::StatResult,
        "statvfs" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.statvfs"),
        "fstatvfs" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.fstatvfs"),
        "statvfs_result" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("os.statvfs_result")
        }
        "terminal_size" => Intrinsic::ObjectRuntimeExport("os.terminal_size"),
        "close" => Intrinsic::Fd(FdOperation::Close),
        "read" => Intrinsic::Fd(FdOperation::Read),
        "write" => Intrinsic::Fd(FdOperation::Write),
        "open" => Intrinsic::Fd(FdOperation::Open),
        "unlink" => Intrinsic::Fd(FdOperation::Unlink),
        "rename" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.rename"),
        "replace" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.replace"),
        "link" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.link"),
        "symlink" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.symlink"),
        "remove" => Intrinsic::Fd(FdOperation::Remove),
        "lseek" => Intrinsic::Fd(FdOperation::Seek),
        "ftruncate" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.ftruncate"),
        "O_RDONLY" => Intrinsic::DescriptorConstant("o_rdonly"),
        "O_WRONLY" => Intrinsic::DescriptorConstant("o_wronly"),
        "O_RDWR" => Intrinsic::DescriptorConstant("o_rdwr"),
        "O_APPEND" => Intrinsic::DescriptorConstant("o_append"),
        "O_CREAT" => Intrinsic::DescriptorConstant("o_creat"),
        "O_EXCL" => Intrinsic::DescriptorConstant("o_excl"),
        "O_TRUNC" => Intrinsic::DescriptorConstant("o_trunc"),
        "O_CLOEXEC" => Intrinsic::DescriptorConstant("o_cloexec"),
        "O_DIRECTORY" => Intrinsic::DescriptorConstant("o_directory"),
        "O_NOFOLLOW" => Intrinsic::DescriptorConstant("o_nofollow"),
        "O_NONBLOCK" => Intrinsic::DescriptorConstant("o_nonblock"),
        "O_TMPFILE" if cfg!(target_os = "linux") => Intrinsic::DescriptorConstant("o_tmpfile"),
        "SEEK_SET" => Intrinsic::DescriptorConstant("seek_set"),
        "SEEK_CUR" => Intrinsic::DescriptorConstant("seek_cur"),
        "SEEK_END" => Intrinsic::DescriptorConstant("seek_end"),
        "SEEK_DATA" => Intrinsic::DescriptorConstant("seek_data"),
        "SEEK_HOLE" => Intrinsic::DescriptorConstant("seek_hole"),
        "dup" => Intrinsic::Fd(FdOperation::Dup),
        "dup2" => Intrinsic::Fd(FdOperation::Dup2),
        "pipe" => Intrinsic::Fd(FdOperation::Pipe),
        "pipe2" if cfg!(target_os = "linux") => Intrinsic::Fd(FdOperation::Pipe2),
        "isatty" => Intrinsic::Fd(FdOperation::Isatty),
        "get_blocking" => Intrinsic::Fd(FdOperation::GetBlocking),
        "set_blocking" => Intrinsic::Fd(FdOperation::SetBlocking),
        "get_inheritable" => Intrinsic::Fd(FdOperation::GetInheritable),
        "set_inheritable" => Intrinsic::Fd(FdOperation::SetInheritable),
        _ => return None,
    })
}
