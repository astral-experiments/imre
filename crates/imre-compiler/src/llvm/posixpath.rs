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
                    "call ptr @imre_posix_join(ptr {handles}, i64 {}, i32 {bytes})",
                    args.len()
                ))
            }
            PosixPathOperation::MixedJoin { path_in_tail } => {
                let mut unions = args[1..]
                    .iter()
                    .filter(|arg| matches!(arg.value.ty, Type::Union(_)));
                let flag = if path_in_tail {
                    "1".to_owned()
                } else if let Some(first) = unions.next() {
                    let path = [InstanceType::Exact(Type::Path)];
                    let mut found = self.instance_test(&first.value, &path);
                    for arg in unions {
                        let next = self.instance_test(&arg.value, &path);
                        found = self.instruction(format!("or i1 {found}, {next}"));
                    }
                    self.instruction(format!("zext i1 {found} to i32"))
                } else {
                    "0".to_owned()
                };
                self.checked_instruction(format!("call ptr @imre_posix_join_error(i32 {flag})"))
            }
            PosixPathOperation::Basename | PosixPathOperation::Dirname => {
                let head = u8::from(operation == PosixPathOperation::Dirname);
                self.instruction(format!(
                    "call ptr @imre_posix_component(ptr {}, i32 {bytes}, i32 {head})",
                    first.operand
                ))
            }
            PosixPathOperation::Splitext => {
                let tuple = self.record(returns);
                self.line(format!(
                    "call void @imre_posix_splitext(ptr {}, i32 {bytes}, ptr {})",
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
