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
            PathOperation::Drive | PathOperation::Root | PathOperation::Anchor => {
                let part = match operation {
                    PathOperation::Drive => 0,
                    PathOperation::Root => 1,
                    _ => 2,
                };
                let operand = self.instruction(format!(
                    "call ptr @imre_path_prefix(ptr {}, i32 {part})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            PathOperation::PureNew(flavor) => {
                let windows = u32::from(flavor == crate::ir::PathFlavor::Windows);
                let operand =
                    self.instruction(format!("call ptr @imre_pure_path_new(i32 {windows})"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            PathOperation::WriteTypeError => self.call(
                "imre_path_write_type_error",
                Type::Int,
                &[&args.last().unwrap().value],
            ),
            PathOperation::Match { full } => {
                let name = if args[1].value.ty.is_path() {
                    "imre_path_match_object"
                } else {
                    "imre_path_match"
                };
                let matched = self.checked_instruction(format!(
                    "call i32 @{name}(ptr {}, ptr {}, i64 {}, i32 {})",
                    args[0].value.operand,
                    args[1].value.operand,
                    args[2].value.operand,
                    u32::from(full)
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i32 {matched}, 0")),
                    small_integer: false,
                }
            }
            PathOperation::ContextNew { temporary } => {
                let operand = self.instruction(format!(
                    "call ptr @imre_directory_context_new(i64 {}, i32 {})",
                    args[0].value.operand,
                    u32::from(temporary)
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            PathOperation::ContextEnter => {
                let value = self.instruction(format!(
                    "call i64 @imre_directory_context_enter(ptr {})",
                    args[0].value.operand
                ));
                let concrete = self.module.types.payload(returns).unwrap_or(returns);
                let word = if concrete == Type::Path {
                    self.instruction(format!("and i64 {value}, -8"))
                } else {
                    value
                };
                self.unpack_word(word, returns)
            }
            PathOperation::ContextExit => {
                self.checked_line(format!(
                    "call void @imre_directory_context_exit(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            PathOperation::UnionText => {
                self.call("imre_union_path_text", Type::Str, &[&args[0].value])
            }
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
            PathOperation::OsExists
            | PathOperation::Exists
            | PathOperation::IsFile
            | PathOperation::IsDir
            | PathOperation::IsAbsolute => {
                let name = match operation {
                    PathOperation::OsExists => "text_exists",
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
            PathOperation::Iterdir | PathOperation::Glob { .. } => {
                let path = &args[0].value.operand;
                let call = if let PathOperation::Glob { recursive } = operation {
                    let follow =
                        self.instruction(format!("zext i1 {} to i32", args[3].value.operand));
                    format!(
                        "call ptr @imre_path_glob(ptr {path}, ptr {}, i64 {}, i32 {follow}, i32 {})",
                        args[1].value.operand,
                        args[2].value.operand,
                        u32::from(recursive)
                    )
                } else {
                    format!("call ptr @imre_path_iter(ptr {path}, ptr null)")
                };
                let operand = self.checked_instruction(call);
                self.owned(Value {
                    ty: Type::PathIterator,
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
                    PathOperation::Pull => "imre_path_next_required",
                    PathOperation::Scripts => "imre_sysconfig_scripts",
                    PathOperation::Absolute => "imre_path_absolute",
                    PathOperation::New => "imre_path_new",
                    PathOperation::Join => "imre_path_join",
                    PathOperation::JoinObject => "imre_path_join_object",
                    PathOperation::AsPosix => "imre_path_as_posix",
                    PathOperation::WithName => "imre_path_with_name",
                    PathOperation::Text => "imre_path_text",
                    PathOperation::Name => "imre_path_name",
                    PathOperation::CountPart => "imre_path_count_part",
                    PathOperation::Stem => "imre_path_stem",
                    PathOperation::Suffix => "imre_path_suffix",
                    PathOperation::Parent => "imre_path_parent",
                    PathOperation::ReadBytes => "imre_path_read_bytes",
                    PathOperation::ReadText if args.len() == 1 => "imre_path_read_text",
                    PathOperation::ReadText => "imre_path_read_text_options",
                    PathOperation::WriteText => "imre_path_write_text_options",
                    PathOperation::WriteUnion { binary: true } => "imre_path_write_bytes_union",
                    PathOperation::WriteUnion { binary: false } => "imre_path_write_text_union",
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
