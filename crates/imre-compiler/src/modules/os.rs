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
    Some(match name {
        "strerror" => Intrinsic::Strerror,
        "stat_result" => Intrinsic::StatResult,
        "close" => Intrinsic::Fd(FdOperation::Close),
        "open" => Intrinsic::Fd(FdOperation::Open),
        "unlink" => Intrinsic::Fd(FdOperation::Unlink),
        "remove" => Intrinsic::Fd(FdOperation::Remove),
        "lseek" => Intrinsic::Fd(FdOperation::Seek),
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
