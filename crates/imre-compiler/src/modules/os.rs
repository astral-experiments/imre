//! Names for OS descriptor operations and constants supplied by the target runtime.
use super::Intrinsic;
use crate::ir::FdOperation;

pub(super) fn symbol(name: &str) -> Option<Intrinsic> {
    Some(match name {
        "strerror" => Intrinsic::Strerror,
        "close" => Intrinsic::Fd(FdOperation::Close),
        "open" => Intrinsic::Fd(FdOperation::Open),
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
        "isatty" => Intrinsic::Fd(FdOperation::Isatty),
        "get_blocking" => Intrinsic::Fd(FdOperation::GetBlocking),
        "set_blocking" => Intrinsic::Fd(FdOperation::SetBlocking),
        "get_inheritable" => Intrinsic::Fd(FdOperation::GetInheritable),
        "set_inheritable" => Intrinsic::Fd(FdOperation::SetInheritable),
        _ => return None,
    })
}
