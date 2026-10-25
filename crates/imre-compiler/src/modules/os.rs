//! Names for OS descriptor operations and target-dependent open flags.
use super::Intrinsic;
use crate::ir::FdOperation;

pub(super) fn symbol(name: &str) -> Option<Intrinsic> {
    Some(match name {
        "strerror" => Intrinsic::Strerror,
        "close" => Intrinsic::Fd(FdOperation::Close),
        "open" => Intrinsic::Fd(FdOperation::Open),
        "unlink" => Intrinsic::Fd(FdOperation::Unlink),
        "remove" => Intrinsic::Fd(FdOperation::Remove),
        "O_RDONLY" => Intrinsic::OpenFlag("rdonly"),
        "O_WRONLY" => Intrinsic::OpenFlag("wronly"),
        "O_RDWR" => Intrinsic::OpenFlag("rdwr"),
        "O_APPEND" => Intrinsic::OpenFlag("append"),
        "O_CREAT" => Intrinsic::OpenFlag("creat"),
        "O_EXCL" => Intrinsic::OpenFlag("excl"),
        "O_TRUNC" => Intrinsic::OpenFlag("trunc"),
        "O_CLOEXEC" => Intrinsic::OpenFlag("cloexec"),
        "O_DIRECTORY" => Intrinsic::OpenFlag("directory"),
        "O_NOFOLLOW" => Intrinsic::OpenFlag("nofollow"),
        "O_NONBLOCK" => Intrinsic::OpenFlag("nonblock"),
        "dup" => Intrinsic::Fd(FdOperation::Dup),
        "dup2" => Intrinsic::Fd(FdOperation::Dup2),
        "pipe" => Intrinsic::Fd(FdOperation::Pipe),
        "isatty" => Intrinsic::Fd(FdOperation::Isatty),
        "get_blocking" => Intrinsic::Fd(FdOperation::GetBlocking),
        "set_blocking" => Intrinsic::Fd(FdOperation::SetBlocking),
        "get_inheritable" => Intrinsic::Fd(FdOperation::GetInheritable),
        "set_inheritable" => Intrinsic::Fd(FdOperation::SetInheritable),
        _ => return None,
    })
}
