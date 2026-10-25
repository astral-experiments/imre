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
            | Type::ObjectValue
    )
}
impl Checker<'_> {
    /// Read scalar fields of the immutable startup settings selected for a command.
    pub(super) fn command_flag(&self, attr: &ast::ExprAttribute) -> Result<Option<Expr>, String> {
        if self.resolve(&attr.value) != Some(Binding::Intrinsic(Intrinsic::SysFlags)) {
            return Ok(None);
        }
        if attr.attr.as_str() == "optimize" {
            return Ok(Some(pipelines::integer(i64::from(
                self.graph.python_optimization(),
            ))));
        }
        let Some((flags, _)) = &self.graph.command_startup else {
            return Ok(None);
        };
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
        Ok(Self::type_error_after(args, ty, message))
    }

    /// Evaluate the supplied operands before raising a native argument error.
    pub(super) fn type_error_after(mut args: Vec<Expr>, ty: Type, message: String) -> Expr {
        args.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        });
        Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::RaiseException, args),
        }
    }

    fn intern_call(&mut self, call: &ast::ExprCall, query: bool) -> Result<Expr, String> {
        let (name, result, op) = if query {
            ("_is_interned", Type::Bool, Builtin::SysIsInterned)
        } else {
            ("intern", Type::Str, Builtin::SysIntern)
        };
        let count = call.arguments.args.len();
        if !query && (count != 1 || !call.arguments.keywords.is_empty()) {
            // Typeshed overloads intern() for LiteralString. This binder
            // raises the real argument-count or keyword error at runtime.
            self.graph.checked_overload(self.module, call.range());
        }
        if count == 0 {
            self.graph
                .checked_missing_argument(self.module, call.range());
        } else if let Some(extra) = call.arguments.args.get(1) {
            self.graph
                .checked_excess_argument(self.module, extra.range());
        }
        if !call.arguments.keywords.is_empty() {
            self.graph.checked_keywords(
                self.module,
                call.arguments.keywords.iter().map(Ranged::range),
            );
            return self.sys_type_error(
                call,
                result,
                format!("sys.{name}() takes no keyword arguments"),
            );
        }
        let [argument] = call.arguments.args.as_slice() else {
            return self.sys_type_error(
                call,
                result,
                format!("sys.{name}() takes exactly one argument ({count} given)"),
            );
        };
        let value = self.expr(argument)?;
        if !query {
            self.graph.checked_overload(self.module, call.range());
        }
        if value.ty == Type::Str {
            return Ok(Expr {
                ty: result,
                kind: ExprKind::Builtin(op, vec![value]),
            });
        }
        if let Type::StrEnum(class) = value.ty {
            if query {
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Scope {
                        body: vec![Statement::Expr(value)],
                        value: Box::new(Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Bool(false),
                        }),
                        clear: vec![],
                    },
                });
            }
            let class = &self.classes.declarations.definitions[class].name;
            return Ok(Self::type_error_after(
                vec![value],
                result,
                format!("can't intern {class}"),
            ));
        }
        let ty = match value.ty {
            Type::None => "None",
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::ListInt | Type::List(_) => "list",
            Type::Tuple(_) => "tuple",
            Type::Dict(_) => "dict",
            Type::Set(_) => "set",
            Type::Class(class) => &self.classes.declarations.definitions[class].name,
            _ => {
                return Err(error(
                    self.source,
                    argument,
                    "interning requires a concrete native argument type",
                ));
            }
        };
        Ok(Self::type_error_after(
            vec![value],
            result,
            format!("{name}() argument must be str, not {ty}"),
        ))
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
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "hasattr" && !self.names.contains_key("hasattr"))
            && self.resolve(&call.func).is_none()
            && call.arguments.keywords.is_empty()
            && let [module, ast::Expr::StringLiteral(attr)] = call.arguments.args.as_slice()
            && let Some(Binding::IntrinsicModule(namespace @ crate::modules::IntrinsicModule::Sys)) =
                self.resolve(module)
            && matches!(
                namespace.symbol(attr.value.to_str()),
                Some(Binding::Intrinsic(
                    Intrinsic::SysIntern { .. }
                        | Intrinsic::SysEncoding(_)
                        | Intrinsic::SysException
                        | Intrinsic::SysExit
                ))
            )
        {
            return Ok(Some(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(true),
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
            Some(Binding::Intrinsic(Intrinsic::SysIntern { query })) => {
                return self.intern_call(call, query).map(Some);
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
        let mut from_tuple = args.len() > 1;
        if from_tuple {
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
                _ => from_tuple = true,
            }
        }
        let mut exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Builtin(
                if from_tuple {
                    Builtin::SystemExitFromTuple
                } else {
                    Builtin::SystemExitNew { omit_none }
                },
                args,
            ),
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
    /// Expose the thread provider and its optional library version as a native struct sequence.
    pub(super) fn sys_thread_info(&self) -> Expr {
        let text = self.types.optional(Type::Str);
        Expr {
            ty: self.types.struct_sequence(
                "sys.thread_info",
                &["name", "lock", "version"],
                vec![text, Type::None, text],
            ),
            kind: ExprKind::Builtin(Builtin::SysThreadInfo, vec![]),
        }
    }

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
