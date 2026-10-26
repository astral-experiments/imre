//! Closed process signatures preserve evaluation order and exact owned results.

use super::*;
use crate::ir::{PathOperation, ProcessCapture, ProcessOperation};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

fn operation(operation: ProcessOperation, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Process(operation), args),
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn absent(ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Absent(Box::new(Expr {
            ty: Type::None,
            kind: ExprKind::None,
        })),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaunchApi {
    Popen,
    Call(bool),
    CheckOutput,
    Run,
}

fn integer_deadline(value: &ast::Expr) -> bool {
    let value = match value {
        ast::Expr::UnaryOp(unary)
            if matches!(unary.op, ast::UnaryOp::USub | ast::UnaryOp::UAdd) =>
        {
            &*unary.operand
        }
        other => other,
    };
    matches!(value, ast::Expr::NumberLiteral(number) if matches!(number.value, ast::Number::Int(_)))
}

fn timeout_argument(call: &ast::ExprCall) -> Option<&ast::Expr> {
    let keyword = call
        .arguments
        .keywords
        .iter()
        .find(|kw| kw.arg.as_ref().is_some_and(|name| name == "timeout"))?;
    (!matches!(keyword.value, ast::Expr::NoneLiteral(_))).then_some(&keyword.value)
}

/// A caught timeout can outlive the function that raised it. Its seconds
/// field has an exact layout only if every possible deadline agrees in kind.
fn timeout_field_type(graph: &SourceGraph) -> Option<Type> {
    struct Deadlines<'a> {
        graph: &'a SourceGraph,
        module: ModuleId,
        int: bool,
        float: bool,
    }
    impl<'ast> Visitor<'ast> for Deadlines<'_> {
        fn visit_expr(&mut self, expr: &'ast ast::Expr) {
            if let ast::Expr::Call(call) = expr
                && let Some(deadline) = match self.graph.resolve(self.module, &call.func) {
                    Some(Binding::Intrinsic(
                        Intrinsic::SubprocessRun
                        | Intrinsic::SubprocessCall
                        | Intrinsic::SubprocessCheckCall
                        | Intrinsic::CheckOutput,
                    )) => timeout_argument(call),
                    _ if matches!(&*call.func, ast::Expr::Attribute(a) if a.attr.as_str() == "wait") => {
                        timeout_argument(call)
                            .or_else(|| call.arguments.args.first())
                            .filter(|d| !matches!(d, ast::Expr::NoneLiteral(_)))
                    }
                    _ if matches!(&*call.func, ast::Expr::Attribute(a) if a.attr.as_str() == "communicate") => {
                        timeout_argument(call)
                            .or_else(|| call.arguments.args.get(1))
                            .filter(|d| !matches!(d, ast::Expr::NoneLiteral(_)))
                    }
                    _ => None,
                }
            {
                if integer_deadline(deadline) {
                    self.int = true;
                } else {
                    self.float = true;
                }
            }
            walk_expr(self, expr);
        }
    }
    let mut deadlines = Deadlines {
        graph,
        module: graph.entry,
        int: false,
        float: false,
    };
    for (id, module) in graph.modules.iter().enumerate() {
        deadlines.module = id;
        deadlines.visit_body(&module.parsed.syntax().body);
    }
    match (deadlines.int, deadlines.float) {
        (true, true) => None,
        (true, false) => Some(Type::Int),
        _ => Some(Type::Float),
    }
}

/// The handler slot and any argument or capture layout shared by its producers.
pub(super) struct CapturedFields {
    pub(super) slot: usize,
    pub(super) tree_issue: Option<Type>,
    argv: Option<Type>,
    capture: Option<ProcessCapture>,
    timeout: bool,
}

/// A timeout can carry partial raw bytes, even for a checked text capture.
fn error_stream(types: &Types, capture: ProcessCapture, stdout: bool, timeout: bool) -> Type {
    let completed = capture.stream(stdout);
    if timeout && completed != Type::None {
        types
            .union(&[completed, types.optional(Type::Bytes)])
            .unwrap()
    } else {
        completed
    }
}

/// Explicit raises, user calls, and task or context resumption can propagate an error from elsewhere.
pub(super) fn process_error_view(
    slot: usize,
    source: &[ast::Stmt],
    body: &[Statement],
    types: &Types,
    kinds: &[u32],
) -> CapturedFields {
    struct Raises(bool);
    impl<'ast> Visitor<'ast> for Raises {
        fn visit_stmt(&mut self, statement: &'ast ast::Stmt) {
            if matches!(statement, ast::Stmt::Raise(_)) {
                self.0 = true;
            } else if !self.0 {
                walk_stmt(self, statement);
            }
        }
    }
    let mut raises = Raises(false);
    raises.visit_body(source);
    let mut captures = HashSet::new();
    let mut argv = HashSet::new();
    let mut precise = !raises.0;
    let mut timed = false;
    let mut tree_issue = None;
    let mut other_shutil = false;
    ir::walk_expressions(body, &mut |value| {
        let (origin, command) = match &value.kind {
            ExprKind::Builtin(Builtin::Process(ProcessOperation::CopyTree(issue)), _) => {
                tree_issue = Some(*issue);
                return;
            }
            ExprKind::Builtin(
                Builtin::Process(ProcessOperation::Copy(_) | ProcessOperation::Move(_)),
                _,
            ) => {
                other_shutil = true;
                return;
            }
            ExprKind::Call(..)
            | ExprKind::Invoke { .. }
            | ExprKind::Construct { .. }
            | ExprKind::Builtin(
                Builtin::Process(
                    ProcessOperation::Await
                    | ProcessOperation::RunLoop
                    | ProcessOperation::RunUntilComplete
                    | ProcessOperation::TaskResult
                    | ProcessOperation::Gather { .. }
                    | ProcessOperation::ContextEnter
                    | ProcessOperation::ContextExit
                    | ProcessOperation::ContextThrow,
                ),
                _,
            ) => {
                precise = false;
                return;
            }
            ExprKind::Builtin(Builtin::Process(ProcessOperation::RunToStderr), _) => (
                ProcessCapture {
                    stdout: false,
                    stderr: false,
                    text: false,
                },
                types.list(Type::Str),
            ),
            ExprKind::Builtin(
                Builtin::Process(ProcessOperation::CheckOutput { capture, .. }),
                args,
            ) => {
                timed = true;
                (*capture, args[0].ty)
            }
            ExprKind::Builtin(
                Builtin::Process(ProcessOperation::Run {
                    timed: deadline, ..
                }),
                _,
            ) => {
                timed |= *deadline;
                let Type::CompletedProcess(capture, cmd) = value.ty else {
                    unreachable!()
                };
                (capture, types.completed_command(cmd))
            }
            ExprKind::Builtin(
                Builtin::Process(ProcessOperation::PopenCommunicate { .. }),
                args,
            ) => {
                timed = true;
                let Type::Popen(capture, _, cmd) = args[0].ty else {
                    unreachable!()
                };
                (capture, types.command_type(cmd))
            }
            ExprKind::Builtin(
                Builtin::Process(ProcessOperation::PopenWait | ProcessOperation::Call { .. }),
                args,
            ) => {
                timed = true;
                let command = match args[0].ty {
                    Type::Popen(_, _, id) => types.command_type(id),
                    ty => ty,
                };
                (
                    ProcessCapture {
                        stdout: false,
                        stderr: false,
                        text: false,
                    },
                    command,
                )
            }
            ExprKind::Builtin(Builtin::Process(ProcessOperation::CheckReturnCode), args) => {
                let Type::CompletedProcess(capture, cmd) = args[0].ty else {
                    unreachable!()
                };
                (capture, types.completed_command(cmd))
            }
            _ => return,
        };
        captures.insert(origin);
        argv.insert(command);
    });
    CapturedFields {
        slot,
        tree_issue: if precise && !other_shutil && kinds == [64] {
            tree_issue
        } else {
            None
        },
        argv: (precise && argv.len() == 1).then(|| *argv.iter().next().unwrap()),
        capture: (precise && captures.len() == 1).then(|| *captures.iter().next().unwrap()),
        // These exception categories include TimeoutExpired.
        timeout: (!precise || timed) && kinds.iter().any(|kind| matches!(kind, 0 | 1 | 25 | 27)),
    }
}

/// Modes 6 and 7 borrow an integer descriptor or file object. The final choice
/// between them is made after checking the argument's type.
fn stream_mode(graph: &SourceGraph, module: ModuleId, value: &ast::Expr) -> u32 {
    if matches!(value, ast::Expr::NoneLiteral(_)) {
        return 0;
    }
    if let ast::Expr::UnaryOp(unary) = value
        && unary.op == ast::UnaryOp::USub
        && let ast::Expr::NumberLiteral(number) = &*unary.operand
        && let ast::Number::Int(number) = &number.value
    {
        return match number.to_string().as_str() {
            "1" => 1, // PIPE
            "2" => 5, // STDOUT
            "3" => 2, // DEVNULL
            _ => 6,
        };
    }
    match graph.resolve(module, value) {
        Some(Binding::Intrinsic(Intrinsic::Pipe)) => 1,
        Some(Binding::Intrinsic(Intrinsic::Devnull)) => 2,
        Some(Binding::Intrinsic(Intrinsic::Stdout)) => 3,
        Some(Binding::Intrinsic(Intrinsic::Stderr)) => 4,
        Some(Binding::Intrinsic(Intrinsic::SubprocessStdout)) => 5,
        Some(Binding::Intrinsic(Intrinsic::Stdin)) => 8,
        _ => 6,
    }
}

/// Capture determines the returned layout. Ordinary stream expressions are
/// evaluated in source order, including ones rejected by run/check_output.
fn stream_plan(
    graph: &SourceGraph,
    module: ModuleId,
    call: &ast::ExprCall,
) -> Result<(u32, ProcessCapture), &'static str> {
    let checked_output =
        graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::CheckOutput));
    let run =
        graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::SubprocessRun));
    let mut streams = [u32::from(checked_output), 0];
    let mut capture = false;
    let mut text = false;
    for keyword in &call.arguments.keywords {
        let value = &keyword.value;
        match keyword.arg.as_ref().map(|name| name.as_str()) {
            Some(name @ ("stdout" | "stderr")) => {
                if checked_output && name == "stdout" {
                    // This keyword always raises, whatever value it evaluates to.
                    continue;
                }
                let index = usize::from(name == "stderr");
                streams[index] = stream_mode(graph, module, value);
            }
            Some(name @ ("capture_output" | "text" | "universal_newlines")) => {
                let flag = match value {
                    ast::Expr::BooleanLiteral(flag) => flag.value,
                    ast::Expr::NoneLiteral(_) => false,
                    _ => {
                        return Err("process capture_output and text require literal bool or None");
                    }
                };
                if name != "capture_output" {
                    text |= flag;
                } else {
                    capture = flag;
                }
            }
            Some("errors")
                if (checked_output || run) && !matches!(value, ast::Expr::NoneLiteral(_)) =>
            {
                if !matches!(value, ast::Expr::StringLiteral(v) if matches!(v.value.to_str(), "strict" | "ignore" | "replace"))
                {
                    return Err("check_output errors requires strict, ignore, replace, or None");
                }
                text = true;
            }
            Some("encoding") if !matches!(value, ast::Expr::NoneLiteral(_)) => {
                if !matches!(value, ast::Expr::StringLiteral(v) if matches!(v.value.to_string().to_ascii_lowercase().replace('_', "-").as_str(), "utf8" | "utf-8"))
                {
                    return Err("process encoding requires literal 'utf-8'");
                }
                text = true;
            }
            _ => {}
        }
    }
    let conflict = capture && streams != [0, 0];
    if capture {
        streams = [1, 1];
    }
    if checked_output {
        streams[0] = 1;
    }
    let stdout = streams[0] == 1;
    let stderr = streams[1] == 1;
    Ok((
        streams[0] | (streams[1] << 4) | (u32::from(conflict) << 8),
        ProcessCapture {
            stdout,
            stderr,
            text: text
                && (stdout
                    || stderr
                    || run
                    || graph.resolve(module, &call.func)
                        == Some(Binding::Intrinsic(Intrinsic::Popen))),
        },
    ))
}

/// A checked failure can escape its lexical run call. The closed module graph
/// determines which optional text/bytes layouts an error field may actually use.
fn error_field_type(graph: &SourceGraph, types: &Types, field: u32, timeout: bool) -> Option<Type> {
    struct Outputs<'a> {
        graph: &'a SourceGraph,
        module: ModuleId,
        stdout: bool,
        variable_command: bool,
        check_command: bool,
        types: &'a Types,
        timeout: bool,
        found: Vec<Type>,
    }
    impl<'ast> Visitor<'ast> for Outputs<'_> {
        fn visit_expr(&mut self, expression: &'ast ast::Expr) {
            if let ast::Expr::Call(call) = expression {
                let ty = match self.graph.resolve(self.module, &call.func) {
                    Some(Binding::Intrinsic(
                        Intrinsic::SubprocessCall | Intrinsic::SubprocessCheckCall,
                    )) => {
                        self.variable_command = true; // the command may have a non-list layout
                        Some(Type::None)
                    }
                    Some(Binding::Intrinsic(Intrinsic::Popen)) => {
                        self.variable_command = true;
                        None
                    }
                    Some(Binding::Intrinsic(Intrinsic::CheckOutput)) => {
                        self.variable_command = true;
                        stream_plan(self.graph, self.module, call)
                            .ok()
                            .map(|(_, capture)| {
                                error_stream(
                                    self.types,
                                    capture,
                                    self.stdout,
                                    self.timeout && timeout_argument(call).is_some(),
                                )
                            })
                    }
                    Some(Binding::Intrinsic(Intrinsic::SubprocessRun)) => {
                        if self.check_command && !self.variable_command {
                            let arg = call.arguments.args.first().or_else(|| {
                                call.arguments
                                    .keywords
                                    .iter()
                                    .find(|k| k.arg.as_deref() == Some("args"))
                                    .map(|k| &k.value)
                            });
                            self.variable_command = !arg.is_some_and(|arg|
                                matches!(arg, ast::Expr::List(list) if list.elts.iter().all(|e| matches!(e, ast::Expr::StringLiteral(_))))
                                || self.graph.session.is_string_list(self.module, arg)
                            );
                        }
                        stream_plan(self.graph, self.module, call)
                            .ok()
                            .filter(|(mode, _)| mode & 256 == 0)
                            .map(|(_, capture)| {
                                error_stream(
                                    self.types,
                                    capture,
                                    self.stdout,
                                    self.timeout && timeout_argument(call).is_some(),
                                )
                            })
                    }
                    _ => None,
                };
                if let Some(ty) = ty
                    && !self.found.contains(&ty)
                {
                    self.found.push(ty);
                }
            }
            walk_expr(self, expression);
        }
    }
    let mut visitor = Outputs {
        graph,
        module: graph.entry,
        stdout: field != 3,
        variable_command: false,
        check_command: field == 0,
        types,
        timeout,
        found: Vec::new(),
    };
    for (id, module) in graph.modules.iter().enumerate() {
        visitor.module = id;
        visitor.visit_body(&module.parsed.syntax().body);
    }
    if field == 0 {
        // Popen and checked launches may retain a tuple, path or other command object.
        (!visitor.variable_command).then(|| types.list(Type::Str))
    } else {
        Some(types.union(&visitor.found).unwrap_or(Type::Str))
    }
}

impl Checker<'_> {
    /// Piped output has a fixed pair layout, so communicate can return bytes
    /// directly for captured streams and None for inherited or discarded streams.
    pub(super) fn async_process_await(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::CreateSubprocessExec)) {
            let strings = self.types.list(Type::Str);
            let argv = self.hidden(strings);
            let mut body = vec![Statement::Assign(
                argv,
                Expr {
                    ty: strings,
                    kind: ExprKind::List(vec![]),
                },
            )];
            let mut clear = vec![argv];
            let mut element = Some(Type::Str);
            for argument in &call.arguments.args {
                if let ast::Expr::Starred(starred) = argument {
                    let items = self.expansion_input(&starred.value, element)?;
                    body.extend(self.expand_items(items, starred, |checker, item| {
                        checker.expansion_insert(argv, &mut element, item, false, starred)
                    })?);
                } else {
                    let path = self.path_argument(argument)?;
                    body.extend(self.expansion_insert(
                        argv,
                        &mut element,
                        path,
                        false,
                        argument,
                    )?);
                }
            }
            let mut cwd = absent(self.types.optional(Type::Str));
            let mut env = absent(self.types.optional(self.types.dict(Type::Str, Type::Str)));
            let mut pipes = [0, 0];
            for keyword in &call.arguments.keywords {
                match keyword.arg.as_deref() {
                    Some(name @ ("stdout" | "stderr")) => {
                        pipes[usize::from(name == "stderr")] = match self.resolve(&keyword.value) {
                            Some(Binding::Intrinsic(Intrinsic::Pipe)) => 1,
                            Some(Binding::Intrinsic(Intrinsic::Devnull)) => 2,
                            _ if matches!(keyword.value, ast::Expr::NoneLiteral(_)) => 0,
                            _ => {
                                return Err(error(
                                    self.source,
                                    keyword,
                                    "async process streams require PIPE, DEVNULL, or None",
                                ));
                            }
                        };
                    }
                    Some(name @ ("cwd" | "env")) => {
                        let value = if name == "cwd" {
                            self.optional_path_argument(&keyword.value)?
                        } else {
                            self.expr_expected(&keyword.value, env.ty)?
                        };
                        let slot = self.hidden(value.ty);
                        body.push(Statement::Assign(slot, value));
                        clear.push(slot);
                        if name == "cwd" {
                            cwd = self.local(slot);
                        } else {
                            env = self.local(slot);
                        }
                    }
                    _ => {
                        return Err(error(
                            self.source,
                            keyword,
                            "unsupported async subprocess option",
                        ));
                    }
                }
            }
            let [stdout, stderr] = pipes;
            let ty = Type::AsyncProcess(u8::from(stdout == 1) | (u8::from(stderr == 1) << 1));
            return Ok(Some(Expr {
                ty,
                kind: ExprKind::Scope {
                    body,
                    value: Box::new(operation(
                        ProcessOperation::AsyncSpawn { stdout, stderr },
                        ty,
                        vec![self.local(argv), cwd, env],
                    )),
                    clear,
                },
            }));
        }
        let ast::Expr::Attribute(attr) = &*call.func else {
            return Ok(None);
        };
        if !matches!(attr.attr.as_str(), "wait" | "communicate")
            || self.class_reference(&attr.value).is_some()
        {
            return Ok(None);
        }
        let receiver = self.expr(&attr.value)?;
        let Type::AsyncProcess(pipes) = receiver.ty else {
            return Ok(None);
        };
        if !call.arguments.is_empty() {
            return Err(error(
                self.source,
                call,
                "async process methods accept no arguments",
            ));
        }
        Ok(Some(if attr.attr.as_str() == "wait" {
            operation(
                ProcessOperation::AsyncStatus { wait: true },
                Type::Int,
                vec![receiver],
            )
        } else {
            let stream = |bit| {
                if pipes & bit != 0 {
                    Type::Bytes
                } else {
                    Type::None
                }
            };
            operation(
                ProcessOperation::AsyncCommunicate,
                self.types.tuple(vec![stream(1), stream(2)]),
                vec![receiver],
            )
        }))
    }

    pub(super) fn async_returncode(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        if attr.attr.as_str() != "returncode" {
            return Err(error(
                self.source,
                attr,
                "unsupported asyncio.subprocess.Process attribute",
            ));
        }
        Ok(operation(
            ProcessOperation::AsyncStatus { wait: false },
            self.types.optional(Type::Int),
            vec![receiver],
        ))
    }

    /// An immediately discarded process result can inherit the parent's stderr
    /// directly without materializing a CompletedProcess or buffering its output.
    pub(super) fn process_to_stderr(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let [argv] = call.arguments.args.as_ref() else {
            return self.process_run(call);
        };
        let mut redirected = false;
        let mut check = None;
        for kw in &call.arguments.keywords {
            match kw.arg.as_ref().map(|name| name.as_str()) {
                Some("check") if check.is_none() => check = Some(&kw.value),
                Some("stdout")
                    if !redirected
                        && self.resolve(&kw.value)
                            == Some(Binding::Intrinsic(Intrinsic::Stderr)) =>
                {
                    redirected = true
                }
                _ => return self.process_run(call),
            }
        }
        let argv = self.command_argument(argv)?;
        if argv.ty != self.types.list(Type::Str) {
            return self.process_run(call);
        }
        let check = match check {
            Some(value) => self.expr_expected(value, Type::Bool)?,
            None => boolean(false),
        };
        Ok(operation(
            ProcessOperation::RunToStderr,
            Type::None,
            vec![argv, check],
        ))
    }

    /// Keywords capture their values in source order before applying the filesystem ABI.
    pub(super) fn shutil_which(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let path = self.types.optional(Type::Str);
        let params = [
            ("cmd", Type::Str, None),
            (
                "mode",
                Type::Int,
                Some(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("1".into()),
                }),
            ),
            ("path", path, Some(absent(path))),
        ]
        .map(|(name, ty, default)| Parameter {
            name: name.into(),
            ty,
            kind: ParameterKind::PositionalOrKeyword,
            default,
            sign_only: false,
        });
        let args = self.arguments_with(call, &params, None, |checker, i, arg| match i {
            0 => checker.fspath_argument(arg),
            1 => checker.expr_expected(arg, Type::Int),
            _ => {
                let raw = checker.expr(arg)?;
                if raw.ty == Type::None {
                    return checker.coerce(raw, path, arg);
                }
                if checker
                    .types
                    .payload(raw.ty)
                    .is_some_and(|t| matches!(t, Type::Str | Type::Bytes))
                {
                    return Ok(raw);
                }
                let value = checker.fspath_value(raw, arg)?;
                let ty = checker.types.optional(value.ty);
                checker.coerce(value, ty, arg)
            }
        })?;
        let returns = self.types.optional(args.values[args.order[0]].ty);
        Ok(self.ordered_builtin(Builtin::Process(ProcessOperation::Which), returns, args))
    }

    /// Consume the original iterable and quote each fsdecoded path as a Windows argument.
    pub(super) fn subprocess_list2cmdline(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = call.arguments.args.as_ref();
        let keywords = call.arguments.keywords.as_ref();
        let source = match (args, keywords) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some("seq") => &kw.value,
            _ => {
                self.graph.checked_keywords(
                    self.module,
                    keywords
                        .iter()
                        .filter(|kw| kw.arg.as_deref() != Some("seq"))
                        .map(Ranged::range),
                );
                if args.is_empty() && !keywords.iter().any(|kw| kw.arg.as_deref() == Some("seq")) {
                    self.graph
                        .checked_missing_argument(self.module, call.range());
                }
                if !args.is_empty() {
                    for kw in keywords
                        .iter()
                        .filter(|kw| kw.arg.as_deref() == Some("seq"))
                    {
                        self.graph.checked_repeated_keyword(self.module, kw.range());
                    }
                }
                for extra in args.iter().skip(1) {
                    self.graph
                        .checked_excess_argument(self.module, extra.range());
                }
                let message = if let Some(name) = keywords
                    .iter()
                    .filter_map(|kw| kw.arg.as_deref())
                    .find(|name| *name != "seq" || !args.is_empty())
                {
                    if name == "seq" {
                        "list2cmdline() got multiple values for argument 'seq'".to_owned()
                    } else {
                        format!("list2cmdline() got an unexpected keyword argument '{name}'")
                    }
                } else if args.is_empty() {
                    "list2cmdline() missing 1 required positional argument: 'seq'".to_owned()
                } else {
                    format!(
                        "list2cmdline() takes 1 positional argument but {} were given",
                        args.len()
                    )
                };
                return self.sys_type_error(call, Type::Str, message);
            }
        };
        self.join_parts(
            unpacking::string(" "),
            source,
            call,
            stdlib::JoinStyle::WindowsCommand,
        )
    }

    /// Evaluate a fresh command list in source order, including mixed path representations.
    pub(super) fn process_argument_list(
        &mut self,
        display: &ast::ExprList,
    ) -> Result<Expr, String> {
        let items = display
            .elts
            .iter()
            .map(|item| self.expr(item))
            .collect::<Result<Vec<_>, _>>()?;
        let kinds = items
            .iter()
            .map(|e| {
                if matches!(e.ty, Type::StrLiteral(_)) {
                    Type::Str
                } else {
                    e.ty
                }
            })
            .collect::<Vec<_>>();
        let ty = if kinds.is_empty() {
            Some(Type::Str)
        } else {
            self.types.union(&kinds)
        }
        .ok_or_else(|| {
            error(
                self.source,
                display,
                "command list elements need a representable union",
            )
        })?;
        let items = items
            .into_iter()
            .zip(&display.elts)
            .map(|(value, source)| self.coerce(value, ty, source))
            .collect::<Result<_, _>>()?;
        Ok(Expr {
            ty: self.types.list(ty),
            kind: ExprKind::List(items),
        })
    }

    /// Keep the caller's argument object; only the OS launch converts each entry to bytes.
    fn command_argument(&mut self, arg: &ast::Expr) -> Result<Expr, String> {
        let scalar = |ty| {
            matches!(
                ty,
                Type::Str | Type::StrLiteral(_) | Type::Bytes | Type::Path | Type::PurePath(_)
            )
        };
        let value = if let ast::Expr::List(display) = arg
            && !display
                .elts
                .iter()
                .any(|item| matches!(item, ast::Expr::Starred(_)))
        {
            self.process_argument_list(display)?
        } else {
            self.expr(arg)?
        };
        let element = |ty| {
            scalar(ty)
                || self
                    .types
                    .members(ty)
                    .is_some_and(|members| members.into_iter().all(scalar))
        };
        if scalar(value.ty)
            || self.types.element(value.ty).is_some_and(element)
            || self
                .types
                .fields(value.ty)
                .is_some_and(|fields| fields.into_iter().all(element))
        {
            Ok(value)
        } else {
            Err(error(
                self.source,
                arg,
                "command requires str, bytes, Path, or a list or tuple of these",
            ))
        }
    }

    /// Keep the original bytes or path owner until the native launch. An absent
    /// path has a null word; every present value has its own exact storage kind.
    fn optional_exec_path(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        let ty = self.types.payload(value.ty).unwrap_or(value.ty);
        let ty = match ty {
            Type::None | Type::StrLiteral(_) => Type::Str,
            Type::Str | Type::Bytes | Type::Path | Type::PurePath(_) => ty,
            _ => {
                return Err(error(
                    self.source,
                    expression,
                    "an executable or cwd requires str, bytes, Path or None",
                ));
            }
        };
        self.coerce(value, self.types.optional(ty), expression)
    }

    /// call and check_call accept the same Popen arguments with an extra wait deadline.
    pub(super) fn popen(
        &mut self,
        call: &ast::ExprCall,
        check_call: Option<bool>,
    ) -> Result<Expr, String> {
        self.launch_process(call, check_call.map_or(LaunchApi::Popen, LaunchApi::Call))
    }

    /// Shell output accepts the same command representations as a shell-mode Popen.
    pub(super) fn shell_output(
        &mut self,
        call: &ast::ExprCall,
        status: bool,
    ) -> Result<Expr, String> {
        let text = self.types.optional(Type::Str);
        let params = ["cmd", "encoding", "errors"].map(|name| Parameter {
            name: name.into(),
            ty: if name == "cmd" { Type::Str } else { text },
            kind: if name == "cmd" {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::KeywordOnly
            },
            default: (name != "cmd").then(|| absent(text)),
            sign_only: false,
        });
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index == 0 {
                checker.command_argument(arg)
            } else {
                checker.expr_expected(arg, text)
            }
        })?;
        let ty = if status {
            self.types.tuple(vec![Type::Int, Type::Str])
        } else {
            Type::Str
        };
        Ok(self.ordered_builtin(
            Builtin::Process(ProcessOperation::ShellOutput { status }),
            ty,
            args,
        ))
    }

    pub(super) fn process_check_output(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.launch_process(call, LaunchApi::CheckOutput)
    }

    fn launch_process(&mut self, call: &ast::ExprCall, api: LaunchApi) -> Result<Expr, String> {
        let directory = self.types.optional(Type::Str);
        let environment = self.types.optional(self.types.dict(Type::Str, Type::Str));
        let (streams, capture) = stream_plan(self.graph, self.module, call)
            .map_err(|message| error(self.source, call, message))?;
        let stdin = call
            .arguments
            .keywords
            .iter()
            .find(|kw| kw.arg.as_deref() == Some("stdin"))
            .map_or(0, |kw| stream_mode(self.graph, self.module, &kw.value));
        let keyword = |name: &str| {
            call.arguments
                .keywords
                .iter()
                .find(|kw| kw.arg.as_deref() == Some(name))
                .map(|kw| &kw.value)
        };
        let text_flag = |name| match keyword(name) {
            Some(ast::Expr::BooleanLiteral(flag)) => Some(flag.value),
            _ => None,
        };
        let errors = match keyword("errors") {
            Some(ast::Expr::StringLiteral(v)) if v.value.to_str() == "ignore" => 1,
            Some(ast::Expr::StringLiteral(v)) if v.value.to_str() == "replace" => 2,
            _ => 0,
        };
        let input_given = api == LaunchApi::CheckOutput && keyword("input").is_some();
        let validation = if api == LaunchApi::CheckOutput && keyword("stdout").is_some() {
            1
        } else if api == LaunchApi::CheckOutput && keyword("check").is_some() {
            2
        } else if input_given && stdin != 0 {
            3
        } else if streams & 256 != 0 {
            4
        } else if matches!((text_flag("text"), text_flag("universal_newlines")), (Some(a), Some(b)) if a != b)
        {
            5
        } else {
            0
        };
        let stdin = if input_given { 1 } else { stdin };
        let mut streams = streams | (stdin << 12) | (u32::from(capture.text) << 16);
        let default_bufsize = || Expr {
            ty: Type::Int,
            kind: ExprKind::Int("-1".into()),
        };
        let mut params = [
            ("args", self.types.list(Type::Str), None),
            ("bufsize", Type::Int, Some(default_bufsize())),
            ("cwd", directory, Some(absent(directory))),
            ("env", environment, Some(absent(environment))),
            ("stdin", Type::Bool, Some(boolean(false))),
            ("stdout", Type::Bool, Some(boolean(false))),
            ("stderr", Type::Bool, Some(boolean(false))),
            ("text", Type::Bool, Some(boolean(false))),
            ("encoding", Type::Bool, Some(boolean(false))),
            ("close_fds", Type::Bool, Some(boolean(true))),
            ("executable", directory, Some(absent(directory))),
            ("shell", Type::Bool, Some(boolean(false))),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, ty, default))| Parameter {
            name: name.to_owned(),
            ty,
            default,
            sign_only: false,
            kind: if index <= 1 {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::KeywordOnly
            },
        })
        .collect::<Vec<_>>();
        if api != LaunchApi::Popen {
            let deadline = self.types.optional(Type::Float);
            params.push(Parameter {
                name: "timeout".into(),
                ty: deadline,
                default: Some(absent(deadline)),
                sign_only: false,
                kind: ParameterKind::KeywordOnly,
            });
        }
        if matches!(api, LaunchApi::CheckOutput | LaunchApi::Run) {
            let input = self
                .types
                .optional(if capture.text { Type::Str } else { Type::Bytes });
            for (name, ty, default) in [
                ("input", input, absent(input)),
                ("check", Type::Bool, boolean(false)),
                ("capture_output", Type::Bool, boolean(false)),
                ("universal_newlines", Type::Bool, boolean(false)),
                ("errors", Type::Bool, boolean(false)),
            ] {
                params.push(Parameter {
                    name: name.into(),
                    ty,
                    default: Some(default),
                    sign_only: false,
                    kind: ParameterKind::KeywordOnly,
                });
            }
        }
        let child_options = params.len();
        if [
            "process_group",
            "umask",
            "start_new_session",
            "pass_fds",
            "pipesize",
        ]
        .iter()
        .any(|name| keyword(name).is_some())
        {
            streams |= 1 << 18; // native LaunchConfig includes the child settings trailer
            for (name, ty, default) in [
                ("process_group", Type::Int, default_bufsize()),
                ("umask", Type::Int, default_bufsize()),
                ("start_new_session", Type::Bool, boolean(false)),
            ] {
                params.push(Parameter {
                    name: name.into(),
                    ty,
                    default: Some(default),
                    sign_only: false,
                    kind: ParameterKind::KeywordOnly,
                });
            }
        }
        if keyword("pass_fds").is_some() {
            streams |= 1 << 19;
            params.push(Parameter {
                name: "pass_fds".into(),
                ty: Type::ListInt,
                default: None,
                sign_only: false,
                kind: ParameterKind::KeywordOnly,
            });
        }
        if keyword("pipesize").is_some() {
            streams |= 1 << 21;
            params.push(Parameter {
                name: "pipesize".into(),
                ty: Type::Int,
                default: None,
                sign_only: false,
                kind: ParameterKind::KeywordOnly,
            });
        }
        if keyword("restore_signals").is_some() {
            // The emitter packs this final boolean into the existing stream word.
            streams |= 1 << 20;
            params.push(Parameter {
                name: "restore_signals".into(),
                ty: Type::Bool,
                default: None,
                sign_only: false,
                kind: ParameterKind::KeywordOnly,
            });
        }
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index >= child_options {
                let name = params[index].name.as_str();
                if name == "pipesize" {
                    let value = checker.expr(arg)?;
                    // Typeshed requires int, but Popen accepts None and raises
                    // TypeError itself for other values. Only this bound call
                    // has native validation; nested calls retain their diagnostics.
                    if api == LaunchApi::Popen && !matches!(value.ty, Type::Int | Type::Bool) {
                        checker.graph.checked_overload(checker.module, call.range());
                    }
                    return Ok(value);
                }
                if name == "pass_fds" {
                    let value = checker.expr(arg)?;
                    let fd = |ty| matches!(ty, Type::Int | Type::Bool);
                    if value.ty == Type::None || checker.types.element(value.ty).is_some_and(fd)
                        || checker.types.set_element(value.ty) == Some(Type::Int)
                        || checker.types.fields(value.ty).is_some_and(|fields| fields.into_iter().all(fd))
                    {
                        return Ok(value);
                    }
                    return Err(error(checker.source, arg,
                        "pass_fds requires a native list, tuple, or set of integer descriptors"));
                }
                if matches!(name, "start_new_session" | "restore_signals") {
                    return checker.condition(arg);
                }
                let value = checker.expr(arg)?;
                if value.ty == Type::Bool {
                    return Ok(Expr { ty: Type::Int, kind: ExprKind::Builtin(Builtin::Int, vec![value]) });
                }
                return if name == "process_group" {
                    checker.coerce(value, checker.types.optional(Type::Int), arg)
                } else {
                    checker.coerce(value, Type::Int, arg)
                };
            }
            match index {
                0 => checker.command_argument(arg),
                1 if matches!(arg, ast::Expr::NoneLiteral(_)) => Ok(default_bufsize()),
                2 | 10 => checker.optional_exec_path(arg),
                4..=6 if stream_mode(checker.graph, checker.module, arg) == 6 => {
                    if let ast::Expr::BooleanLiteral(flag) = arg {
                        return Ok(Expr { ty: Type::Int, kind: ExprKind::Int(u8::from(flag.value).to_string()) });
                    }
                    let value = checker.expr(arg)?;
                    // Keep arbitrary integers out of the fixed stream layout: a
                    // dynamic -1 would create a pipe and change the returned type.
                    if (index == 5 && api == LaunchApi::CheckOutput)
                        || validation != 0
                        || matches!(value.ty, Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO)
                        || integer_deadline(arg)
                        || matches!(&value.kind, ExprKind::Builtin(Builtin::FileFileno, _))
                    {
                        Ok(value)
                    } else {
                        Err(error(checker.source, arg,
                            "process redirection requires a file, a literal descriptor, or file.fileno()"))
                    }
                }
                4..=8 => Ok(boolean(false)), // effect-free literals checked by stream_plan
                11 => checker.condition(arg),
                14 if api == LaunchApi::Run => checker.expr_expected(arg, Type::Bool),
                14 => checker.expr(arg), // evaluated before reporting a forbidden check keyword
                15..=17 => Ok(boolean(false)), // literals validated by stream_plan
                12 => checker.deadline_value(arg),
                _ => checker.expr_expected(arg, params[index].ty),
            }
        })?;
        for (param, shift) in [(4, 12), (5, 0), (6, 4)] {
            if (streams >> shift) & 15 == 6
                && matches!(
                    args.values[args.order[param]].ty,
                    Type::BinaryIO | Type::BytesIO | Type::TextIO | Type::StringIO
                )
            {
                streams |= 1 << shift;
            }
        }
        let (op, ty) = match api {
            LaunchApi::CheckOutput => (
                ProcessOperation::CheckOutput {
                    streams,
                    validation,
                    errors,
                    capture,
                },
                capture.stream(true),
            ),
            LaunchApi::Run => (
                ProcessOperation::Run {
                    streams,
                    validation,
                    errors,
                    timed: timeout_argument(call).is_some(),
                },
                self.types.completed(capture, args.values[args.order[0]].ty),
            ),
            LaunchApi::Call(check) => (ProcessOperation::Call { streams, check }, Type::Int),
            LaunchApi::Popen => (
                ProcessOperation::Popen { streams },
                Type::Popen(
                    capture,
                    stdin == 1,
                    self.types.process_args(args.values[args.order[0]].ty),
                ),
            ),
        };
        Ok(self.ordered_builtin(Builtin::Process(op), ty, args))
    }

    pub(super) fn popen_attribute(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        if receiver.ty == Type::ActivePopen {
            let (op, ty) = match attr.attr.as_str() {
                "pid" => (ProcessOperation::PopenPid, Type::Int),
                "returncode" => (
                    ProcessOperation::PopenStatus { poll: false },
                    self.types.optional(Type::Int),
                ),
                _ => {
                    return Err(error(
                        self.source,
                        attr,
                        "this Popen has unknown stream and argument types",
                    ));
                }
            };
            return Ok(operation(op, ty, vec![receiver]));
        }
        let Type::Popen(capture, stdin, cmd) = receiver.ty else {
            unreachable!()
        };
        let (op, ty) = match attr.attr.as_str() {
            name @ ("stdin" | "stdout" | "stderr") => {
                let (index, piped) = match name {
                    "stdin" => (0, stdin),
                    "stdout" => (1, capture.stdout),
                    _ => (2, capture.stderr),
                };
                (
                    ProcessOperation::PopenPipe(index),
                    if !piped {
                        Type::None
                    } else if capture.text {
                        Type::TextIO
                    } else {
                        Type::BinaryIO
                    },
                )
            }
            "args" => (ProcessOperation::PopenArgs, self.types.command_type(cmd)),
            "pid" => (ProcessOperation::PopenPid, Type::Int),
            "returncode" => (
                ProcessOperation::PopenStatus { poll: false },
                self.types.optional(Type::Int),
            ),
            _ => return Err(error(self.source, attr, "unsupported Popen attribute")),
        };
        Ok(operation(op, ty, vec![receiver]))
    }

    pub(super) fn popen_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let mut args = vec![receiver];
        let (op, ty) = match attr.attr.as_str() {
            "poll" if call.arguments.is_empty() => (
                ProcessOperation::PopenStatus { poll: true },
                self.types.optional(Type::Int),
            ),
            "communicate" => {
                let Type::Popen(capture, ..) = args[0].ty else {
                    return Err(error(
                        self.source,
                        call,
                        "cannot infer capture modes for an active child",
                    ));
                };
                let input = self
                    .types
                    .optional(if capture.text { Type::Str } else { Type::Bytes });
                let timeout = self.types.optional(Type::Float);
                let params = [("input", input), ("timeout", timeout)].map(|(name, ty)| Parameter {
                    name: name.into(),
                    ty,
                    kind: ParameterKind::PositionalOrKeyword,
                    default: Some(absent(ty)),
                    sign_only: false,
                });
                let values =
                    self.arguments_with(call, &params, args.pop(), |checker, index, arg| {
                        if index == 1 {
                            checker.deadline_value(arg)
                        } else {
                            checker.expr_expected(arg, input)
                        }
                    })?;
                let pair = self
                    .types
                    .tuple(vec![capture.stream(true), capture.stream(false)]);
                return Ok(self.ordered_builtin(
                    Builtin::Process(ProcessOperation::PopenCommunicate { text: capture.text }),
                    pair,
                    values,
                ));
            }
            "wait" => {
                let timeout = self.types.optional(Type::Float);
                let param = Parameter {
                    name: "timeout".into(),
                    ty: timeout,
                    kind: ParameterKind::PositionalOrKeyword,
                    default: Some(absent(timeout)),
                    sign_only: false,
                };
                let values =
                    self.arguments_with(call, &[param], args.pop(), |checker, _, argument| {
                        checker.deadline_value(argument)
                    })?;
                return Ok(self.ordered_builtin(
                    Builtin::Process(ProcessOperation::PopenWait),
                    Type::Int,
                    values,
                ));
            }
            "send_signal" => {
                let param = Parameter {
                    name: "sig".into(),
                    ty: Type::Int,
                    kind: ParameterKind::PositionalOrKeyword,
                    default: None,
                    sign_only: false,
                };
                let values = self.arguments(call, &[param], args.pop())?;
                return Ok(self.ordered_builtin(
                    Builtin::Process(ProcessOperation::PopenSignal),
                    Type::None,
                    values,
                ));
            }
            name @ ("kill" | "terminate") if call.arguments.is_empty() => {
                args.push(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int(if name == "kill" { "9" } else { "15" }.into()),
                });
                (ProcessOperation::PopenSignal, Type::None)
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "unsupported Popen method or arguments",
                ));
            }
        };
        Ok(operation(op, ty, args))
    }

    fn deadline_value(&mut self, argument: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(argument)?;
        if integer_deadline(argument) && value.ty == Type::Int {
            return Ok(value);
        }
        let timeout = self.types.optional(Type::Float);
        if !matches!(value.ty, Type::Float | Type::None) && value.ty != timeout {
            return Err(error(
                self.source,
                argument,
                "process timeout requires an integer literal or float | None seconds",
            ));
        }
        self.coerce(value, timeout, argument)
    }

    pub(super) fn process_run(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.launch_process(call, LaunchApi::Run)
    }

    /// Path conversion is sealed and pure; optional paths retain their absence.
    pub(super) fn optional_path_argument(
        &mut self,
        expression: &ast::Expr,
    ) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        let expected = self.types.optional(Type::Str);
        if value.ty == Type::Path {
            return self.coerce(
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Path(PathOperation::Text), vec![value]),
                },
                expected,
                expression,
            );
        }
        if self.types.payload(value.ty) == Some(Type::Path) {
            let slot = self.hidden(value.ty);
            let local = Expr {
                ty: value.ty,
                kind: ExprKind::Local(slot),
            };
            let test = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
            };
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    Builtin::Path(PathOperation::Text),
                    vec![Expr {
                        ty: Type::Path,
                        kind: ExprKind::Unwrap(Box::new(local)),
                    }],
                ),
            };
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: expected,
                        kind: ExprKind::Conditional(
                            Box::new(test),
                            Box::new(absent(expected)),
                            Box::new(self.coerce(text, expected, expression)?),
                        ),
                    }),
                    clear: vec![slot],
                },
            });
        }
        self.coerce(value, expected, expression)
    }

    /// A string default keeps a precise string result, while None uses the
    /// zero word of Imre's existing optional-string representation.
    pub(super) fn environ_get(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let optional = self.types.optional(Type::Str);
        let params = [
            Parameter {
                name: "key".to_owned(),
                ty: Type::Str,
                kind: ParameterKind::PositionalOrKeyword,
                default: None,
                sign_only: false,
            },
            Parameter {
                name: "default".to_owned(),
                ty: optional,
                kind: ParameterKind::PositionalOrKeyword,
                default: Some(absent(optional)),
                sign_only: false,
            },
        ];
        let args = self.arguments_with(call, &params, None, |checker, index, argument| {
            if index == 0 {
                return checker.expr_expected(argument, Type::Str);
            }
            let value = checker.expr(argument)?;
            if value.ty == Type::Str {
                Ok(value)
            } else {
                checker.coerce(value, optional, argument)
            }
        })?;
        let returns = args.values[args.order[1]].ty;
        Ok(self.ordered_builtin(
            Builtin::Process(ProcessOperation::EnvironGet),
            returns,
            args,
        ))
    }

    pub(super) fn environ_method(
        &mut self,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if attribute.attr.as_str() == "get" {
            return self.environ_get(call);
        }
        self.reject_builtin_keywords(call)?;
        if attribute.attr.as_str() != "copy" || !call.arguments.args.is_empty() {
            return Err(error(
                self.source,
                call,
                "os.environ accepts copy() or get(key, default=None)",
            ));
        }
        Ok(operation(
            ProcessOperation::EnvironCopy,
            self.types.dict(Type::Str, Type::Str),
            vec![],
        ))
    }

    pub(super) fn process_attribute(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let Type::CompletedProcess(capture, cmd) = receiver.ty else {
            unreachable!()
        };
        let (field, ty) = match attribute.attr.as_str() {
            "args" => (0, self.types.completed_command(cmd)),
            "returncode" => (1, Type::Int),
            "stdout" => (2, capture.stream(true)),
            "stderr" => (3, capture.stream(false)),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "unsupported CompletedProcess attribute",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Field(Box::new(receiver), field),
        })
    }

    pub(super) fn process_method(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        if attribute.attr.as_str() != "check_returncode" || !call.arguments.args.is_empty() {
            return Err(error(
                self.source,
                call,
                "CompletedProcess supports only check_returncode() without arguments",
            ));
        }
        Ok(operation(
            ProcessOperation::CheckReturnCode,
            Type::None,
            vec![receiver],
        ))
    }

    /// Handlers share the Exception type; the runtime guards each fixed payload
    /// projection and raises AttributeError for other exception categories.
    pub(super) fn process_error_attribute(
        &self,
        receiver: &Expr,
        attribute: &ast::ExprAttribute,
    ) -> Option<Expr> {
        let local = if let ExprKind::Local(slot) = receiver.kind {
            self.captured_process_errors
                .iter()
                .rev()
                .find(|capture| capture.slot == slot)
        } else {
            None
        };
        let field = match attribute.attr.as_str() {
            "cmd" => 0,
            "returncode" => 1,
            "stdout" => 2,
            "stderr" => 3,
            "output" => 4,
            _ => return None,
        };
        let timeout = local.is_none_or(|facts| facts.timeout);
        let ty = match (field, local) {
            (
                0,
                Some(CapturedFields {
                    argv: Some(argv), ..
                }),
            ) => *argv,
            (1, _) => Type::Int,
            (
                2..=4,
                Some(CapturedFields {
                    capture: Some(capture),
                    ..
                }),
            ) => error_stream(self.types, *capture, field != 3, timeout),
            _ => error_field_type(self.graph, self.types, field, timeout)?,
        };
        Some(operation(
            ProcessOperation::ErrorField(field | if timeout && field >= 2 { 16 } else { 0 }),
            ty,
            vec![receiver.clone()],
        ))
    }

    /// A proved timeout exposes bytes snapshots without reusing text projections.
    pub(super) fn timeout_error_attribute(
        &self,
        receiver: &Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Option<Expr>, String> {
        let (field, ty) = match attribute.attr.as_str() {
            "cmd" => {
                // The narrowed exception local may be wrapped in a view.
                let local = attribute
                    .value
                    .as_name_expr()
                    .and_then(|name| self.names.get(name.id.as_str()))
                    .and_then(|slot| {
                        self.captured_process_errors
                            .iter()
                            .rev()
                            .find(|capture| capture.slot == *slot)
                    })
                    .and_then(|capture| capture.argv);
                let ty = local
                    .or_else(|| error_field_type(self.graph, self.types, 0, true))
                    .ok_or_else(|| {
                        error(
                            self.source,
                            attribute,
                            "the original timeout command has no single known type",
                        )
                    })?;
                (0, ty)
            }
            "timeout" => (
                1,
                timeout_field_type(self.graph).ok_or_else(|| {
                    error(
                        self.source,
                        attribute,
                        "TimeoutExpired.timeout has both int and float deadlines in this program",
                    )
                })?,
            ),
            "stdout" => (2, self.types.optional(Type::Bytes)),
            "stderr" => (3, self.types.optional(Type::Bytes)),
            "output" => (4, self.types.optional(Type::Bytes)),
            _ => return Ok(None),
        };
        Ok(Some(operation(
            ProcessOperation::TimeoutField(field),
            ty,
            vec![receiver.clone()],
        )))
    }
}
