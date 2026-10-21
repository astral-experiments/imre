//! Path values use the existing native object ownership ABI, with sealed operations.

use super::*;
use crate::ir::PathOperation;

impl Emitter<'_, '_> {
    pub(super) fn path_builtin(
        &mut self,
        operation: PathOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        match operation {
            PathOperation::Exists | PathOperation::IsFile | PathOperation::IsDir => {
                let name = match operation {
                    PathOperation::Exists => "exists",
                    PathOperation::IsFile => "is_file",
                    PathOperation::IsDir => "is_dir",
                    _ => unreachable!(),
                };
                let byte = self.instruction(format!(
                    "call i8 @relar_path_{name}(ptr {})",
                    args[0].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i8 {byte}, 0")),
                    small_integer: false,
                }
            }
            PathOperation::Iterdir | PathOperation::Glob => {
                let pattern = args.get(1).map_or("null", |arg| arg.value.operand.as_str());
                let call = format!(
                    "call ptr @relar_path_iter(ptr {}, ptr {pattern})",
                    args[0].value.operand
                );
                // iterdir scans now; validated glob scans only on its first pull.
                let operand = if matches!(operation, PathOperation::Iterdir) {
                    self.checked_instruction(call)
                } else {
                    self.instruction(call)
                };
                self.owned(Value {
                    ty: Type::Iterator,
                    operand,
                    small_integer: false,
                })
            }
            PathOperation::Next => {
                let pointer = self.checked_instruction(format!(
                    "call ptr @relar_path_next(ptr {})",
                    args[0].value.operand
                ));
                let operand = self.instruction(format!("ptrtoint ptr {pointer} to i64"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            _ => {
                let name = match operation {
                    PathOperation::New => "relar_path_new",
                    PathOperation::Join => "relar_path_join",
                    PathOperation::Text => "relar_path_text",
                    PathOperation::Name => "relar_path_name",
                    PathOperation::Stem => "relar_path_stem",
                    PathOperation::Suffix => "relar_path_suffix",
                    PathOperation::Parent => "relar_path_parent",
                    PathOperation::ReadText => "relar_path_read_text",
                    PathOperation::WriteText => "relar_path_write_text",
                    _ => unreachable!("special path ABIs were handled above"),
                };
                self.call(
                    name,
                    returns,
                    &args.iter().map(|arg| &arg.value).collect::<Vec<_>>(),
                )
            }
        }
    }
}
