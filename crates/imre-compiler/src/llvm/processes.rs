//! Native process calls retain their rooted inputs and return concrete records.

use super::*;
use crate::ir::ProcessOperation;

mod descriptors;
mod readiness;

fn shutil_tag(ty: Type) -> u32 {
    match ty {
        Type::Str => 0,
        Type::Bytes => 1,
        Type::Path => 2,
        _ => unreachable!(),
    }
}

impl Emitter<'_, '_> {
    /// Keep the callee, bound context, return ownership, and parameter ABI together.
    fn shutil_copier(&mut self, copy: &Operand) -> (String, String, String, u32, u32) {
        if copy.value.ty == Type::Str {
            return (
                "0".into(),
                "@imre_shutil_noncallable".into(),
                copy.value.operand.clone(),
                0,
                0,
            );
        }
        if copy.value.ty == Type::Bool {
            return (
                self.instruction(format!("zext i1 {} to i32", copy.value.operand)),
                "null".into(),
                "null".into(),
                0,
                0,
            );
        }
        let (params, result) = self.module.types.signature(copy.value.ty).unwrap();
        let target = self.instruction(format!("load ptr, ptr {}", copy.value.operand));
        let slot = self.instruction(format!(
            "getelementptr %callable, ptr {}, i32 0, i32 1",
            copy.value.operand
        ));
        let context = self.instruction(format!("load ptr, ptr {}", slot));
        let tagged = u32::from(matches!(params[0], Type::Union(_)))
            | (u32::from(matches!(params[1], Type::Union(_))) << 1);
        (
            "0".into(),
            target,
            context,
            representation_kind(result, &self.module.types),
            tagged,
        )
    }

    pub(super) fn process_builtin(
        &mut self,
        operation: ProcessOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        match operation {
            ProcessOperation::SelectorClass(mode) => {
                let which = mode.map_or(2, u32::from);
                let operand =
                    self.instruction(format!("call ptr @imre_popen_selector_class(i32 {which})"));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::SetSelector => {
                self.line(format!(
                    "call void @imre_popen_set_selector(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
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
            ProcessOperation::ShellOutput { status } => {
                let tuple = status.then(|| self.record(returns));
                let status_slot = tuple.as_ref().map_or("null", |v| v.operand.as_str());
                let kind = representation_kind(args[0].value.ty, &self.module.types);
                let output = self.checked_instruction(format!(
                    "call ptr @imre_subprocess_shell_output(ptr {}, i32 {kind}, i64 {}, i64 {}, ptr {status_slot})",
                    args[0].value.operand, args[1].value.operand, args[2].value.operand
                ));
                if let Some(tuple) = tuple {
                    let slot = self
                        .instruction(format!("getelementptr i64, ptr {}, i64 1", tuple.operand));
                    self.line(format!("store ptr {output}, ptr {slot}"));
                    tuple
                } else {
                    self.owned(Value {
                        ty: returns,
                        operand: output,
                        small_integer: false,
                    })
                }
            }
            ProcessOperation::Popen { streams }
            | ProcessOperation::Call { streams, .. }
            | ProcessOperation::CheckOutput { streams, .. }
            | ProcessOperation::Run { streams, .. } => {
                let cwd =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let env =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[3].value.operand));
                let executable =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[10].value.operand));
                // Bit 20 is set in the ABI word only when the caller disables
                // restoration. The optional boolean is the final bound operand.
                let signal_arg = streams & (1 << 20) != 0;
                let stream_word = if signal_arg {
                    self.instruction(format!(
                        "select i1 {}, i32 {}, i32 {}",
                        args[args.len() - 1].value.operand,
                        streams & !(1 << 20),
                        streams
                    ))
                } else {
                    streams.to_string()
                };
                let kind = |index: usize| {
                    representation_kind(
                        self.module
                            .types
                            .payload(args[index].value.ty)
                            .unwrap_or(args[index].value.ty),
                        &self.module.types,
                    )
                };
                let mut fields = vec![
                    ("ptr", args[0].value.operand.clone()),
                    ("ptr", cwd),
                    ("ptr", env),
                    ("ptr", executable),
                    ("i64", args[1].value.operand.clone()),
                    ("i32", kind(0).to_string()),
                    ("i32", kind(2).to_string()),
                    ("i32", kind(10).to_string()),
                    ("i32", stream_word),
                    ("i1", args[9].value.operand.clone()),
                    ("i1", args[11].value.operand.clone()),
                ];
                // ChildConfig embeds RedirectConfig, which embeds the legacy prefix.
                let child_options = streams & (1 << 18) != 0;
                if child_options
                    || [12, 0, 4]
                        .into_iter()
                        .any(|shift| matches!((streams >> shift) & 15, 6 | 7))
                {
                    for (index, shift) in [(4, 12), (5, 0), (6, 4)] {
                        let word = if matches!((streams >> shift) & 15, 6 | 7) {
                            self.word(&args[index].value)
                        } else {
                            "0".to_owned()
                        };
                        fields.push(("i64", word));
                    }
                }
                if child_options {
                    let pass_fds = streams & (1 << 19) != 0;
                    let pipe_size = streams & (1 << 21) != 0;
                    let start = args.len()
                        - 3
                        - usize::from(pass_fds)
                        - usize::from(pipe_size)
                        - usize::from(signal_arg);
                    fields.extend([
                        ("i64", args[start].value.operand.clone()),
                        ("i64", args[start + 1].value.operand.clone()),
                        ("i1", args[start + 2].value.operand.clone()),
                    ]);
                    if pass_fds {
                        fields.extend([
                            ("i64", self.word(&args[start + 3].value)),
                            ("i32", kind(start + 3).to_string()),
                        ]);
                    } else if pipe_size {
                        // SizedPipeConfig embeds the complete pass_fds layout.
                        fields.extend([("i64", "0".into()), ("i32", "0".into())]);
                    }
                    if pipe_size {
                        let value = &args[start + 3 + usize::from(pass_fds)].value;
                        fields.extend([
                            ("i64", self.word(value)),
                            (
                                "i32",
                                representation_kind(value.ty, &self.module.types).to_string(),
                            ),
                        ]);
                    }
                }
                let layout = format!(
                    "{{{}}}",
                    fields
                        .iter()
                        .map(|(ty, _)| *ty)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let config = self.entry_alloca(&layout);
                let mut aggregate = "poison".to_owned();
                for (i, (ty, value)) in fields.iter().enumerate() {
                    aggregate = self.instruction(format!(
                        "insertvalue {layout} {aggregate}, {ty} {value}, {i}"
                    ));
                }
                self.line(format!("store {layout} {aggregate}, ptr {config}"));
                if let ProcessOperation::Call { check, .. } = operation {
                    let timeout = &args[12].value;
                    let kind = if timeout.ty == Type::Int { 2 } else { 7 };
                    let word = self.checked_instruction(format!(
                        "call i64 @imre_call_config(ptr {config}, i32 {kind}, i64 {}, i1 {})",
                        timeout.operand,
                        u8::from(check)
                    ));
                    return self.unpack_word(word, returns);
                }
                let operand = match operation {
                    ProcessOperation::Run {
                        validation, errors, ..
                    }
                    | ProcessOperation::CheckOutput {
                        validation, errors, ..
                    } => {
                        let timeout = &args[12].value;
                        let kind = if timeout.ty == Type::Int { 2 } else { 7 };
                        let (checked, descriptor) =
                            if matches!(operation, ProcessOperation::Run { .. }) {
                                let name = self
                                    .output
                                    .global_ref(format!("layout_{}", record_name(returns)));
                                (args[14].value.operand.as_str(), format!("@{name}"))
                            } else {
                                ("true", "null".to_owned())
                            };
                        self.checked_instruction(format!("call ptr @imre_run_config(ptr {config}, i32 {kind}, i64 {}, i64 {}, i32 {validation}, i32 {errors}, i1 {checked}, ptr {descriptor})", timeout.operand, args[13].value.operand))
                    }
                    _ => self
                        .checked_instruction(format!("call ptr @imre_popen_config(ptr {config})")),
                };
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
            ProcessOperation::PopenActive => self.call("imre_popen_active", returns, &[]),
            ProcessOperation::PopenCleanup => {
                self.line("call void @imre_popen_cleanup()");
                none()
            }
            ProcessOperation::PopenIdentity => {
                self.call("imre_popen_identity", returns, &[&args[0].value])
            }
            ProcessOperation::PopenStatus { poll: true } => {
                let word = self.checked_instruction(format!(
                    "call i64 @imre_popen_status(ptr {}, i1 true)",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::PopenStatus { poll: false } => {
                let control = self.instruction(format!(
                    "call ptr @imre_popen_control(ptr {})",
                    args[0].value.operand
                ));
                let word = self.checked_instruction(format!(
                    "call i64 @imre_child_status(ptr {control}, i1 false)"
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::PopenSetPoll { returns_none } => {
                self.checked_line(format!(
                    "call void @imre_popen_set_poll(ptr {}, ptr {}, i1 {})",
                    args[1].value.operand,
                    args[0].value.operand,
                    u8::from(returns_none)
                ));
                none()
            }
            ProcessOperation::PopenDeletePoll => {
                self.checked_line(format!(
                    "call void @imre_popen_del_poll(ptr {})",
                    args[0].value.operand
                ));
                none()
            }
            ProcessOperation::PopenSetStatus => {
                let control = self.instruction(format!(
                    "call ptr @imre_popen_control(ptr {})",
                    args[1].value.operand
                ));
                self.line(format!(
                    "call void @imre_child_set_status(ptr {control}, i64 {})",
                    args[0].value.operand
                ));
                none()
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
                let control = self.instruction(format!(
                    "call ptr @imre_popen_control(ptr {})",
                    args[0].value.operand
                ));
                let kind = if args[1].value.ty == Type::Int { 2 } else { 7 };
                let word = self.checked_instruction(format!(
                    "call i64 @imre_child_wait(ptr {control}, i32 {kind}, i64 {}, ptr {})",
                    args[1].value.operand, args[0].value.operand
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
                let overridden = self.checked_instruction(format!(
                    "call i1 @imre_popen_signal_override(ptr {}, i64 {})",
                    args[0].value.operand, args[1].value.operand
                ));
                let (default, done) = (self.label(), self.label());
                self.branch(&overridden, &done, &default);
                self.start(&default);
                let control = self.instruction(format!(
                    "call ptr @imre_popen_control(ptr {})",
                    args[0].value.operand
                ));
                self.checked_line(format!(
                    "call void @imre_child_signal(ptr {control}, i64 {})",
                    args[1].value.operand
                ));
                self.jump(&done);
                self.start(&done);
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
            ProcessOperation::RunToStderr => {
                let check = self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                self.checked_line(format!(
                    "call void @imre_subprocess_to_stderr(ptr {}, i32 {check})",
                    args[0].value.operand
                ));
                none()
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
            ProcessOperation::DescriptorConstant(name) => self.os_descriptor_constant(name),
            ProcessOperation::Readiness(op) => self.readiness(op, args, returns),
            ProcessOperation::Fd(operation) => self.os_descriptor(operation, args, returns),
            ProcessOperation::CredentialId { group } => {
                let operand = self.checked_instruction(format!(
                    "call i64 @imre_os_credential_id(i64 {}, i32 {})",
                    args[0].value.operand,
                    u8::from(group)
                ));
                Value {
                    ty: Type::Int,
                    operand,
                    small_integer: true,
                }
            }
            ProcessOperation::Credential(name) => {
                if returns == Type::None {
                    let words = args
                        .iter()
                        .map(|arg| format!("i64 {}", arg.value.operand))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.checked_line(format!("call void @imre_os_{name}({words})"));
                    none()
                } else if returns == Type::Int {
                    let operand = self.instruction(format!("call i64 @imre_os_{name}()"));
                    Value {
                        ty: returns,
                        operand,
                        small_integer: true,
                    }
                } else {
                    let tuple = self.record(returns);
                    self.checked_line(format!("call void @imre_os_{name}(ptr {})", tuple.operand));
                    tuple
                }
            }
            ProcessOperation::GetCwd => self.call("imre_getcwd", Type::Str, &[]),
            ProcessOperation::RmTree => {
                let [path, ignore, _, _, fd] = args else {
                    unreachable!()
                };
                let (ptr, tag) = match path.value.ty {
                    Type::None => ("null", 2),
                    Type::Bytes => (path.value.operand.as_str(), 1),
                    Type::Path | Type::PurePath(_) => (path.value.operand.as_str(), 3),
                    _ => (path.value.operand.as_str(), 0),
                };
                let (fd, fd_kind) = match fd.value.ty {
                    Type::Str => ("0", 2),
                    Type::None => ("0", 0),
                    _ => (fd.value.operand.as_str(), 1),
                };
                let ignore = self.instruction(format!("zext i1 {} to i32", ignore.value.operand));
                self.checked_line(format!("call void @imre_shutil_rmtree(ptr {ptr}, i32 {tag}, i32 {ignore}, i64 {fd}, i32 {fd_kind}, ptr null, ptr null, ptr null, ptr null)"));
                none()
            }
            ProcessOperation::IgnorePatterns => {
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_shutil_ignore_patterns_call(ptr {}, ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand, args[2].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::TreeErrorArgs => {
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(returns)));
                let ptr = self.checked_instruction(format!(
                    "call ptr @imre_shutil_tree_error_args(ptr {}, ptr @{layout})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand: ptr,
                    small_integer: false,
                })
            }
            ProcessOperation::CopyTree(issue) => {
                let [src, dst, links, ignore, stat, dangling, existing] = args else {
                    unreachable!()
                };
                let (stat, target, context, repr, tagged) = self.shutil_copier(stat);
                debug_assert_eq!(tagged, 0);
                let [links, dangling, existing] = [links, dangling, existing]
                    .map(|arg| self.instruction(format!("zext i1 {} to i32", arg.value.operand)));
                let (ignore_target, ignore_context, ignore_set) = if ignore.value.ty == Type::None {
                    ("null".into(), "null".into(), 0)
                } else {
                    let (_, ret) = self.module.types.signature(ignore.value.ty).unwrap();
                    let target =
                        self.instruction(format!("load ptr, ptr {}", ignore.value.operand));
                    let slot = self.instruction(format!(
                        "getelementptr %callable, ptr {}, i32 0, i32 1",
                        ignore.value.operand
                    ));
                    let context = self.instruction(format!("load ptr, ptr {slot}"));
                    (
                        target,
                        context,
                        u32::from(self.module.types.set_element(ret).is_some()),
                    )
                };
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(issue)));
                let operand = self.checked_instruction(format!(
                    "call i64 @imre_shutil_copytree(ptr {}, i32 {}, ptr {}, i32 {}, i32 {links}, i32 {stat}, i32 {dangling}, i32 {existing}, ptr @{layout}, ptr {target}, ptr {context}, i32 {repr}, ptr {ignore_target}, ptr {ignore_context}, i32 {ignore_set})",
                    src.value.operand, shutil_tag(src.value.ty), dst.value.operand, shutil_tag(dst.value.ty)));
                self.unpack_word(operand, returns)
            }
            ProcessOperation::Move(issue) => {
                let [src, dst, copy] = args else {
                    unreachable!()
                };
                let (stat, target, context, repr, tagged) = self.shutil_copier(copy);
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(issue)));
                let word = self.checked_instruction(format!(
                    "call i64 @imre_shutil_move(ptr {}, i32 {}, ptr {}, i32 {}, i32 {stat}, ptr @{layout}, ptr {target}, ptr {context}, i32 {repr}, i32 {tagged})",
                    src.value.operand,shutil_tag(src.value.ty),dst.value.operand,shutil_tag(dst.value.ty)));
                self.unpack_word(word, returns)
            }
            ProcessOperation::DestInSrc => {
                let [src, dst] = args else { unreachable!() };
                let byte = self.checked_instruction(format!(
                    "call i32 @imre_shutil_destinsrc(ptr {}, i32 {}, ptr {}, i32 {})",
                    src.value.operand,
                    shutil_tag(src.value.ty),
                    dst.value.operand,
                    shutil_tag(dst.value.ty)
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i32 {byte}, 0")),
                    small_integer: false,
                }
            }
            ProcessOperation::Copy(op) => {
                let [src, dst, follow] = args else {
                    unreachable!()
                };
                let follow = self.instruction(format!("zext i1 {} to i32", follow.value.operand));
                let operand = self.checked_instruction(format!(
                    "call i64 @imre_shutil_copy(i32 {}, ptr {}, i32 {}, ptr {}, i32 {}, i32 {follow})",
                    op as u32,
                    src.value.operand,
                    shutil_tag(src.value.ty),
                    dst.value.operand,
                    shutil_tag(dst.value.ty)
                ));
                self.unpack_word(operand, returns)
            }
            ProcessOperation::Which => {
                let path =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[2].value.operand));
                let bytes = u8::from(args[0].value.ty == Type::Bytes);
                let path_bytes =
                    u8::from(self.module.types.payload(args[2].value.ty) == Some(Type::Bytes));
                let pointer = self.checked_instruction(format!("call ptr @imre_shutil_which(ptr {}, i64 {}, ptr {path}, i32 {bytes}, i32 {path_bytes}, ptr @imre_shutil_confstr_cs_path, ptr @imre_shutil_os_defpath)", args[0].value.operand, args[1].value.operand));
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
            ProcessOperation::OsErrorField(field) => {
                let word = self.checked_instruction(format!(
                    "call i64 @imre_os_error_field(ptr {}, i32 {field})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            ProcessOperation::QuoteCommandArgument => {
                let word = self.word(&args[0].value);
                let kind = representation_kind(args[0].value.ty, &self.module.types);
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_subprocess_quote_argument(i64 {word}, i32 {kind})"
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            ProcessOperation::Strerror => {
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_os_strerror(i64 {})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
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
