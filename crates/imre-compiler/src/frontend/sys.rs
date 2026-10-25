//! Sealed bindings for the process's standard streams and exit exception.
use super::*;

impl Checker<'_> {
    /// Read scalar fields of the immutable startup settings selected for a command.
    pub(super) fn command_flag(&self, attr: &ast::ExprAttribute) -> Result<Option<Expr>, String> {
        let Some((flags, _)) = &self.graph.command_startup else {
            return Ok(None);
        };
        if self.resolve(&attr.value) != Some(Binding::Intrinsic(Intrinsic::SysFlags)) {
            return Ok(None);
        }
        if attr.attr.as_str() == "safe_path" {
            return Ok(Some(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(flags.safe_path),
            }));
        }
        let setting = match attr.attr.as_str() {
            "isolated" | "ignore_environment" => flags.isolated,
            "dont_write_bytecode" => flags.dont_write_bytecode,
            "no_user_site" => flags.no_user_site,
            "utf8_mode" => flags.utf8_mode,
            _ => return Err(error(self.source, attr, "unsupported sys.flags field")),
        };
        Ok(Some(pipelines::integer(i64::from(setting))))
    }

    fn sys_no_args(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        result: Expr,
    ) -> Result<Expr, String> {
        if call.arguments.is_empty() {
            return Ok(result);
        }
        let count = call.arguments.args.len();
        let message = if call.arguments.keywords.is_empty() {
            format!("sys.{name}() takes no arguments ({count} given)")
        } else {
            format!("sys.{name}() takes no keyword arguments")
        };
        self.sys_type_error(call, result.ty, message)
    }

    pub(super) fn sys_type_error(
        &mut self,
        call: &ast::ExprCall,
        ty: Type,
        message: String,
    ) -> Result<Expr, String> {
        let mut args = Vec::new();
        for argument in &call.arguments.args {
            args.push(self.expr(argument)?);
        }
        for keyword in &call.arguments.keywords {
            if keyword.arg.is_none() {
                return Err(error(
                    self.source,
                    keyword,
                    "expanded keyword arguments are unsupported",
                ));
            }
            args.push(self.expr(&keyword.value)?);
        }
        args.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        });
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::RaiseException, args),
        })
    }

    pub(super) fn sys_call(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        match self.resolve(&call.func) {
            Some(Binding::Intrinsic(Intrinsic::SysExit)) => {
                return self.system_exit(call, true).map(Some);
            }
            Some(Binding::Intrinsic(Intrinsic::SysGetRecursionLimit)) => {
                calls::reject_unpacking(call, self.source)?;
                for arg in &call.arguments.args {
                    self.graph.checked_excess_argument(self.module, arg.range());
                }
                self.graph.checked_keywords(
                    self.module,
                    call.arguments.keywords.iter().map(Ranged::range),
                );
                let result = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::SysGetRecursionLimit, vec![]),
                };
                return self
                    .sys_no_args(call, "getrecursionlimit", result)
                    .map(Some);
            }
            Some(Binding::Intrinsic(Intrinsic::SysSetRecursionLimit)) => {
                return self.set_recursion_limit(call).map(Some);
            }
            Some(Binding::Intrinsic(Intrinsic::SysEncoding(name))) => {
                return self
                    .sys_no_args(call, name, unpacking::string("utf-8"))
                    .map(Some);
            }
            Some(Binding::Intrinsic(Intrinsic::SysException)) => {
                let result = Expr {
                    ty: self.types.optional(Type::Exception),
                    kind: ExprKind::Builtin(Builtin::ActiveException, Vec::new()),
                };
                return self.sys_no_args(call, "exception", result).map(Some);
            }
            Some(Binding::Intrinsic(Intrinsic::SysJitPredicate(name))) => {
                // Imre emits machine code before the program starts. No running
                // frame has a JIT compiler, regardless of the child's PYTHON_JIT.
                let result = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                };
                return self.sys_no_args(call, name, result).map(Some);
            }
            _ => {}
        }
        let ast::Expr::Attribute(attribute) = &*call.func else {
            return Ok(None);
        };
        let stderr = match self.resolve(&attribute.value) {
            Some(Binding::Intrinsic(Intrinsic::Stdout)) => false,
            Some(Binding::Intrinsic(Intrinsic::Stderr)) => true,
            _ => return Ok(None),
        };
        self.reject_builtin_keywords(call)?;
        let (ty, op, args) = match (attribute.attr.as_str(), call.arguments.args.as_slice()) {
            ("write", [text]) => (
                Type::Int,
                Builtin::SysStreamWrite { stderr },
                vec![self.expr_expected(text, Type::Str)?],
            ),
            ("flush", []) => (Type::None, Builtin::SysStreamFlush { stderr }, Vec::new()),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "standard streams support write(str) and flush()",
                ));
            }
        };
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Builtin(op, args),
        }))
    }

    fn set_recursion_limit(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        if !args.keywords.is_empty() {
            self.graph
                .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
            for keyword in &args.keywords {
                if keyword.arg.as_deref() == Some("limit") {
                    self.graph
                        .checked_positional_keyword(self.module, keyword.range());
                }
            }
            if args.args.is_empty() {
                self.graph
                    .checked_missing_argument(self.module, call.range());
            }
            return self.sys_type_error(
                call,
                Type::None,
                "sys.setrecursionlimit() takes no keyword arguments".to_owned(),
            );
        }
        let [arg] = args.args.as_ref() else {
            if args.args.is_empty() {
                self.graph
                    .checked_missing_argument(self.module, call.range());
            }
            for arg in args.args.iter().skip(1) {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
            return self.sys_type_error(
                call,
                Type::None,
                format!(
                    "sys.setrecursionlimit() takes exactly one argument ({} given)",
                    args.args.len()
                ),
            );
        };
        let value = self.expr(arg)?;
        let index = self.index_word(value, arg, false)?;
        self.graph.checked_argument_type(self.module, arg.range());
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::SysSetRecursionLimit, vec![index]),
        })
    }

    pub(super) fn system_exit(
        &mut self,
        call: &ast::ExprCall,
        raise: bool,
    ) -> Result<Expr, String> {
        let returns = if raise { Type::None } else { Type::Exception };
        if !call.arguments.keywords.is_empty() {
            let message = if raise {
                "sys.exit() takes no keyword arguments"
            } else {
                "SystemExit() takes no keyword arguments"
            };
            return self.sys_type_error(call, returns, message.to_owned());
        }
        if raise && call.arguments.args.len() > 1 {
            let message = format!(
                "exit expected at most 1 argument, got {}",
                call.arguments.args.len()
            );
            return self.sys_type_error(call, returns, message);
        }
        let mut args = Vec::new();
        for arg in &call.arguments.args {
            let value = self.expr(arg)?;
            if !exit_payload_supported(value.ty, self.types) {
                return Err(error(
                    self.source,
                    call,
                    "unsupported SystemExit payload representation",
                ));
            }
            args.push(value);
        }
        // SystemExit(a, b) stores the tuple of constructor arguments. sys.exit
        // passes a tuple as the argument vector to SystemExit, so only the
        // latter unwraps a singleton or omits an empty tuple.
        if args.len() > 1 {
            args = vec![Expr {
                ty: self.types.tuple(args.iter().map(|arg| arg.ty).collect()),
                kind: ExprKind::Tuple(args),
            }];
        }
        let mut evaluate = Vec::new();
        let mut omit_none = raise;
        if raise
            && let [arg] = args.as_slice()
            && matches!(arg.ty, Type::Tuple(_))
        {
            let fields = self.types.fields(arg.ty).unwrap();
            match fields.as_slice() {
                [] => evaluate.push(Statement::Expr(args.pop().unwrap())),
                [ty] => {
                    let tuple = args.pop().unwrap();
                    args.push(Expr {
                        ty: *ty,
                        kind: ExprKind::Field(Box::new(tuple), 0),
                    });
                    omit_none = false;
                }
                _ => {}
            }
        }
        let mut exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Builtin(Builtin::SystemExitNew { omit_none }, args),
        };
        if !evaluate.is_empty() {
            exception = Expr {
                ty: Type::Exception,
                kind: ExprKind::Scope {
                    body: evaluate,
                    clear: Vec::new(),
                    value: Box::new(exception),
                },
            };
        }
        Ok(if raise {
            Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::RaiseException, vec![exception]),
            }
        } else {
            exception
        })
    }
}

/// Exit payloads that the runtime can retain, compare, and format without boxing away their type.
pub(super) fn exit_payload_supported(ty: Type, types: &Types) -> bool {
    match ty {
        Type::None | Type::Bool | Type::Int | Type::Str | Type::StrLiteral(_) => true,
        Type::Tuple(_) => types
            .fields(ty)
            .unwrap()
            .iter()
            .all(|&field| exit_payload_supported(field, types)),
        _ => false,
    }
}

impl Checker<'_> {
    pub(super) fn sys_version_info(&self, implementation: bool) -> Expr {
        Expr {
            ty: self.types.struct_sequence(
                "sys.version_info",
                crate::version::FIELDS,
                vec![Type::Int, Type::Int, Type::Int, Type::Str, Type::Int],
            ),
            kind: ExprKind::Builtin(Builtin::SysVersionInfo { implementation }, vec![]),
        }
    }
}
