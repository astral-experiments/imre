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
    /// Resolve a bundled definition even when a caller only imported getLogger.
    fn logging_definition(&self, name: &str) -> Option<DefinitionId> {
        let module = self
            .graph
            .modules
            .iter()
            .find(|module| module.is_bundled("__imre_logging_classes__"))?;
        match module.bindings.get(name)? {
            Binding::Definition(def) => Some(*def),
            _ => None,
        }
    }

    fn handler_type(&self) -> Option<Type> {
        let id = self.logging_definition("Handler")?;
        Some(Type::Class(*self.classes.declarations.ids.get(&id)?))
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
                    required("msg", Type::Str),
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
        Ok(self.call_value(sig, Type::None, args))
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
        let kw = ParameterKind::KeywordOnly;
        let boolean = |n| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(n),
        };
        let params = [
            parameter("level", level, kw, absent(level)),
            parameter("format", string, kw, absent(string)),
            parameter("datefmt", string, kw, absent(string)),
            parameter("force", Type::Bool, kw, boolean(false)),
            parameter("stream", output, kw, absent(output)),
            parameter("style", Type::Str, kw, text("%")),
            parameter("handlers", handlers, kw, absent(handlers)),
        ];
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
        for flag in [!supplied("format"), supplied("stream")] {
            args.order.push(args.values.len());
            args.values.push(boolean(flag));
        }
        self.logging_source_call("_basic_config", args, call)
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
        self.reject_builtin_keywords(call)?;
        let args = call.arguments.args.as_ref();
        let name = attribute.attr.as_str();
        let (level, args) = match (name, args) {
            ("getEffectiveLevel", []) => {
                return Ok(operation(Log::EffectiveLevel, Type::Int, vec![logger]));
            }
            ("isEnabledFor", [level]) => {
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
        self.reject_builtin_keywords(call)?;
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
        let mut values = vec![logger, level, self.expr(message)?];
        for operand in operands {
            values.push(self.expr(operand)?);
        }
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
        let message = if operands.is_empty() {
            self.log_scalar(self.local(clear[2]), 's', message)?
        } else {
            let ast::Expr::StringLiteral(format) = message else {
                return Err(error(
                    self.source,
                    message,
                    "log messages with arguments require a literal format",
                ));
            };
            self.log_format(format.value.to_str(), &clear[3..], message)?
        };
        let enabled = operation(
            Log::Enabled,
            Type::Bool,
            vec![logger.clone(), level.clone()],
        );
        let def = self
            .logging_definition("_deliver")
            .ok_or_else(|| error(self.source, call, "missing native logging delivery"))?;
        let sig = self.functions.get(&def).ok_or_else(|| {
            error(
                self.source,
                call,
                "missing native logging delivery signature",
            )
        })?;
        self.calls.insert(sig.id);
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
        let emit = self.call_value(
            sig,
            Type::None,
            ir::CallArgs::positional(vec![logger, level, message, path, lineno]),
        );
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::Conditional(
                        Box::new(enabled),
                        Box::new(emit),
                        Box::new(Expr {
                            ty: Type::None,
                            kind: ExprKind::None,
                        }),
                    ),
                }),
            },
        })
    }

    fn log_scalar(&self, value: Expr, kind: char, node: &ast::Expr) -> Result<Expr, String> {
        if kind == 's' && matches!(value.ty, Type::Str | Type::StrLiteral(_) | Type::StrEnum(_)) {
            return Ok(Self::string_value(value));
        }
        if kind == 's' && value.ty == Type::Path {
            return Ok(paths::operation(
                ir::PathOperation::Text,
                Type::Str,
                vec![value],
            ));
        }
        if kind == 'd' && value.ty == Type::Bool {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Conditional(
                    Box::new(value),
                    Box::new(text("1")),
                    Box::new(text("0")),
                ),
            });
        }
        if (kind == 'd' && value.ty != Type::Int)
            || !matches!(
                value.ty,
                Type::Str
                    | Type::StrLiteral(_)
                    | Type::Bytes
                    | Type::Int
                    | Type::Float
                    | Type::Bool
                    | Type::None
            )
        {
            return Err(error(
                self.source,
                node,
                "log format argument requires a supported scalar type",
            ));
        }
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Repr, vec![Self::string_value(value)]),
        })
    }

    fn log_format(
        &self,
        mut format: &str,
        slots: &[usize],
        node: &ast::Expr,
    ) -> Result<Expr, String> {
        let mut parts = Vec::new();
        let mut slots = slots.iter();
        while let Some((literal, tail)) = format.split_once('%') {
            if !literal.is_empty() {
                parts.push(text(literal));
            }
            let Some(kind) = tail.chars().next() else {
                return Err(error(self.source, node, "incomplete log format"));
            };
            match kind {
                '%' => parts.push(text("%")),
                's' | 'r' | 'd' => {
                    let slot = slots
                        .next()
                        .ok_or_else(|| error(self.source, node, "missing log format argument"))?;
                    parts.push(self.log_scalar(self.local(*slot), kind, node)?);
                }
                _ => {
                    return Err(error(
                        self.source,
                        node,
                        "supported log conversions are %s, %r, %d and %%",
                    ));
                }
            }
            format = &tail[kind.len_utf8()..];
        }
        if slots.next().is_some() {
            return Err(error(self.source, node, "unused log format argument"));
        }
        if !format.is_empty() {
            parts.push(text(format));
        }
        Ok(parts
            .into_iter()
            .reduce(|a, b| Expr {
                ty: Type::Str,
                kind: ExprKind::Binary(Binary::Concat, Box::new(a), Box::new(b)),
            })
            .unwrap_or_else(|| text("")))
    }
}
