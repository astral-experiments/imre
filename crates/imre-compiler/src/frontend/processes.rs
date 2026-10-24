//! Closed process signatures preserve evaluation order and exact owned results.

use super::*;
use crate::ir::{PathOperation, ProcessOperation};
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
                && self.graph.resolve(self.module, &call.func)
                    == Some(Binding::Intrinsic(Intrinsic::SubprocessRun))
                && let Some(keyword) = call
                    .arguments
                    .keywords
                    .iter()
                    .find(|kw| kw.arg.as_ref().is_some_and(|name| name == "timeout"))
                && !matches!(keyword.value, ast::Expr::NoneLiteral(_))
            {
                if integer_deadline(&keyword.value) {
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

/// A direct capture and CompletedProcess.check_returncode() produce text.
/// User calls (including property getters) and explicit raises can propagate
/// an error created elsewhere, so their handlers use the program-wide type.
pub(super) fn captures_errors(source: &[ast::Stmt], body: &[Statement]) -> bool {
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
    let mut captured = !raises.0;
    ir::walk_expressions(body, &mut |value| {
        if matches!(
            value.kind,
            ExprKind::Call(..)
                | ExprKind::Invoke { .. }
                | ExprKind::Construct { .. }
                | ExprKind::Builtin(Builtin::Process(ProcessOperation::RunToStderr), _)
        ) {
            captured = false;
        }
    });
    captured
}

pub(super) struct CapturedFields {
    slot: usize,
    argv: Option<Type>,
    inherited_stderr: bool,
}

/// The argv type is shared only if every locally protected launch agrees.
/// Synchronous run captures both streams, while check_output inherits stderr.
pub(super) fn captured_fields(slot: usize, body: &[Statement], types: &Types) -> CapturedFields {
    let mut argv = HashSet::new();
    let mut inherited_stderr = false;
    ir::walk_expressions(body, &mut |expr| {
        if let ExprKind::Builtin(Builtin::Process(op), args) = &expr.kind {
            match op {
                ProcessOperation::CheckOutput => {
                    argv.insert(args[0].ty);
                    inherited_stderr = true;
                }
                ProcessOperation::Run
                | ProcessOperation::RunTimeout
                | ProcessOperation::CheckReturnCode => {
                    argv.insert(types.list(Type::Str));
                }
                _ => {}
            }
        }
    });
    let argv = match argv.len() {
        0 => Some(types.list(Type::Str)),
        1 => argv.into_iter().next(),
        _ => None,
    };
    CapturedFields {
        slot,
        argv,
        inherited_stderr,
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
            return Err(error(
                self.source,
                call,
                "redirected subprocess.run requires one argv list",
            ));
        };
        let argv = self.expr_expected(argv, self.types.list(Type::Str))?;
        let mut check = boolean(false);
        for kw in &call.arguments.keywords {
            match kw.arg.as_ref().map(|name| name.as_str()) {
                Some("check") => check = self.expr_expected(&kw.value, Type::Bool)?,
                Some("stdout")
                    if self.resolve(&kw.value) == Some(Binding::Intrinsic(Intrinsic::Stderr)) => {}
                _ => {
                    return Err(error(
                        self.source,
                        kw,
                        "redirected subprocess.run supports check and stdout=sys.stderr",
                    ));
                }
            }
        }
        Ok(operation(
            ProcessOperation::RunToStderr,
            Type::None,
            vec![argv, check],
        ))
    }

    /// An exact UTF-8 capture inherits stderr. Keep the original argv owner in
    /// CalledProcessError so a path stays a Path and later list edits are visible.
    pub(super) fn process_check_output(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let ([argv], [encoding]) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            return Err(error(
                self.source,
                call,
                "check_output requires an argv list and encoding='utf-8'",
            ));
        };
        if encoding.arg.as_deref() != Some("encoding")
            || !matches!(&encoding.value, ast::Expr::StringLiteral(text) if matches!(text.value.to_str(), "utf8" | "utf-8"))
        {
            return Err(error(
                self.source,
                encoding,
                "check_output requires a literal UTF-8 encoding",
            ));
        }
        let string_or_path = self.types.union(&[Type::Str, Type::Path]).unwrap();
        let argv = if let ast::Expr::List(display) = argv {
            let items = display
                .elts
                .iter()
                .map(|item| self.expr(item))
                .collect::<Result<Vec<_>, _>>()?;
            let element = if items.iter().all(|item| item.ty == Type::Str) {
                Type::Str
            } else if items.iter().all(|item| item.ty == Type::Path) {
                Type::Path
            } else {
                string_or_path
            };
            let items = items
                .into_iter()
                .zip(&display.elts)
                .map(|(item, at)| self.coerce(item, element, at))
                .collect::<Result<Vec<_>, _>>()?;
            Expr {
                ty: self.types.list(element),
                kind: ExprKind::List(items),
            }
        } else {
            self.expr(argv)?
        };
        if !self.types.element(argv.ty).is_some_and(|element| {
            element == Type::Str || element == Type::Path || element == string_or_path
        }) {
            return Err(error(
                self.source,
                call,
                "check_output argv must be a list of str or Path",
            ));
        }
        Ok(operation(
            ProcessOperation::CheckOutput,
            Type::Str,
            vec![argv],
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

    /// Literal mode arguments establish a text-only result. Dynamic values are
    /// rooted in source order before their permutation into the runtime ABI.
    pub(super) fn process_run(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let strings = self.types.list(Type::Str);
        let directory = self.types.optional(Type::Str);
        let environment = self.types.optional(self.types.dict(Type::Str, Type::Str));
        let timeout = self.types.optional(Type::Float);
        let text_mode = call
            .arguments
            .keywords
            .iter()
            .any(|kw| kw.arg.as_ref().is_some_and(|name| name == "text"));
        let params = [
            ("args", strings, None),
            ("cwd", directory, Some(absent(directory))),
            ("env", environment, Some(absent(environment))),
            ("check", Type::Bool, Some(boolean(false))),
            ("capture_output", Type::Bool, None),
            (
                "encoding",
                Type::Str,
                text_mode.then(|| Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str("utf-8".to_owned()),
                }),
            ),
            ("text", Type::Bool, Some(boolean(true))),
            ("timeout", timeout, Some(absent(timeout))),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, ty, default))| Parameter {
            name: name.to_owned(),
            ty,
            kind: if index == 0 {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::KeywordOnly
            },
            default,
            sign_only: false,
        })
        .collect::<Vec<_>>();
        let supplied = self.arguments_with(call, &params, None, |checker, index, argument| {
            match index {
                1 => checker.optional_path_argument(argument),
                4 | 6 => {
                    if !matches!(argument, ast::Expr::BooleanLiteral(value) if value.value) {
                        return Err(error(checker.source, argument, "process capture_output and text require literal True"));
                    }
                    checker.expr(argument)
                }
                5 => {
                    if !matches!(argument, ast::Expr::StringLiteral(value) if value.value.to_string() == "utf-8") {
                        return Err(error(checker.source, argument, "process encoding requires literal 'utf-8'"));
                    }
                    checker.expr(argument)
                }
                7 => {
                    let value = checker.expr(argument)?;
                    if integer_deadline(argument) && value.ty == Type::Int {
                        return Ok(value);
                    }
                    if !matches!(value.ty, Type::Float | Type::None) && value.ty != timeout {
                        return Err(error(checker.source, argument, "process timeout requires an integer literal or float | None seconds"));
                    }
                    checker.coerce(value, timeout, argument)
                }
                _ => checker.expr_expected(argument, params[index].ty),
            }
        })?;
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let timed = call.arguments.keywords.iter().any(|keyword| {
            keyword
                .arg
                .as_ref()
                .is_some_and(|name| name.as_str() == "timeout")
        });
        let mut order = supplied.order[..4].to_vec();
        if timed {
            order.push(supplied.order[7]);
        }
        let mut values = vec![None; supplied.values.len()];
        for (index, value) in supplied.values.into_iter().enumerate() {
            // The validated mode literals have no effects and need no runtime storage.
            if order.contains(&index) {
                let slot = self.hidden(value.ty);
                values[index] = Some(Expr {
                    ty: value.ty,
                    kind: ExprKind::Local(slot),
                });
                body.push(Statement::Assign(slot, value));
                clear.push(slot);
            }
        }
        let args = order
            .iter()
            .map(|&index| values[index].take().unwrap())
            .collect();
        Ok(Expr {
            ty: Type::CompletedProcess,
            kind: ExprKind::Scope {
                body,
                value: Box::new(operation(
                    if timed {
                        ProcessOperation::RunTimeout
                    } else {
                        ProcessOperation::Run
                    },
                    Type::CompletedProcess,
                    args,
                )),
                clear,
            },
        })
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
        let (field, ty) = match attribute.attr.as_str() {
            "args" => (0, self.types.list(Type::Str)),
            "returncode" => (1, Type::Int),
            "stdout" => (2, Type::Str),
            "stderr" => (3, Type::Str),
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
        let (field, mut ty) = match attribute.attr.as_str() {
            "cmd" => (0, self.types.list(Type::Str)),
            "returncode" => (1, Type::Int),
            "stdout" => (2, Type::Str),
            "stderr" => (3, Type::Str),
            "output" => (4, Type::Str),
            _ => return None,
        };
        let local = if let ExprKind::Local(slot) = receiver.kind {
            self.captured_process_errors
                .iter()
                .rev()
                .find(|capture| capture.slot == slot)
        } else {
            None
        };
        if let Some(capture) = local {
            if field == 0 {
                ty = capture.argv?;
            }
            if field == 3 && capture.inherited_stderr {
                ty = self.types.optional(Type::Str);
            }
        } else if matches!(field, 0 | 2..=4) {
            struct Sources<'a> {
                graph: &'a SourceGraph,
                module: ModuleId,
                redirected: bool,
                check_output: bool,
            }
            impl<'ast> Visitor<'ast> for Sources<'_> {
                fn visit_expr(&mut self, expr: &'ast ast::Expr) {
                    if let ast::Expr::Call(call) = expr {
                        match self.graph.resolve(self.module, &call.func) {
                            Some(Binding::Intrinsic(Intrinsic::CheckOutput)) => {
                                self.check_output = true
                            }
                            Some(Binding::Intrinsic(Intrinsic::SubprocessRun)) => {
                                self.redirected |= call
                                    .arguments
                                    .keywords
                                    .iter()
                                    .any(|kw| kw.arg.as_deref() == Some("stdout"));
                            }
                            _ => {}
                        }
                    }
                    walk_expr(self, expr);
                }
            }
            let mut calls = Sources {
                graph: self.graph,
                module: self.module,
                redirected: false,
                check_output: false,
            };
            for (module, source) in self.graph.modules.iter().enumerate() {
                calls.module = module;
                calls.visit_body(&source.parsed.syntax().body);
            }
            // Escaping exceptions may carry argv lists with different invariant
            // element layouts. Only a locally proved argv is projectable then.
            if field == 0 && calls.check_output {
                return None;
            }
            if field != 0 && (calls.redirected || field == 3 && calls.check_output) {
                ty = self.types.optional(ty);
            }
        }
        Some(operation(
            ProcessOperation::ErrorField(field),
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
            "cmd" => (0, self.types.list(Type::Str)),
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
