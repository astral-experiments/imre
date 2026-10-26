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
    /// Return the callable currently bound in the native builtins namespace.
    pub(super) fn builtin_len(&self) -> Expr {
        Expr {
            ty: self
                .types
                .callable(vec![self.types.len_argument()], Type::Int),
            kind: ExprKind::Builtin(Builtin::BuiltinsLen, vec![]),
        }
    }

    pub(super) fn unshadowed_len(&self, expr: &ast::Expr) -> bool {
        matches!(expr, ast::Expr::Name(name) if name.id == "len" && !self.names.contains_key("len"))
            && self.resolve(expr).is_none()
    }

    /// Pack and retain the actual argument for a direct, cached or iterator call to len.
    pub(super) fn pack_len_value(
        &mut self,
        value: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let ty = self.types.len_argument();
        if value.ty == ty {
            return Ok(value);
        }
        let (class, unhashable) = len_argument_class(self.types, value.ty).ok_or_else(|| {
            error(
                self.source,
                node,
                "this live len call requires a supported builtin argument layout",
            )
        })?;
        let diagnostic = unhashable
            .map(|name| format!("unhashable type: '{name}'"))
            .unwrap_or_default();
        let mut scope = calls::ArgumentScope::default();
        let value = scope.hold(self, value);
        let mut key_fields = vec![unpacking::string(class)];
        if unhashable.is_none() {
            key_fields.push(normalize_len_key(self.types, value.clone()));
        }
        let key = Expr {
            ty: self.types.tuple(key_fields.iter().map(|f| f.ty).collect()),
            kind: ExprKind::Tuple(key_fields),
        };
        let key = Expr {
            ty: self.types.len_key(),
            kind: ExprKind::Builtin(Builtin::RecordView, vec![key]),
        };
        let frame = Expr {
            ty: self
                .types
                .tuple(vec![Type::Str, Type::Str, key.ty, value.ty]),
            kind: ExprKind::Tuple(vec![
                unpacking::string(class),
                unpacking::string(diagnostic),
                key,
                value,
            ]),
        };
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::RecordView, vec![frame]),
        }))
    }

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
                    ty: Type::ObjectValue,
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
                    | "errors"
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

    /// Evaluate the metadata receiver before looking up its field name.
    pub(super) fn implementation_hasattr(&mut self, receiver: Expr, name: Expr) -> Expr {
        let names = Expr {
            ty: self.types.list(Type::Str),
            kind: ExprKind::List(
                crate::version::IMPLEMENTATION
                    .iter()
                    .map(|s| unpacking::string(*s))
                    .collect(),
            ),
        };
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: vec![Statement::Expr(receiver)],
                clear: vec![],
                value: Box::new(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::StringListContains, vec![name, names]),
                }),
            },
        }
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
            Some(Binding::Intrinsic(Intrinsic::SysGetFrameModuleName)) => {
                return self.frame_module_name(call).map(Some);
            }
            Some(Binding::Intrinsic(Intrinsic::SysEncoding(query))) => {
                calls::reject_unpacking(call, self.source)?;
                let result = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::SysEncoding(query), vec![]),
                };
                let result = self.sys_no_args(call, query.method(), result)?;
                for arg in &call.arguments.args {
                    self.graph.checked_excess_argument(self.module, arg.range());
                }
                self.graph.checked_keywords(
                    self.module,
                    call.arguments.keywords.iter().map(Ranged::range),
                );
                return Ok(Some(result));
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

    /// Bind the optional index to the current native stack. CPython accepts `depth=` here.
    fn frame_module_name(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        let ty = self.types.optional(Type::Str);
        let count = args.args.len() + args.keywords.len();
        if count > 1 {
            for arg in args.args.iter().skip(1) {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
            if !args.args.is_empty() {
                for keyword in &args.keywords {
                    if keyword.arg.as_deref() == Some("depth") {
                        self.graph
                            .checked_repeated_keyword(self.module, keyword.range());
                    }
                }
            }
            self.graph
                .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
            let kind = if args.args.is_empty() {
                "keyword argument"
            } else {
                "argument"
            };
            return self.sys_type_error(
                call,
                ty,
                format!("_getframemodulename() takes at most 1 {kind} ({count} given)"),
            );
        }
        let arg = if let Some(keyword) = args.keywords.first() {
            let name = keyword.arg.as_ref().expect("unpacking rejected above");
            if name != "depth" {
                self.graph
                    .checked_keywords(self.module, std::iter::once(keyword.range()));
                return self.sys_type_error(
                    call,
                    ty,
                    format!("_getframemodulename() got an unexpected keyword argument '{name}'"),
                );
            }
            Some(&keyword.value)
        } else {
            args.args.first()
        };
        let depth = if let Some(arg) = arg {
            let value = self.expr(arg)?;
            self.graph.checked_argument_type(self.module, arg.range());
            self.index_word(value, arg, false)?
        } else {
            pipelines::integer(0)
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::SysGetFrameModuleName, vec![depth]),
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

/// Ordinary tuple equality equates bool with int. Only the private cache key is widened.
fn normalize_len_key(types: &Types, value: Expr) -> Expr {
    match value.ty {
        Type::Bool => Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Int, vec![value]),
        },
        Type::StrLiteral(_) => literals::view(value, Type::Str),
        Type::Tuple(_) => {
            let fields: Vec<_> = types
                .len_tuple_fields(value.ty)
                .unwrap()
                .into_iter()
                .enumerate()
                .map(|(i, ty)| {
                    normalize_len_key(
                        types,
                        Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(value.clone()), i),
                        },
                    )
                })
                .collect();
            Expr {
                ty: types.tuple(fields.iter().map(|field| field.ty).collect()),
                kind: ExprKind::Tuple(fields),
            }
        }
        _ => value,
    }
}

/// The first mutable tuple element supplies the class name in a hashing error.
fn len_argument_class(types: &Types, ty: Type) -> Option<(&'static str, Option<&'static str>)> {
    let pair = match ty {
        Type::Str | Type::StrLiteral(_) => ("str", None),
        Type::Bytes => ("bytes", None),
        Type::Int => ("int", None),
        Type::Bool => ("bool", None),
        Type::None => ("NoneType", None),
        // The container supplies its own length and is itself unhashable;
        // neither operation visits its elements.
        Type::ListInt | Type::List(_) => ("list", Some("list")),
        Type::Dict(_) => ("dict", Some("dict")),
        Type::Set(_) => ("set", Some("set")),
        Type::Tuple(_) => {
            let mut invalid = None;
            for child in types.len_tuple_fields(ty)? {
                let (_, child_error) = len_argument_class(types, child)?;
                invalid = invalid.or(child_error);
            }
            ("tuple", invalid)
        }
        _ => return None,
    };
    Some(pair)
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
            ty: crate::version::info_type(self.types),
            kind: ExprKind::Builtin(Builtin::SysVersionInfo { implementation }, vec![]),
        }
    }
}
