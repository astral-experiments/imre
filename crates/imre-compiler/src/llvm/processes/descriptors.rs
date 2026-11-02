//! Lower public OS descriptor calls with rooted arguments.
use super::*;
use crate::ir::FdOperation;

impl Emitter<'_, '_> {
    /// Read the target's descriptor constant as a tagged integer.
    pub(super) fn os_descriptor_constant(&mut self, name: &str) -> Value {
        let operand = self.instruction(format!("load i64, ptr @imre_os_{name}"));
        Value {
            ty: Type::Int,
            operand,
            small_integer: true,
        }
    }

    pub(super) fn os_descriptor(
        &mut self,
        op: FdOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        let name = match op {
            FdOperation::Open => {
                let bytes = u32::from(args[0].value.ty == Type::Bytes);
                let dir_fd = if args[3].value.ty == Type::None {
                    "0"
                } else {
                    &args[3].value.operand
                };
                let operand = self.checked_instruction(format!(
                    "call i64 @imre_os_open(ptr {}, i32 {bytes}, i64 {}, i64 {}, i64 {dir_fd})",
                    args[0].value.operand, args[1].value.operand, args[2].value.operand
                ));
                return Value {
                    ty: Type::Int,
                    operand,
                    small_integer: true,
                };
            }
            FdOperation::Unlink | FdOperation::Remove => {
                let [path, fd] = args else { unreachable!() };
                let bytes = u32::from(path.value.ty == Type::Bytes);
                self.checked_line(format!(
                    "call void @imre_os_unlink(ptr {}, i32 {bytes}, i64 {}, i32 {})",
                    path.value.operand,
                    fd.value.operand,
                    u32::from(op == FdOperation::Remove)
                ));
                return none();
            }
            FdOperation::Pipe | FdOperation::Pipe2 | FdOperation::OpenPty => {
                let tuple = self.record(returns);
                match args {
                    [] => {
                        let call = if op == FdOperation::OpenPty {
                            "openpty"
                        } else {
                            "pipe"
                        };
                        self.checked_line(format!(
                            "call void @imre_os_{call}(ptr {})",
                            tuple.operand
                        ))
                    }
                    [flags] => self.checked_line(format!(
                        "call void @imre_os_pipe2(ptr {}, i64 {})",
                        tuple.operand, flags.value.operand
                    )),
                    _ => unreachable!(),
                }
                return tuple;
            }
            FdOperation::PtsName => {
                let (word, site) = match args {
                    [fd] => (
                        fd.value.operand.clone(),
                        "ptr null, ptr null, i64 0".to_owned(),
                    ),
                    [fd, module, filename, line] => (
                        self.instruction(format!("select i1 {}, i64 3, i64 1", fd.value.operand)),
                        format!(
                            "ptr {}, ptr {}, i64 {}",
                            module.value.operand, filename.value.operand, line.value.operand
                        ),
                    ),
                    _ => unreachable!(),
                };
                let operand = self
                    .checked_instruction(format!("call ptr @imre_os_ptsname(i64 {word}, {site})"));
                return self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                });
            }
            FdOperation::TransferFd => "transfer_fd",
            FdOperation::Read => "read",
            FdOperation::Write => "write",
            FdOperation::Close => "close",
            FdOperation::Seek => "lseek",
            FdOperation::Dup => "dup",
            FdOperation::Dup2 => "dup2",
            FdOperation::Isatty => "isatty",
            FdOperation::DeviceEncoding => "device_encoding",
            FdOperation::GetBlocking => "get_blocking",
            FdOperation::SetBlocking => "set_blocking",
            FdOperation::GetInheritable => "get_inheritable",
            FdOperation::SetInheritable => "set_inheritable",
        };
        let args = args
            .iter()
            .map(|arg| {
                let repr = if arg.value.ty == Type::Bytes {
                    "ptr"
                } else {
                    "i64"
                };
                format!("{repr} {}", arg.value.operand)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let repr = match returns {
            Type::None => "void",
            Type::Bytes => "ptr",
            Type::Bool => "i32",
            _ => "i64",
        };
        let call = format!("call {repr} @imre_os_{name}({args})");
        if returns == Type::None {
            self.checked_line(call);
            return none();
        }
        let raw = self.checked_instruction(call);
        let operand = if returns == Type::Bool {
            self.instruction(format!("trunc i32 {raw} to i1"))
        } else {
            raw
        };
        self.owned(Value {
            ty: returns,
            operand,
            small_integer: false,
        })
    }
}
