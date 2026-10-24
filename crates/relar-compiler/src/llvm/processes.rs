//! Native process calls retain their rooted inputs and return concrete records.

use super::*;
use crate::ir::ProcessOperation;

impl Emitter<'_, '_> {
    pub(super) fn process_builtin(
        &mut self,
        operation: ProcessOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        match operation {
            ProcessOperation::Executable => self.call("relar_sys_executable", Type::Str, &[]),
            ProcessOperation::RunToStderr => {
                let check = self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                self.checked_line(format!(
                    "call void @relar_subprocess_to_stderr(ptr {}, i32 {check})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::Run | ProcessOperation::RunTimeout => {
                let layout = self
                    .output
                    .global_ref("layout_completed_process".to_owned());
                let cwd =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let environment =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let check = self.instruction(format!("zext i1 {} to i32", args[3].value.operand));
                let operand = if matches!(operation, ProcessOperation::RunTimeout) {
                    if args[4].value.ty == Type::Int {
                        self.checked_instruction(format!(
                            "call ptr @relar_subprocess_run_timeout_int(ptr {}, ptr {cwd}, ptr {environment}, i32 {check}, i64 {}, ptr @{layout})",
                            args[0].value.operand, args[4].value.operand
                        ))
                    } else {
                        let timeout = self
                            .instruction(format!("inttoptr i64 {} to ptr", args[4].value.operand));
                        self.checked_instruction(format!(
                        "call ptr @relar_subprocess_run_timeout(ptr {}, ptr {cwd}, ptr {environment}, i32 {check}, ptr {timeout}, ptr @{layout})",
                        args[0].value.operand
                    ))
                    }
                } else {
                    self.checked_instruction(format!(
                        "call ptr @relar_subprocess_run(ptr {}, ptr {cwd}, ptr {environment}, i32 {check}, ptr @{layout})",
                        args[0].value.operand
                    ))
                };
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::EnvironGet if returns == Type::Str => self.call(
                "relar_environ_get",
                Type::Str,
                &[&args[0].value, &args[1].value],
            ),
            ProcessOperation::EnvironGet => {
                let default =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let pointer = self.checked_instruction(format!(
                    "call ptr @relar_environ_get(ptr {}, ptr {default})",
                    args[0].value.operand
                ));
                let operand = self.instruction(format!("ptrtoint ptr {pointer} to i64"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::GetCwd => self.call("relar_getcwd", Type::Str, &[]),
            ProcessOperation::Which => {
                let path =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let bytes = u8::from(args[0].value.ty == Type::Bytes);
                let path_bytes =
                    u8::from(self.module.types.payload(args[2].value.ty) == Some(Type::Bytes));
                let pointer = self.checked_instruction(format!("call ptr @relar_shutil_which(ptr {}, i64 {}, ptr {path}, i32 {bytes}, i32 {path_bytes})", args[0].value.operand, args[1].value.operand));
                let operand = self.instruction(format!("ptrtoint ptr {pointer} to i64"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::EnvironCopy => {
                let operand = self.checked_instruction("call ptr @relar_environ_copy()");
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::CheckReturnCode => {
                self.checked_line(format!(
                    "call void @relar_subprocess_check(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::ErrorField(field) => {
                let word = self.checked_instruction(format!(
                    "call i64 @relar_subprocess_error_field(ptr {}, i32 {field})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::TimeoutField(field) => {
                let word = self.instruction(format!(
                    "call i64 @relar_subprocess_timeout_field(ptr {}, i32 {field})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::ExceptionView => {
                self.reference(&args[0].value, "retain");
                // Separate SSA names keep this owner registered when argument
                // cleanup releases an owned source view of the same pointer.
                let operand = self.instruction(format!(
                    "getelementptr i8, ptr {}, i64 0",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
        }
    }
}
