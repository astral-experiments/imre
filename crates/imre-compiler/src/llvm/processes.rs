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
                    "call ptr @imre_async_loop_new()".to_owned()
                } else {
                    format!("call ptr @imre_async_ensure(ptr {})", args[0].value.operand)
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
                    "call i64 @imre_async_loop_run(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::SetLoop
            | ProcessOperation::CloseLoop
            | ProcessOperation::LoopSignal => {
                let call = match operation {
                    ProcessOperation::SetLoop => format!(
                        "call void @imre_async_loop_set(ptr {})",
                        args[0].value.operand
                    ),
                    ProcessOperation::CloseLoop => format!(
                        "call void @imre_async_loop_close(ptr {})",
                        args[0].value.operand
                    ),
                    _ => format!(
                        "call void @imre_async_loop_signal(ptr {}, i64 {}, ptr {})",
                        args[0].value.operand, args[1].value.operand, args[2].value.operand
                    ),
                };
                self.checked_line(call);
                none()
            }
            ProcessOperation::CancelTask => {
                let flag = self.instruction(format!(
                    "call i32 @imre_async_cancel(ptr {})",
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
                    "call ptr @imre_async_coroutine(ptr @{thunk}, ptr {}, i32 {kind})",
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
                    "call ptr @imre_context_new(ptr @{thunk}, ptr {})",
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
                    "call void @imre_context_{op}(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::ContextYield => {
                self.checked_line("call void @imre_context_yield()");
                none()
            }
            ProcessOperation::GeneratorNext | ProcessOperation::GeneratorYield => {
                let byte = if matches!(operation, ProcessOperation::GeneratorNext) {
                    self.checked_instruction(format!(
                        "call i32 @imre_generator_next(ptr {})",
                        args[0].value.operand
                    ))
                } else {
                    let word = self.word(&args[0].value);
                    let kind = representation_kind(args[0].value.ty, &self.module.types);
                    self.checked_instruction(format!(
                        "call i32 @imre_generator_yield(i64 {word}, i32 {kind})"
                    ))
                };
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i32 {byte}, 0")),
                    small_integer: false,
                }
            }
            ProcessOperation::GeneratorTake => {
                let word = self.instruction(format!(
                    "call i64 @imre_generator_take(ptr {})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::ContextThrow => {
                let byte = self.checked_instruction(format!(
                    "call i32 @imre_context_throw(ptr {}, ptr {})",
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
                    "call ptr @imre_async_coroutine(ptr @{thunk}, ptr {}, i32 {kind})",
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
                    "call i64 @imre_async_await(ptr {}, i1 {run}, i1 {task})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::TaskGroup
            | ProcessOperation::CreateTask
            | ProcessOperation::Semaphore => {
                let instruction = match operation {
                    ProcessOperation::TaskGroup => "call ptr @imre_async_group()".to_owned(),
                    ProcessOperation::CreateTask => format!(
                        "call ptr @imre_async_create_task(ptr {}, ptr {})",
                        args[0].value.operand, args[1].value.operand
                    ),
                    _ => format!(
                        "call ptr @imre_async_semaphore(i64 {})",
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
                    "call ptr @imre_async_gather(ptr {}, i1 {}, i32 {kind}, i1 {})",
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
                    "call i64 @imre_async_result(ptr {})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::MemberCause => {
                let operand = self.instruction(format!(
                    "call ptr @imre_async_member_cause(ptr {}, ptr {})",
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
                    "call ptr @imre_async_exception_at(ptr {}, i64 {})",
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
                    "call void @imre_async_group_enter(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::AbortGroup => {
                self.line(format!(
                    "call void @imre_async_abort_group(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                none()
            }
            ProcessOperation::JoinGroup | ProcessOperation::Acquire | ProcessOperation::Release => {
                let instruction = if matches!(operation, ProcessOperation::JoinGroup) {
                    format!("call void @imre_async_join(ptr {})", args[0].value.operand)
                } else {
                    format!(
                        "call void @imre_async_permit(ptr {}, i1 {})",
                        args[0].value.operand,
                        u8::from(matches!(operation, ProcessOperation::Acquire))
                    )
                };
                self.checked_line(instruction);
                none()
            }
            ProcessOperation::Popen { streams } | ProcessOperation::Call { streams, .. } => {
                let cwd =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let env =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[3].value.operand));
                let spawn = format!(
                    "ptr {}, ptr {cwd}, ptr {env}, i32 {streams}, i64 {}, i1 {}",
                    args[0].value.operand, args[1].value.operand, args[9].value.operand
                );
                if let ProcessOperation::Call { check, .. } = operation {
                    let timeout = &args[10].value;
                    let kind = if timeout.ty == Type::Int { 2 } else { 7 };
                    let word = self.checked_instruction(format!(
                        "call i64 @imre_subprocess_call({spawn}, i32 {kind}, i64 {}, i1 {})",
                        timeout.operand,
                        u8::from(check)
                    ));
                    return self.unpack_word(word, returns);
                }
                let operand =
                    self.checked_instruction(format!("call ptr @imre_popen_pipes({spawn})"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::PopenPipe(index) => {
                if returns == Type::None {
                    return none();
                }
                let operand = self.instruction(format!(
                    "call ptr @imre_popen_pipe(ptr {}, i32 {index})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::PopenArgs => self.call("imre_popen_args", returns, &[&args[0].value]),
            ProcessOperation::PopenPid => self.call("imre_popen_pid", returns, &[&args[0].value]),
            ProcessOperation::PopenStatus { poll } => {
                let word = self.checked_instruction(format!(
                    "call i64 @imre_popen_status(ptr {}, i1 {})",
                    args[0].value.operand,
                    u8::from(poll)
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::PopenCommunicate { text } => {
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(returns)));
                let input =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let kind = if args[2].value.ty == Type::Int { 2 } else { 7 };
                let operand = self.checked_instruction(format!("call ptr @imre_popen_communicate(ptr {}, ptr {input}, i1 {}, i32 {kind}, i64 {}, ptr @{layout})", args[0].value.operand, u8::from(text), args[2].value.operand));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::PopenWait => {
                let kind = if args[1].value.ty == Type::Int { 2 } else { 7 };
                let word = self.checked_instruction(format!(
                    "call i64 @imre_popen_wait(ptr {}, i32 {kind}, i64 {})",
                    args[0].value.operand, args[1].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::PopenExit => {
                self.checked_line(format!(
                    "call void @imre_popen_exit(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::PopenSignal => {
                self.checked_line(format!(
                    "call void @imre_popen_signal(ptr {}, i64 {})",
                    args[0].value.operand, args[1].value.operand
                ));
                none()
            }
            ProcessOperation::AsyncSpawn { stdout, stderr } => {
                let cwd =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let env =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_async_process_spawn(ptr {}, ptr {cwd}, ptr {env}, i32 {stdout}, i32 {stderr})",
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
                    "call i64 @imre_async_process_status(ptr {}, i1 {})",
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
                    "call ptr @imre_async_process_communicate(ptr {}, ptr @{layout})",
                    args[0].value.operand,
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::Executable => self.call("imre_sys_executable", Type::Str, &[]),
            ProcessOperation::CheckOutput => {
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_subprocess_check_output(ptr {})",
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
                    "call void @imre_subprocess_to_stderr(ptr {}, i32 {check})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::Run { streams } => {
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(returns)));
                let cwd =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let environment =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let check = self.instruction(format!("zext i1 {} to i32", args[3].value.operand));
                let (kind, timeout) = match args.get(4) {
                    Some(arg) => (
                        if arg.value.ty == Type::Int { 2 } else { 7 },
                        arg.value.operand.as_str(),
                    ),
                    None => (0, "0"),
                };
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_subprocess_run_modes(ptr {}, ptr {cwd}, ptr {environment}, i32 {check}, i32 {streams}, i32 {kind}, i64 {timeout}, ptr @{layout})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::EnvironGet if returns == Type::Str => self.call(
                "imre_environ_get",
                Type::Str,
                &[&args[0].value, &args[1].value],
            ),
            ProcessOperation::EnvironGet => {
                let default =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                let pointer = self.checked_instruction(format!(
                    "call ptr @imre_environ_get(ptr {}, ptr {default})",
                    args[0].value.operand
                ));
                let operand = self.instruction(format!("ptrtoint ptr {pointer} to i64"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::GetCwd => self.call("imre_getcwd", Type::Str, &[]),
            ProcessOperation::Which => {
                let path =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let bytes = u8::from(args[0].value.ty == Type::Bytes);
                let path_bytes =
                    u8::from(self.module.types.payload(args[2].value.ty) == Some(Type::Bytes));
                let pointer = self.checked_instruction(format!("call ptr @imre_shutil_which(ptr {}, i64 {}, ptr {path}, i32 {bytes}, i32 {path_bytes})", args[0].value.operand, args[1].value.operand));
                let operand = self.instruction(format!("ptrtoint ptr {pointer} to i64"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::EnvironCopy => {
                let operand = self.checked_instruction("call ptr @imre_environ_copy()");
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::CheckReturnCode => {
                self.checked_line(format!(
                    "call void @imre_subprocess_check(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::ErrorField(field) => {
                let tagged = matches!(
                    self.module.types.payload(returns).unwrap_or(returns),
                    Type::Union(_)
                );
                let field = field | if tagged { 8 } else { 0 };
                let word = self.checked_instruction(format!(
                    "call i64 @imre_subprocess_error_field(ptr {}, i32 {field})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::TimeoutField(field) => {
                let word = self.instruction(format!(
                    "call i64 @imre_subprocess_timeout_field(ptr {}, i32 {field})",
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
