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
            ProcessOperation::NewLoop | ProcessOperation::EnsureTask => {
                let call = if matches!(operation, ProcessOperation::NewLoop) {
                    "call ptr @relar_async_loop_new()".to_owned()
                } else {
                    format!(
                        "call ptr @relar_async_ensure(ptr {})",
                        args[0].value.operand
                    )
                };
                let operand = self.checked_instruction(call);
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::RunUntilComplete => {
                let word = self.checked_instruction(format!(
                    "call i64 @relar_async_loop_run(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::SetLoop
            | ProcessOperation::CloseLoop
            | ProcessOperation::LoopSignal => {
                let call = match operation {
                    ProcessOperation::SetLoop => format!(
                        "call void @relar_async_loop_set(ptr {})",
                        args[0].value.operand
                    ),
                    ProcessOperation::CloseLoop => format!(
                        "call void @relar_async_loop_close(ptr {})",
                        args[0].value.operand
                    ),
                    _ => format!(
                        "call void @relar_async_loop_signal(ptr {}, i64 {}, ptr {})",
                        args[0].value.operand, args[1].value.operand, args[2].value.operand
                    ),
                };
                self.checked_line(call);
                none()
            }
            ProcessOperation::CancelTask => {
                let flag = self.instruction(format!(
                    "call i32 @relar_async_cancel(ptr {})",
                    args[0].value.operand
                ));
                let operand = self.instruction(format!("trunc i32 {flag} to i1"));
                Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                }
            }
            ProcessOperation::Coroutine { function, result } => {
                let kind = representation_kind(result, &self.module.types);
                let thunk = self.output.used(
                    self.output.function_owner(function),
                    self.output.async_name(function),
                );
                let operand = self.instruction(format!(
                    "call ptr @relar_async_coroutine(ptr @{thunk}, ptr {}, i32 {kind})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::ContextNew { function } => {
                let thunk = self.output.used(
                    self.output.function_owner(function),
                    self.output.async_name(function),
                );
                let operand = self.instruction(format!(
                    "call ptr @relar_context_new(ptr @{thunk}, ptr {})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::ContextEnter | ProcessOperation::ContextExit => {
                let op = if matches!(operation, ProcessOperation::ContextEnter) {
                    "enter"
                } else {
                    "exit"
                };
                self.checked_line(format!(
                    "call void @relar_context_{op}(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::ContextYield => {
                self.checked_line("call void @relar_context_yield()");
                none()
            }
            ProcessOperation::ContextThrow => {
                let byte = self.checked_instruction(format!(
                    "call i32 @relar_context_throw(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i32 {byte}, 0")),
                    small_integer: false,
                }
            }
            ProcessOperation::IndirectCoroutine { signature, result } => {
                let Type::Callable(signature) = signature else {
                    unreachable!()
                };
                let kind = representation_kind(result, &self.module.types);
                let thunk = self
                    .output
                    .used(self.output.entry, format!("async_invoke_{signature}"));
                let operand = self.instruction(format!(
                    "call ptr @relar_async_coroutine(ptr @{thunk}, ptr {}, i32 {kind})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::Await | ProcessOperation::RunLoop => {
                let run = u8::from(matches!(operation, ProcessOperation::RunLoop));
                let task = u8::from(matches!(args[0].value.ty, Type::Task(_)));
                let word = self.checked_instruction(format!(
                    "call i64 @relar_async_await(ptr {}, i1 {run}, i1 {task})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::TaskGroup
            | ProcessOperation::CreateTask
            | ProcessOperation::Semaphore => {
                let instruction = match operation {
                    ProcessOperation::TaskGroup => "call ptr @relar_async_group()".to_owned(),
                    ProcessOperation::CreateTask => format!(
                        "call ptr @relar_async_create_task(ptr {}, ptr {})",
                        args[0].value.operand, args[1].value.operand
                    ),
                    _ => format!(
                        "call ptr @relar_async_semaphore(i64 {})",
                        args[0].value.operand
                    ),
                };
                let operand = self.checked_instruction(instruction);
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::Gather { element, tagged } => {
                let kind = representation_kind(element, &self.module.types);
                let operand = self.checked_instruction(format!(
                    "call ptr @relar_async_gather(ptr {}, i1 {}, i32 {kind}, i1 {})",
                    args[0].value.operand,
                    args[1].value.operand,
                    u8::from(tagged)
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::TaskResult => {
                let word = self.checked_instruction(format!(
                    "call i64 @relar_async_result(ptr {})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::MemberCause => {
                let operand = self.instruction(format!(
                    "call ptr @relar_async_member_cause(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::ExceptionAt => {
                let operand = self.checked_instruction(format!(
                    "call ptr @relar_async_exception_at(ptr {}, i64 {})",
                    args[0].value.operand, args[1].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::EnterGroup => {
                self.checked_line(format!(
                    "call void @relar_async_group_enter(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::AbortGroup => {
                self.line(format!(
                    "call void @relar_async_abort_group(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                none()
            }
            ProcessOperation::JoinGroup | ProcessOperation::Acquire | ProcessOperation::Release => {
                let instruction = if matches!(operation, ProcessOperation::JoinGroup) {
                    format!("call void @relar_async_join(ptr {})", args[0].value.operand)
                } else {
                    format!(
                        "call void @relar_async_permit(ptr {}, i1 {})",
                        args[0].value.operand,
                        u8::from(matches!(operation, ProcessOperation::Acquire))
                    )
                };
                self.checked_line(instruction);
                none()
            }
            ProcessOperation::AsyncSpawn { stdout, stderr } => {
                let cwd =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let env =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let operand = self.checked_instruction(format!(
                    "call ptr @relar_async_process_spawn(ptr {}, ptr {cwd}, ptr {env}, i32 {stdout}, i32 {stderr})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::AsyncStatus { wait } => {
                let word = self.checked_instruction(format!(
                    "call i64 @relar_async_process_status(ptr {}, i1 {})",
                    args[0].value.operand,
                    u8::from(wait)
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::AsyncCommunicate => {
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(returns)));
                let operand = self.checked_instruction(format!(
                    "call ptr @relar_async_process_communicate(ptr {}, ptr @{layout})",
                    args[0].value.operand,
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::Executable => self.call("relar_sys_executable", Type::Str, &[]),
            ProcessOperation::CheckOutput => {
                let operand = self.checked_instruction(format!(
                    "call ptr @relar_subprocess_check_output(ptr {})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
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
