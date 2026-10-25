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
            FdOperation::Pipe | FdOperation::Pipe2 => {
                let tuple = self.record(returns);
                match args {
                    [] => {
                        self.checked_line(format!("call void @imre_os_pipe(ptr {})", tuple.operand))
                    }
                    [flags] => self.checked_line(format!(
                        "call void @imre_os_pipe2(ptr {}, i64 {})",
                        tuple.operand, flags.value.operand
                    )),
                    _ => unreachable!(),
                }
                return tuple;
            }
            FdOperation::Close => "close",
            FdOperation::Seek => "lseek",
            FdOperation::Dup => "dup",
            FdOperation::Dup2 => "dup2",
            FdOperation::Isatty => "isatty",
            FdOperation::GetBlocking => "get_blocking",
            FdOperation::SetBlocking => "set_blocking",
            FdOperation::GetInheritable => "get_inheritable",
            FdOperation::SetInheritable => "set_inheritable",
        };
        let args = args
            .iter()
            .map(|arg| format!("i64 {}", arg.value.operand))
            .collect::<Vec<_>>()
            .join(", ");
        let repr = match returns {
            Type::None => "void",
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
