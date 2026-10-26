//! Bind the standard logger registry and keep message conversion behind the level check.
use super::*;
use ir::LogOperation as Log;

fn operation(op: Log, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Logging(op), args),
    }
}

pub(super) fn number(level: u8) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(level.to_string()),
    }
}

fn text(value: &str) -> Expr {
    Expr {
        ty: Type::Str,
        kind: ExprKind::Str(value.to_owned()),
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

fn parameter(name: &str, ty: Type, kind: ParameterKind, default: Expr) -> Parameter {
    Parameter {
        name: name.to_owned(),
        ty,
        kind,
        default: Some(default),
        sign_only: false,
    }
}

fn required(name: &str, ty: Type) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        kind: ParameterKind::PositionalOrKeyword,
        default: None,
        sign_only: false,
    }
}

impl Checker<'_> {
    fn handler_type(&self) -> Option<Type> {
        let id = self.graph.logging_definition("Handler")?;
        Some(Type::Class(*self.classes.declarations.ids.get(&id)?))
    }

    /// Register the completed record so a shutdown callback cannot observe uninitialized fields.
    pub(super) fn register_logging_handler(&self, class: usize, record: Expr) -> Expr {
        if let Some(Type::Class(base)) = self.handler_type()
            && (class == base
                || self.classes.declarations.definitions[class]
                    .bases
                    .contains(&base))
            && let Some(shutdown) = self.graph.logging_definition("_shutdown_handler")
        {
            return operation(
                Log::HandlerRegister(self.functions[&shutdown].id),
                record.ty,
                vec![record],
            );
        }
        record
    }

    pub(super) fn text_output_method(
        &mut self,
        stream: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if matches!(attribute.attr.as_str(), "close" | "fileno") {
            let file = operation(Log::StreamFile, Type::TextIO, vec![stream]);
            let args = self.arguments(call, &[], Some(file))?;
            let (builtin, ty) = if attribute.attr.as_str() == "close" {
                (Builtin::TextClose, Type::None)
            } else {
                (Builtin::FileFileno, Type::Int)
            };
            return Ok(self.ordered_builtin(builtin, ty, args));
        }
        let (op, ty, params) = match attribute.attr.as_str() {
            "write" => (
                Log::WriteStream,
                self.types.optional(Type::Int),
                vec![required("s", Type::Str)],
            ),
            "flush" => (Log::FlushStream, Type::None, vec![]),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "writable text stream supports write, flush, close and fileno",
                ));
            }
        };
        let args = self.arguments(call, &params, Some(stream))?;
        Ok(self.ordered_builtin(Builtin::Logging(op), ty, args))
    }

    /// Bind the private operations used by the public handler implementation.
    pub(super) fn handler_operation(
        &mut self,
        symbol: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let source = &self.graph.modules[self.module];
        let Some(Binding::Definition(def)) = source.bindings.get("Handler") else {
            return Err(error(
                self.source,
                call,
                "missing logging.Handler definition",
            ));
        };
        let handler = Type::Class(self.classes.declarations.ids[def]);
        let optional_name = self.types.optional(Type::Str);
        let (op, result, params) = match symbol {
            Intrinsic::HandlerFallback => (
                Log::Emit,
                Type::None,
                vec![
                    required("logger", Type::Logger),
                    required("level", Type::Int),
                    required("record", Type::LogRecord),
                    required("found", Type::Bool),
                ],
            ),
            Intrinsic::HandlerRename => (
                Log::HandlerRename,
                Type::None,
                vec![
                    required("handler", handler),
                    required("old", optional_name),
                    required("new", optional_name),
                ],
            ),
            Intrinsic::HandlerLookup => (
                Log::HandlerLookup,
                self.types.optional(handler),
                vec![required("name", Type::Str)],
            ),
            Intrinsic::FileEncodingUsesUtf8 => (Log::FileEncodingUsesUtf8, Type::Bool, vec![]),
            Intrinsic::PrintActiveException => (Log::PrintActiveException, Type::None, vec![]),
            _ => unreachable!(),
        };
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::Logging(op), result, args))
    }

    /// A configuration literal can contain differently typed values. Give each
    /// nested map and list its ordered, mutable document layout at construction.
    pub(super) fn logging_dictionary_config(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let parameters = [required("config", documents::MAP)];
        let mut scalar = None;
        let mut args = self.arguments_with(call, &parameters, None, |checker, _, argument| {
            if matches!(argument, ast::Expr::Dict(_)) {
                return checker.expr_expected(argument, documents::MAP);
            }
            let value = checker.expr(argument)?;
            if let Some((Type::Str, ty)) = checker.types.mapping(value.ty) {
                scalar = match ty {
                    Type::None => Some(0),
                    Type::Bool => Some(1),
                    Type::Int => Some(2),
                    Type::Str => Some(3),
                    Type::Float => Some(7),
                    _ => None,
                };
                if scalar.is_some() {
                    return Ok(value);
                }
            }
            checker.document_coerce(value, documents::MAP, argument)
        })?;
        let op = if let Some(kind) = scalar {
            args.order.push(args.values.len());
            args.values.push(number(kind));
            Log::DictConfigTyped
        } else {
            Log::DictConfig
        };
        Ok(self.ordered_builtin(Builtin::Logging(op), Type::None, args))
    }

    /// Read the configuration path only after evaluating all call arguments.
    pub(super) fn logging_file_config(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let parameters = [
            required("fname", Type::Str),
            parameter(
                "defaults",
                Type::None,
                ParameterKind::PositionalOrKeyword,
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
            ),
            parameter(
                "disable_existing_loggers",
                Type::Bool,
                ParameterKind::PositionalOrKeyword,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                },
            ),
            parameter(
                "encoding",
                self.types.optional(Type::Str),
                ParameterKind::PositionalOrKeyword,
                absent(self.types.optional(Type::Str)),
            ),
        ];
        let args = self.arguments(call, &parameters, None)?;
        Ok(self.ordered_builtin(Builtin::Logging(Log::FileConfig), Type::None, args))
    }

    pub(super) fn logging_level_map(&self, names: bool) -> Expr {
        let (op, key, value) = if names {
            (Log::Names, Type::Str, Type::Int)
        } else {
            (Log::Numbers, Type::Int, Type::Str)
        };
        operation(op, self.types.dict(key, value), vec![])
    }

    pub(super) fn logging_levels(
        &mut self,
        symbol: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (op, ty, args) = match symbol {
            Intrinsic::AddLevelName => (
                Log::AddLevel,
                Type::None,
                self.arguments(
                    call,
                    &[
                        required("level", Type::Int),
                        required("levelName", Type::Str),
                    ],
                    None,
                )?,
            ),
            Intrinsic::LoggingDisable => {
                let params = [parameter(
                    "level",
                    Type::Int,
                    ParameterKind::PositionalOrKeyword,
                    number(50),
                )];
                (
                    Log::Disable,
                    Type::None,
                    self.arguments_with(call, &params, None, |checker, _, arg| {
                        checker.checked_logging_level(arg)
                    })?,
                )
            }
            Intrinsic::LevelNamesMapping => (
                Log::NamesCopy,
                self.types.dict(Type::Str, Type::Int),
                self.arguments(call, &[], None)?,
            ),
            Intrinsic::GetLevelName => {
                let args = self.arguments_with(
                    call,
                    &[required("level", Type::Int)],
                    None,
                    |checker, _, arg| Ok(Self::string_value(checker.expr(arg)?)),
                )?;
                let (op, ty) = match args.values[0].ty {
                    Type::Int => (Log::LevelName, Type::Str),
                    Type::Str => (Log::LevelForName, self.types.string_or_int()),
                    _ => {
                        return Err(error(
                            self.source,
                            call,
                            "expected a string or integer logging level",
                        ));
                    }
                };
                (op, ty, args)
            }
            _ => unreachable!(),
        };
        Ok(self.ordered_builtin(Builtin::Logging(op), ty, args))
    }

    pub(super) fn root_logger(&self) -> Expr {
        operation(
            Log::Get,
            Type::Logger,
            vec![absent(self.types.optional(Type::Str))],
        )
    }

    pub(super) fn get_logger(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let name = self.types.optional(Type::Str);
        let params = [parameter(
            "name",
            name,
            ParameterKind::PositionalOrKeyword,
            absent(name),
        )];
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::Logging(Log::Get), Type::Logger, args))
    }

    /// Validate the level inside the constructor, after all caller expressions ran.
    pub(super) fn new_logger(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [
            required("name", Type::Str),
            parameter(
                "level",
                Type::Int,
                ParameterKind::PositionalOrKeyword,
                number(0),
            ),
        ];
        let mut scope = calls::ArgumentScope::default();
        let args = self.bind_supplied(
            call,
            &params,
            None,
            Some(&mut scope),
            None,
            |checker, index, arg| {
                if index == 0 {
                    arg.checked(checker, Type::Str, false)
                } else {
                    Ok(Self::string_value(arg.value(checker)?))
                }
            },
        )?;
        let name = args.values[args.order[0]].clone();
        let level = self.checked_level_value(args.values[args.order[1]].clone(), call)?;
        Ok(scope.finish(operation(Log::New, Type::Logger, vec![name, level])))
    }

    /// Invoke the bundled implementation after binding the caller's arguments in source order.
    fn logging_source_call(
        &mut self,
        name: &str,
        args: ir::CallArgs,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let sig = self
            .graph
            .logging_definition(name)
            .and_then(|def| self.functions.get(&def))
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    &format!("missing native logging function {name}"),
                )
            })?;
        self.calls.insert(sig.id);
        Ok(self.call_value(sig, sig.returns, args))
    }

    pub(super) fn configure_logging(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let level = self.types.optional(Type::Int);
        let string = self.types.optional(Type::Str);
        let output = self.types.optional(Type::TextOutput);
        let handler = self
            .handler_type()
            .ok_or_else(|| error(self.source, call, "missing logging.Handler"))?;
        let list = self.types.list(handler);
        let handlers = self.types.optional(list);
        let filename = self
            .types
            .union(&[Type::Str, Type::Path, Type::None])
            .expect("optional filename");
        let default_errors = self.coerce(text("backslashreplace"), string, call)?;
        let kw = ParameterKind::KeywordOnly;
        let boolean = |n| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(n),
        };
        let mut params = vec![
            parameter("level", level, kw, absent(level)),
            parameter("format", string, kw, absent(string)),
            parameter("datefmt", string, kw, absent(string)),
            parameter("force", Type::Bool, kw, boolean(false)),
            parameter("stream", output, kw, absent(output)),
            parameter("style", Type::Str, kw, text("%")),
            parameter("handlers", handlers, kw, absent(handlers)),
            parameter("filename", filename, kw, absent(filename)),
            parameter("filemode", Type::Str, kw, text("a")),
            parameter("encoding", string, kw, absent(string)),
            parameter("errors", string, kw, default_errors),
        ];
        let known = params.len();
        let mut unused = Vec::new();
        let mut unused_with_handlers = Vec::new();
        for keyword in &call.arguments.keywords {
            let Some(name) = keyword.arg.as_ref().map(|name| name.as_str()) else {
                continue;
            };
            if !params[..known].iter().any(|p| p.name == name) {
                unused.push(name);
                unused_with_handlers.push(name);
                if !params[known..].iter().any(|p| p.name == name) {
                    params.push(Parameter {
                        kind: kw,
                        ..required(name, Type::None)
                    });
                }
            } else if name == "filemode" {
                unused_with_handlers.push(name);
            }
        }
        let mut name = false;
        let mut args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index == 0 {
                let value = Self::string_value(checker.expr(arg)?);
                if value.ty == Type::Str {
                    name = true;
                    return checker.coerce(value, string, arg);
                }
                return checker.coerce(value, level, arg);
            }
            if index >= known {
                return checker.expr(arg);
            }
            if index == 6 && !matches!(arg, ast::Expr::List(_) | ast::Expr::ListComp(_)) {
                let value = checker.expr(arg)?;
                // The private helper only reads this list. Preserve the original list and
                // its evaluation order without widening the caller's mutable list contract.
                if let (Some(Type::Class(child)), Type::Class(base)) =
                    (checker.types.element(value.ty), handler)
                    && checker.classes.declarations.definitions[child]
                        .bases
                        .contains(&base)
                {
                    let view = Expr {
                        ty: list,
                        kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
                    };
                    return checker.coerce(view, handlers, arg);
                }
                return checker.coerce(value, handlers, arg);
            }
            checker.expr_expected(arg, params[index].ty)
        })?;
        let mut scope = calls::ArgumentScope::default();
        if !unused.is_empty() {
            // Hold every operand in call order, including the options used only
            // for their names. Pass the known parameters to the bundled function.
            let held: Vec<_> = args
                .values
                .into_iter()
                .map(|v| scope.hold(self, v))
                .collect();
            args.values = args.order[..known]
                .iter()
                .map(|&i| held[i].clone())
                .collect();
            args.order = (0..known).collect();
        }
        let extra = args.values.len();
        if name {
            args.order.push(args.order[0]);
            args.order[0] = extra;
            args.values.push(absent(level));
        } else {
            args.order.push(extra);
            args.values.push(absent(string));
        }
        let supplied = |s| {
            call.arguments
                .keywords
                .iter()
                .any(|k| k.arg.as_ref().is_some_and(|n| n.as_str() == s))
        };
        for flag in [
            !supplied("format"),
            supplied("stream"),
            supplied("filename"),
        ] {
            args.order.push(args.values.len());
            args.values.push(boolean(flag));
        }
        // Whether these options conflict depends on the live root. The native
        // binder has already checked the type of every argument expression.
        let runtime_options = !unused.is_empty()
            || (supplied("filename") && supplied("stream"))
            || (supplied("handlers")
                && ["filename", "stream", "filemode"]
                    .iter()
                    .any(|s| supplied(s)));
        for names in [unused, unused_with_handlers] {
            args.order.push(args.values.len());
            args.values.push(text(&names.join(", ")));
        }
        let result = self.logging_source_call("_basic_config", args, call)?;
        if runtime_options {
            self.graph.checked_overload(self.module, call.range());
        }
        Ok(scope.finish(result))
    }

    /// Share _checkLevel between global suppression and logger thresholds.
    fn checked_logging_level(&mut self, arg: &ast::Expr) -> Result<Expr, String> {
        let value = Self::string_value(self.expr(arg)?);
        self.checked_level_value(value, arg)
    }

    fn checked_level_value(&self, value: Expr, arg: &impl Ranged) -> Result<Expr, String> {
        match value.ty {
            Type::Int => Ok(value),
            Type::Bool => Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
            }),
            Type::Str => Ok(operation(Log::CheckLevel, Type::Int, vec![value])),
            ty if dataclass_values::can_format(ty, self.types, self.classes)
                && !matches!(ty, Type::Class(_)) =>
            {
                let description = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
                };
                Ok(operation(Log::InvalidLevel, Type::Int, vec![description]))
            }
            _ => Err(error(
                self.source,
                arg,
                "unable to represent this invalid logging level",
            )),
        }
    }

    pub(super) fn new_log_manager(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let root = self.types.optional(Type::Logger);
        let args = self.arguments(call, &[required("rootnode", root)], None)?;
        Ok(self.ordered_builtin(Builtin::Logging(Log::NewManager), Type::LogManager, args))
    }

    pub(super) fn log_manager_attribute(
        &self,
        manager: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (op, ty) = match attr.attr.as_str() {
            "root" => (Log::ManagerRoot, self.types.optional(Type::Logger)),
            "disable" => (Log::ManagerDisable, Type::Int),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "unsupported logging.Manager attribute",
                ));
            }
        };
        Ok(operation(op, ty, vec![manager]))
    }

    /// The property validates its argument after both assignment expressions have run.
    pub(super) fn set_manager_disable(
        &mut self,
        manager: Expr,
        value: Expr,
        target: &ast::Expr,
    ) -> Result<Statement, String> {
        let mut scope = calls::ArgumentScope::default();
        let value = scope.hold(self, Self::string_value(value));
        let manager = scope.hold(self, manager);
        let level = self.checked_level_value(value, target)?;
        Ok(Statement::Expr(scope.finish(operation(
            Log::SetManagerDisable,
            Type::None,
            vec![manager, level],
        ))))
    }

    pub(super) fn logger_attribute(
        &self,
        logger: Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        match attribute.attr.as_str() {
            "_cache" => Ok(operation(
                Log::EnabledCache,
                self.types.dict(Type::Int, Type::Bool),
                vec![logger],
            )),
            "handlers" => {
                let handler = self.handler_type().ok_or_else(|| {
                    error(self.source, attribute, "missing logging.Handler definition")
                })?;
                Ok(operation(
                    Log::Handlers,
                    self.types.list(handler),
                    vec![logger],
                ))
            }
            "disabled" => Ok(operation(Log::Disabled, Type::Bool, vec![logger])),
            "manager" => Ok(operation(Log::Manager, Type::LogManager, vec![logger])),
            "propagate" => Ok(operation(Log::Propagate, Type::Bool, vec![logger])),
            "root" => Ok(operation(Log::Root, Type::Logger, vec![logger])),
            "parent" => Ok(operation(
                Log::Parent,
                self.types.optional(Type::Logger),
                vec![logger],
            )),
            "name" => Ok(operation(Log::Name, Type::Str, vec![logger])),
            "level" => Ok(operation(Log::Level, Type::Int, vec![logger])),
            _ => Err(error(
                self.source,
                attribute,
                "supported logger attributes are name, level, parent and root",
            )),
        }
    }

    pub(super) fn logger_method(
        &mut self,
        logger: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if attribute.attr.as_str() == "makeRecord" {
            return self.logger_make_record(logger, call);
        }
        if matches!(attribute.attr.as_str(), "addHandler" | "removeHandler") {
            let handler = self.handler_type().ok_or_else(|| {
                error(self.source, attribute, "missing logging.Handler definition")
            })?;
            let args = self.arguments(call, &[required("hdlr", handler)], Some(logger))?;
            let op = if attribute.attr.as_str() == "addHandler" {
                Log::AddHandler
            } else {
                Log::RemoveHandler
            };
            return Ok(self.ordered_builtin(Builtin::Logging(op), Type::None, args));
        }
        if attribute.attr.as_str() == "hasHandlers" {
            let args = self.arguments(call, &[], Some(logger))?;
            return Ok(self.ordered_builtin(Builtin::Logging(Log::HasHandlers), Type::Bool, args));
        }
        if attribute.attr.as_str() == "getChildren" {
            let args = self.arguments(call, &[], Some(logger))?;
            return Ok(self.ordered_builtin(
                Builtin::Logging(Log::Children),
                self.types.set(Type::Logger),
                args,
            ));
        }
        if attribute.attr.as_str() == "setLevel" {
            let args = self.arguments_with(
                call,
                &[required("level", Type::Int)],
                Some(logger),
                |checker, _, arg| checker.checked_logging_level(arg),
            )?;
            return Ok(self.ordered_builtin(Builtin::Logging(Log::SetLevel), Type::None, args));
        }
        let args = call.arguments.args.as_ref();
        let name = attribute.attr.as_str();
        let (level, args) = match (name, args) {
            ("getEffectiveLevel", []) => {
                self.reject_builtin_keywords(call)?;
                return Ok(operation(Log::EffectiveLevel, Type::Int, vec![logger]));
            }
            ("isEnabledFor", [level]) => {
                self.reject_builtin_keywords(call)?;
                let level = self.expr_expected(level, Type::Int)?;
                return Ok(operation(Log::Enabled, Type::Bool, vec![logger, level]));
            }
            ("log", [level, args @ ..]) => (self.expr_expected(level, Type::Int)?, args),
            ("debug", args) => (number(10), args),
            ("info", args) => (number(20), args),
            ("warning", args) => (number(30), args),
            ("error", args) => (number(40), args),
            ("critical" | "fatal", args) => (number(50), args),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "unsupported logger method or argument count",
                ));
            }
        };
        self.log_message(logger, level, args, false, call)
    }

    pub(super) fn root_log(
        &mut self,
        level: Option<u8>,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let args = call.arguments.args.as_ref();
        let (level, args) = if let Some(level) = level {
            (number(level), args)
        } else if let Some((level, args)) = args.split_first() {
            (self.expr_expected(level, Type::Int)?, args)
        } else {
            return Err(error(self.source, call, "missing log level"));
        };
        self.log_message(self.root_logger(), level, args, true, call)
    }

    /// Bind an inherited adapter logging method at its original call site.
    pub(super) fn adapter_event(
        &mut self,
        class: usize,
        receiver: &Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        let method_level = match name {
            "log" => None,
            "debug" => Some(10),
            "info" => Some(20),
            "warning" => Some(30),
            "error" => Some(40),
            "critical" => Some(50),
            _ => return Ok(None),
        };
        let Some(base) = self
            .graph
            .logging_definition("LoggerAdapter")
            .and_then(|def| self.classes.declarations.ids.get(&def))
            .copied()
        else {
            return Ok(None);
        };
        if class != base
            && !self.classes.declarations.definitions[class]
                .bases
                .contains(&base)
        {
            return Ok(None);
        }
        if name != "log" && self.classes.methods[class].contains_key("log") {
            return Err(error(
                self.source,
                call,
                "forwarding adapter methods to an overridden log signature is unsupported",
            ));
        }
        let args = call.arguments.args.as_ref();
        let (level, args) = if let Some(level) = method_level {
            (number(level), args)
        } else if let Some((level, rest)) = args.split_first() {
            (self.expr_expected(level, Type::Int)?, rest)
        } else {
            return Err(error(self.source, call, "missing log level"));
        };
        let receiver = self.coerce(receiver.clone(), Type::Class(base), call)?;
        self.log_message(receiver, level, args, false, call)
            .map(Some)
    }

    fn log_message(
        &mut self,
        logger: Expr,
        level: Expr,
        args: &[ast::Expr],
        root: bool,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let Some((message, operands)) = args.split_first() else {
            return Err(error(self.source, call, "missing log message"));
        };
        let adapter = logger.ty != Type::Logger;
        let original = if adapter {
            self.expr_expected(message, Type::Str)?
        } else {
            self.expr(message)?
        };
        let mut values = vec![logger, level, original];
        for operand in operands {
            values.push(self.expr(operand)?);
        }
        let extra = match call.arguments.keywords.as_ref() {
            [] => None,
            [kw] if kw.arg.as_ref().is_some_and(|name| name.as_str() == "extra") => {
                if adapter {
                    let fields = self.types.dict(Type::Str, Type::Str);
                    let value = if matches!(kw.value, ast::Expr::Dict(_)) {
                        self.expr_expected(&kw.value, fields)?
                    } else {
                        self.expr(&kw.value)?
                    };
                    Some(self.coerce(value, self.types.optional(fields), &kw.value)?)
                } else {
                    Some(self.logging_extra(&kw.value)?)
                }
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "keyword arguments are unsupported for this builtin",
                ));
            }
        };
        let extra_index = extra.map(|field| {
            let i = values.len();
            values.push(field);
            i
        });
        // Python evaluates all arguments, even when a logger discards an event.
        let mut body = Vec::new();
        let mut clear = Vec::new();
        for value in values {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
        }
        // Module-level convenience functions install the default handler after
        // evaluating the message arguments, even when the root filters the event.
        if root {
            body.push(Statement::Expr(self.logging_source_call(
                "_auto_config",
                ir::CallArgs::positional(Vec::new()),
                call,
            )?));
        }
        let logger = self.local(clear[0]);
        let level = self.local(clear[1]);
        let original = self.check_record_value(self.local(clear[2]), message)?;
        let arguments = clear[3..3 + operands.len()]
            .iter()
            .zip(operands)
            .map(|(&slot, node)| self.check_record_value(self.local(slot), node))
            .collect::<Result<Vec<_>, _>>()?;
        let arguments = Expr {
            ty: self
                .types
                .tuple(arguments.iter().map(|arg| arg.ty).collect()),
            kind: ExprKind::Tuple(arguments),
        };
        let extra = extra_index.map(|i| self.local(clear[i]));
        let event = if adapter {
            let enabled = self.logging_source_call(
                "_adapter_enabled",
                ir::CallArgs::positional(vec![logger.clone(), level.clone()]),
                call,
            )?;
            let has_extra = Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(extra.is_some()),
            };
            let context = self.types.optional(self.types.dict(Type::Str, Type::Str));
            let processed = self.logging_source_call(
                "_adapter_process",
                ir::CallArgs::positional(vec![
                    logger,
                    original,
                    extra.unwrap_or_else(|| absent(context)),
                    has_extra,
                ]),
                call,
            )?;
            let mut prepared = calls::ArgumentScope::default();
            let held = prepared.hold(self, processed);
            let field = |index, ty| Expr {
                ty,
                kind: ExprKind::Field(Box::new(held.clone()), index),
            };
            let forwarded = self.log_event(
                field(0, Type::Logger),
                level,
                field(1, Type::Str),
                arguments,
                Some(field(2, context)),
                call,
            )?;
            when_enabled(enabled, prepared.finish(forwarded))
        } else {
            self.log_event(logger, level, original, arguments, extra, call)?
        };
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(event),
            },
        })
    }

    fn log_event(
        &mut self,
        logger: Expr,
        level: Expr,
        original: Expr,
        arguments: Expr,
        extra: Option<Expr>,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let enabled = operation(
            Log::Enabled,
            Type::Bool,
            vec![logger.clone(), level.clone()],
        );
        let path = text(
            self.graph.modules[self.module]
                .path
                .to_string_lossy()
                .as_ref(),
        );
        let line = self.source[..usize::from(call.start())]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1;
        let lineno = Expr {
            ty: Type::Int,
            kind: ExprKind::Int(line.to_string()),
        };
        let none = || Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let caller = caller_name(
            &self.graph.modules[self.module].parsed.syntax().body,
            call.range(),
        );
        let record = logging_records::op(
            ir::LogRecordOperation::New,
            Type::LogRecord,
            vec![
                operation(Log::Name, Type::Str, vec![logger.clone()]),
                level.clone(),
                path,
                lineno,
                original,
                arguments,
                none(),
                text(caller),
                none(),
            ],
        );
        let record = if let Some(extra) = extra {
            self.record_with_extra(record, extra)
        } else {
            record
        };
        let emit = self.logging_source_call(
            "_deliver",
            ir::CallArgs::positional(vec![logger, level, record]),
            call,
        )?;
        Ok(when_enabled(enabled, emit))
    }
}

/// Return the name of the Python frame that executes this call. Defaults and
/// decorators execute in the enclosing frame; only the definition body changes it.
fn caller_name(body: &[ast::Stmt], call: TextRange) -> &str {
    use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

    struct Caller<'a> {
        call: TextRange,
        name: &'a str,
    }
    impl<'a> Visitor<'a> for Caller<'a> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if !stmt.range().contains_range(self.call) {
                return;
            }
            let scope = match stmt {
                ast::Stmt::FunctionDef(f) => Some((f.name.as_str(), &*f.body)),
                ast::Stmt::ClassDef(c) => Some((c.name.as_str(), &*c.body)),
                _ => None,
            };
            if let Some((name, body)) = scope
                && body.iter().any(|s| s.range().contains_range(self.call))
            {
                self.name = name;
                self.visit_body(body);
            } else {
                walk_stmt(self, stmt);
            }
        }

        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if !expr.range().contains_range(self.call) {
                return;
            }
            match expr {
                ast::Expr::Lambda(f) if f.body.range().contains_range(self.call) => {
                    self.name = "<lambda>"
                }
                ast::Expr::Generator(g)
                    if !g.generators[0].iter.range().contains_range(self.call) =>
                {
                    self.name = "<genexpr>"
                }
                _ => {}
            }
            walk_expr(self, expr);
        }
    }
    let mut caller = Caller {
        call,
        name: "<module>",
    };
    caller.visit_body(body);
    caller.name
}

fn when_enabled(enabled: Expr, event: Expr) -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::Conditional(
            Box::new(enabled),
            Box::new(event),
            Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }),
        ),
    }
}
