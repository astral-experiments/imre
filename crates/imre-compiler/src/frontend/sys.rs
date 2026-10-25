//! Sealed bindings for the process's standard streams and exit exception.
use super::*;

pub(super) fn displayable(ty: Type) -> bool {
    matches!(
        ty,
        Type::None
            | Type::Bool
            | Type::Int
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::ListInt
            | Type::ValueView
    )
}
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

    pub(super) fn stdout_value(&self) -> Expr {
        Expr {
            ty: self.types.optional(Type::TextIO),
            kind: ExprKind::Builtin(Builtin::SysStdout, vec![]),
        }
    }

    /// A missing from-import constructs the same structured ImportError as a Python call.
    pub(super) fn import_sys_value(&self, symbol: Intrinsic) -> Expr {
        let (module, name, present, value) = match symbol {
            Intrinsic::Stdout => ("sys", "stdout", Builtin::SysHasStdout, self.stdout_value()),
            Intrinsic::BuiltinsLast => (
                "builtins",
                "_",
                Builtin::BuiltinsHasLast,
                Expr {
                    ty: Type::ValueView,
                    kind: ExprKind::Builtin(Builtin::BuiltinsLast, vec![]),
                },
            ),
            _ => unreachable!("only mutable sys and builtins exports need an import snapshot"),
        };
        let error = self.import_error_value(
            exception_kind("ImportError").unwrap(),
            vec![unpacking::string(format!(
                "cannot import name '{name}' from '{module}' (unknown location)"
            ))],
            vec![unpacking::string(module), unpacking::string(name)],
            [Some(1), None, Some(2)],
        );
        let missing = Expr {
            ty: value.ty,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![error]),
        };
        Expr {
            ty: value.ty,
            kind: ExprKind::Conditional(
                Box::new(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(present, vec![]),
                }),
                Box::new(value),
                Box::new(missing),
            ),
        }
    }

    /// Bind a mutable export at the local import statement, inside any active handler.
    pub(super) fn local_sys_import(
        &mut self,
        import: &ast::StmtImportFrom,
    ) -> Result<Option<Vec<Statement>>, String> {
        let Some(module) = &import.module else {
            return Ok(None);
        };
        if import.level != 0 || import.is_lazy || !matches!(module.as_str(), "sys" | "builtins") {
            return Ok(None);
        }
        let mut bindings = Vec::new();
        for alias in &import.names {
            let symbol = match (module.as_str(), alias.name.as_str()) {
                ("sys", "stdout") => Intrinsic::Stdout,
                ("builtins", "_") => Intrinsic::BuiltinsLast,
                _ => {
                    return Err(error(
                        self.source,
                        alias,
                        "unsupported local import from this native module",
                    ));
                }
            };
            let value = self.import_sys_value(symbol);
            let name = alias.asname.as_ref().unwrap_or(&alias.name);
            let slot = self.declare(name.as_str(), value.ty, alias)?;
            if symbol == Intrinsic::BuiltinsLast {
                self.graph
                    .checked_builtin_import(self.module, alias.range());
            }
            self.assigned_value(slot, &value);
            bindings.push(Statement::Assign(slot, value));
        }
        Ok(Some(bindings))
    }

    /// Preserve lookup of an optional text stream before evaluating method arguments.
    pub(super) fn optional_stream(&self, value: Expr, name: &str) -> Expr {
        if self.types.payload(value.ty) == Some(Type::TextIO)
            && matches!(
                name,
                "write"
                    | "flush"
                    | "close"
                    | "read"
                    | "readline"
                    | "encoding"
                    | "closed"
                    | "fileno"
            )
        {
            Expr {
                ty: Type::TextIO,
                kind: ExprKind::Builtin(
                    Builtin::TextOptionalReceiver,
                    vec![value, unpacking::string(name)],
                ),
            }
        } else {
            value
        }
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
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "hasattr" && !self.names.contains_key("hasattr"))
            && self.resolve(&call.func).is_none()
            && call.arguments.keywords.is_empty()
            && let [module, ast::Expr::StringLiteral(name)] = call.arguments.args.as_slice()
            && self.resolve(module)
                == Some(Binding::IntrinsicModule(
                    crate::modules::IntrinsicModule::Builtins,
                ))
            && name.value.to_str() == "_"
        {
            return Ok(Some(Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::BuiltinsHasLast, vec![]),
            }));
        }
        match self.resolve(&call.func) {
            Some(Binding::Intrinsic(Intrinsic::SysDisplayHook)) => {
                if !call.arguments.keywords.is_empty() {
                    return self
                        .sys_type_error(
                            call,
                            Type::None,
                            "sys.displayhook() takes no keyword arguments".to_owned(),
                        )
                        .map(Some);
                }
                let [value] = call.arguments.args.as_slice() else {
                    return self
                        .sys_type_error(
                            call,
                            Type::None,
                            format!(
                                "sys.displayhook() takes exactly one argument ({} given)",
                                call.arguments.args.len()
                            ),
                        )
                        .map(Some);
                };
                let value = self.expr(value)?;
                if !displayable(value.ty) {
                    return Err(error(
                        self.source,
                        call,
                        "sys.displayhook requires a supported native representation",
                    ));
                }
                return Ok(Some(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::SysDisplayHook, vec![value]),
                }));
            }
            Some(Binding::Intrinsic(Intrinsic::SysExit)) => {
                return self.system_exit(call, true).map(Some);
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
            Some(Binding::Intrinsic(Intrinsic::Stdout | Intrinsic::OriginalStdout)) => {
                // The ordinary TextIO method path evaluates and retains the receiver first.
                return Ok(None);
            }
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
