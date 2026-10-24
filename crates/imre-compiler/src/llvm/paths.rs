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
            PathOperation::Resolve => {
                let strict = self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_path_resolve(ptr {}, i32 {strict})",
                    args[0].value.operand,
                ));
                self.owned(Value {
                    ty: Type::Path,
                    operand,
                    small_integer: false,
                })
            }
            PathOperation::Mkdir => {
                let parents = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                let exist_ok =
                    self.instruction(format!("zext i1 {} to i32", args[3].value.operand));
                self.checked_line(format!(
                    "call void @imre_path_mkdir(ptr {}, i64 {}, i32 {parents}, i32 {exist_ok})",
                    args[0].value.operand, args[1].value.operand,
                ));
                none()
            }
            PathOperation::Unlink => {
                let missing_ok =
                    self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                self.checked_line(format!(
                    "call void @imre_path_unlink(ptr {}, i32 {missing_ok})",
                    args[0].value.operand,
                ));
                none()
            }
            PathOperation::Rmdir | PathOperation::CleanupTemporaryDirectory => {
                let name = if matches!(operation, PathOperation::Rmdir) {
                    "imre_path_rmdir"
                } else {
                    "imre_tempdir_cleanup"
                };
                self.checked_line(format!("call void @{name}(ptr {})", args[0].value.operand));
                none()
            }
            PathOperation::TemporaryDirectory => {
                let directory =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[0].value.operand));
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_tempdir_new(ptr {directory}, ptr {}, ptr {})",
                    args[1].value.operand, args[2].value.operand,
                ));
                self.owned(Value {
                    ty: Type::Str,
                    operand,
                    small_integer: false,
                })
            }
            PathOperation::Exists
            | PathOperation::IsFile
            | PathOperation::IsDir
            | PathOperation::IsAbsolute => {
                let name = match operation {
                    PathOperation::Exists => "exists",
                    PathOperation::IsFile => "is_file",
                    PathOperation::IsDir => "is_dir",
                    PathOperation::IsAbsolute => "is_absolute",
                    _ => unreachable!(),
                };
                let byte = self.instruction(format!(
                    "call i32 @imre_path_{name}(ptr {})",
                    args[0].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i32 {byte}, 0")),
                    small_integer: false,
                }
            }
            PathOperation::Iterdir | PathOperation::Glob => {
                let pattern = args.get(1).map_or("null", |arg| arg.value.operand.as_str());
                let call = format!(
                    "call ptr @imre_path_iter(ptr {}, ptr {pattern})",
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
                    "call ptr @imre_path_next(ptr {})",
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
                    PathOperation::Absolute => "imre_path_absolute",
                    PathOperation::New => "imre_path_new",
                    PathOperation::Join => "imre_path_join",
                    PathOperation::Text => "imre_path_text",
                    PathOperation::Name => "imre_path_name",
                    PathOperation::Stem => "imre_path_stem",
                    PathOperation::Suffix => "imre_path_suffix",
                    PathOperation::Parent => "imre_path_parent",
                    PathOperation::ReadText => "imre_path_read_text",
                    PathOperation::WriteText => "imre_path_write_text",
                    PathOperation::WithName => "imre_path_with_name",
                    PathOperation::ReadBytes => "imre_path_read_bytes",
                    PathOperation::WriteBytes => "imre_path_write_bytes",
                    PathOperation::Replace => "imre_path_replace",
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
