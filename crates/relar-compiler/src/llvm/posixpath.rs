//! Sealed string and byte paths share the same ASCII separator rules.

use super::*;
use crate::ir::PosixPathOperation;

impl Emitter<'_, '_> {
    pub(super) fn posix_path_builtin(
        &mut self,
        operation: PosixPathOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        let first = &args[0].value;
        let bytes = u8::from(first.ty == Type::Bytes);
        let operand = match operation {
            PosixPathOperation::Join => {
                let handles = self.entry_alloca(format!("[{} x ptr]", args.len()));
                for (index, arg) in args.iter().enumerate() {
                    let slot =
                        self.instruction(format!("getelementptr ptr, ptr {handles}, i64 {index}"));
                    self.line(format!("store ptr {}, ptr {slot}", arg.value.operand));
                }
                self.instruction(format!(
                    "call ptr @relar_posix_join(ptr {handles}, i64 {}, i32 {bytes})",
                    args.len()
                ))
            }
            PosixPathOperation::MixedJoin { path_in_tail } => self.checked_instruction(format!(
                "call ptr @relar_posix_join_error(i32 {})",
                u8::from(path_in_tail)
            )),
            PosixPathOperation::Basename | PosixPathOperation::Dirname => {
                let head = u8::from(operation == PosixPathOperation::Dirname);
                self.instruction(format!(
                    "call ptr @relar_posix_component(ptr {}, i32 {bytes}, i32 {head})",
                    first.operand
                ))
            }
            PosixPathOperation::Splitext => {
                let tuple = self.record(returns);
                self.line(format!(
                    "call void @relar_posix_splitext(ptr {}, i32 {bytes}, ptr {})",
                    first.operand, tuple.operand
                ));
                return tuple;
            }
        };
        self.owned(Value {
            ty: returns,
            operand,
            small_integer: false,
        })
    }
}
