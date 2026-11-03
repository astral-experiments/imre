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
        let constant = match name {
            "F_LOCK" => Some(libc::F_LOCK),
            "F_TLOCK" => Some(libc::F_TLOCK),
            "F_ULOCK" => Some(libc::F_ULOCK),
            "F_TEST" => Some(libc::F_TEST),
            "POSIX_FADV_NORMAL" => Some(libc::POSIX_FADV_NORMAL),
            "POSIX_FADV_SEQUENTIAL" => Some(libc::POSIX_FADV_SEQUENTIAL),
            "POSIX_FADV_RANDOM" => Some(libc::POSIX_FADV_RANDOM),
            "POSIX_FADV_WILLNEED" => Some(libc::POSIX_FADV_WILLNEED),
            "POSIX_FADV_DONTNEED" => Some(libc::POSIX_FADV_DONTNEED),
            "POSIX_FADV_NOREUSE" => Some(libc::POSIX_FADV_NOREUSE),
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
        if let Some(constant) = constant {
            return Some(Intrinsic::IntConstant(i64::from(constant)));
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
    #[cfg(target_os = "linux")]
    match name {
        "getxattr" => return Some(Intrinsic::ObjectRuntimeExport("posix.getxattr")),
        "setxattr" => return Some(Intrinsic::ObjectRuntimeExport("posix.setxattr")),
        "listxattr" => return Some(Intrinsic::ObjectRuntimeExport("posix.listxattr")),
        "removexattr" => return Some(Intrinsic::ObjectRuntimeExport("posix.removexattr")),
        "XATTR_CREATE" => return Some(Intrinsic::IntConstant(i64::from(libc::XATTR_CREATE))),
        "XATTR_REPLACE" => return Some(Intrinsic::IntConstant(i64::from(libc::XATTR_REPLACE))),
        "XATTR_SIZE_MAX" => return Some(Intrinsic::IntConstant(65536)),
        _ => {}
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
        "abort" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.abort"),
        "access" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.access"),
        #[cfg(target_os = "linux")]
        "MFD_CLOEXEC" => Intrinsic::IntConstant(i64::from(libc::MFD_CLOEXEC)),
        #[cfg(target_os = "linux")]
        "SPLICE_F_MOVE" => Intrinsic::IntConstant(i64::from(libc::SPLICE_F_MOVE)),
        #[cfg(target_os = "linux")]
        "SPLICE_F_NONBLOCK" => Intrinsic::IntConstant(i64::from(libc::SPLICE_F_NONBLOCK)),
        #[cfg(target_os = "linux")]
        "SPLICE_F_MORE" => Intrinsic::IntConstant(i64::from(libc::SPLICE_F_MORE)),
        "splice" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.splice"),
        "memfd_create" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.memfd_create")
        }
        "posix_spawn" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.posix_spawn")
        }
        "posix_spawnp" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.posix_spawnp")
        }
        "POSIX_SPAWN_OPEN" if cfg!(target_os = "linux") => Intrinsic::IntConstant(0),
        "POSIX_SPAWN_CLOSE" if cfg!(target_os = "linux") => Intrinsic::IntConstant(1),
        "POSIX_SPAWN_DUP2" if cfg!(target_os = "linux") => Intrinsic::IntConstant(2),
        // Native module initialization probes the optional libc closefrom extension.
        "POSIX_SPAWN_CLOSEFROM" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.POSIX_SPAWN_CLOSEFROM")
        }
        "urandom" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.urandom"),
        "umask" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.umask"),
        "utime" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.utime"),
        "chdir" if cfg!(unix) => Intrinsic::Chdir,
        "fchdir" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.fchdir"),
        "fsync" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.fsync"),
        "fdatasync" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.fdatasync")
        }
        "chmod" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.chmod"),
        "fchmod" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.fchmod"),
        "chown" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.chown"),
        "fchown" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.fchown"),
        "lchown" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.lchown"),
        "mkdir" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.mkdir"),
        "mkfifo" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.mkfifo"),
        "mknod" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.mknod"),
        "sched_param" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_param")
        }
        "sched_get_priority_min" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_get_priority_min")
        }
        "sched_get_priority_max" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_get_priority_max")
        }
        "sched_yield" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_yield")
        }
        "sched_getaffinity" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_getaffinity")
        }
        "sched_setaffinity" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_setaffinity")
        }
        "sched_rr_get_interval" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_rr_get_interval")
        }
        "sched_getparam" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_getparam")
        }
        "sched_setparam" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_setparam")
        }
        "sched_setscheduler" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_setscheduler")
        }
        "sched_getscheduler" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sched_getscheduler")
        }
        "posix_fadvise" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.posix_fadvise")
        }
        "posix_fallocate" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.posix_fallocate")
        }
        "copy_file_range" if cfg!(target_os = "linux") => Intrinsic::Fd(FdOperation::CopyFileRange),
        "sendfile" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.sendfile"),
        "readlink" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.readlink"),
        "_exit" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix._exit"),
        "getppid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getppid"),
        "WIFSTOPPED" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.WIFSTOPPED"),
        "execv" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.execv"),
        "kill" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.kill"),
        "getpgrp" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getpgrp"),
        "getpgid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getpgid"),
        "setpgid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.setpgid"),
        "getsid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getsid"),
        "waitpid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.waitpid"),
        "waitstatus_to_exitcode" if cfg!(unix) => {
            Intrinsic::ObjectRuntimeExport("posix.waitstatus_to_exitcode")
        }
        "WNOHANG" if cfg!(unix) => Intrinsic::DescriptorConstant("wnohang"),
        "getgrouplist" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.getgrouplist")
        }
        "getgroups" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getgroups"),
        "cpu_count" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.cpu_count")
        }
        "getpid" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.getpid"),
        "scandir" if cfg!(any(target_os = "linux", target_os = "macos")) => {
            Intrinsic::ObjectRuntimeExport("posix.scandir")
        }
        "DirEntry" if cfg!(any(target_os = "linux", target_os = "macos")) => {
            Intrinsic::ObjectRuntimeExport("posix.DirEntry")
        }
        "major" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.major"),
        "minor" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.minor"),
        "makedev" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.makedev"),
        #[cfg(target_os = "linux")]
        "EFD_CLOEXEC" => Intrinsic::IntConstant(i64::from(libc::EFD_CLOEXEC)),
        #[cfg(target_os = "linux")]
        "EFD_NONBLOCK" => Intrinsic::IntConstant(i64::from(libc::EFD_NONBLOCK)),
        #[cfg(target_os = "linux")]
        "EFD_SEMAPHORE" => Intrinsic::IntConstant(i64::from(libc::EFD_SEMAPHORE)),
        "eventfd" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.eventfd"),
        "eventfd_read" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.eventfd_read")
        }
        "eventfd_write" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.eventfd_write")
        }
        "strerror" => Intrinsic::Strerror,
        "stat_result" => Intrinsic::StatResult,
        "fstat" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.fstat"),
        "confstr" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.confstr"),
        "confstr_names" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.confstr_names")
        }
        "sysconf" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.sysconf"),
        "sysconf_names" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.sysconf_names")
        }
        "statvfs" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.statvfs"),
        "fstatvfs" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.fstatvfs"),
        "statvfs_result" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("os.statvfs_result")
        }
        "terminal_size" => Intrinsic::ObjectRuntimeExport("os.terminal_size"),
        "close" => Intrinsic::Fd(FdOperation::Close),
        "closerange" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.closerange"),
        "read" => Intrinsic::Fd(FdOperation::Read),
        "pwritev" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.pwritev"),
        #[cfg(target_os = "linux")]
        "RWF_DSYNC" => Intrinsic::IntConstant(i64::from(libc::RWF_DSYNC)),
        #[cfg(target_os = "linux")]
        "RWF_SYNC" => Intrinsic::IntConstant(i64::from(libc::RWF_SYNC)),
        #[cfg(target_os = "linux")]
        "RWF_APPEND" => Intrinsic::IntConstant(i64::from(libc::RWF_APPEND)),
        "preadv" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.preadv"),
        #[cfg(target_os = "linux")]
        "RWF_HIPRI" => Intrinsic::IntConstant(i64::from(libc::RWF_HIPRI)),
        #[cfg(target_os = "linux")]
        "RWF_NOWAIT" => Intrinsic::IntConstant(i64::from(libc::RWF_NOWAIT)),
        "writev" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.writev"),
        "readv" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.readv"),
        "readinto" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.readinto"),
        "lockf" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.lockf"),
        "pread" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.pread"),
        "pwrite" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.pwrite"),
        "write" => Intrinsic::Fd(FdOperation::Write),
        "open" => Intrinsic::Fd(FdOperation::Open),
        "unlink" => Intrinsic::Fd(FdOperation::Unlink),
        "rename" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.rename"),
        "replace" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.replace"),
        "link" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.link"),
        "symlink" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.symlink"),
        "remove" => Intrinsic::Fd(FdOperation::Remove),
        "lseek" => Intrinsic::Fd(FdOperation::Seek),
        "truncate" if cfg!(unix) => Intrinsic::ObjectRuntimeExport("posix.truncate"),
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
        "O_PATH" if cfg!(target_os = "linux") => Intrinsic::DescriptorConstant("o_path"),
        "O_TMPFILE" if cfg!(target_os = "linux") => Intrinsic::DescriptorConstant("o_tmpfile"),
        "SEEK_SET" => Intrinsic::DescriptorConstant("seek_set"),
        "SEEK_CUR" => Intrinsic::DescriptorConstant("seek_cur"),
        "SEEK_END" => Intrinsic::DescriptorConstant("seek_end"),
        "SEEK_DATA" => Intrinsic::DescriptorConstant("seek_data"),
        "SEEK_HOLE" => Intrinsic::DescriptorConstant("seek_hole"),
        "dup" => Intrinsic::Fd(FdOperation::Dup),
        "dup2" => Intrinsic::Fd(FdOperation::Dup2),
        "posix_openpt" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.posix_openpt")
        }
        "grantpt" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.grantpt"),
        "unlockpt" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.unlockpt"),
        "ptsname" if cfg!(target_os = "linux") => Intrinsic::Fd(FdOperation::PtsName),
        "tcgetpgrp" if cfg!(target_os = "linux") => Intrinsic::Fd(FdOperation::TcGetPgrp),
        "ttyname" if cfg!(target_os = "linux") => Intrinsic::ObjectRuntimeExport("posix.ttyname"),
        "O_NOCTTY" if cfg!(target_os = "linux") => Intrinsic::DescriptorConstant("o_noctty"),
        "get_terminal_size" if cfg!(target_os = "linux") => {
            Intrinsic::ObjectRuntimeExport("posix.get_terminal_size")
        }
        "openpty" => Intrinsic::Fd(FdOperation::OpenPty),
        "pipe" => Intrinsic::Fd(FdOperation::Pipe),
        "pipe2" if cfg!(target_os = "linux") => Intrinsic::Fd(FdOperation::Pipe2),
        "device_encoding" if cfg!(unix) => Intrinsic::Fd(FdOperation::DeviceEncoding),
        "isatty" => Intrinsic::Fd(FdOperation::Isatty),
        "get_blocking" => Intrinsic::Fd(FdOperation::GetBlocking),
        "set_blocking" => Intrinsic::Fd(FdOperation::SetBlocking),
        "get_inheritable" => Intrinsic::Fd(FdOperation::GetInheritable),
        "set_inheritable" => Intrinsic::Fd(FdOperation::SetInheritable),
        _ => return None,
    })
}
