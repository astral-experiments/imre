//! Bind logging formatters while retaining the caller's actual defaults dictionary.
use super::*;
use ir::FormatterOperation as Op;

fn op(kind: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Formatter(kind), args),
    }
}
fn param(name: &str, ty: Type, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        default,
        kind: ParameterKind::PositionalOrKeyword,
        sign_only: false,
    }
}
fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}
fn absent(ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Absent(Box::new(none())),
    }
}

impl Checker<'_> {
    pub(super) fn log_formatter(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let mut defaults = param("defaults", Type::None, Some(none()));
        defaults.kind = ParameterKind::KeywordOnly;
        let params = [
            param("fmt", string, Some(absent(string))),
            param("datefmt", string, Some(absent(string))),
            param("style", Type::Str, Some(unpacking::string("%"))),
            param(
                "validate",
                Type::Bool,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                }),
            ),
            defaults,
        ];
        let args = self.arguments_with(call, &params, None, |this, i, arg| {
            if i != 4 {
                return this.expr_expected(arg, params[i].ty);
            }
            if let ast::Expr::Dict(dict) = arg
                && dict.items.is_empty()
            {
                return this.dictionary(dict, Some((Type::Str, Type::None)));
            }
            let value = this.expr(arg)?;
            if value.ty == Type::None
                || matches!(this.types.mapping(value.ty), Some((Type::Str, _)))
            {
                this.check_record_value(value, arg)
            } else {
                Err(error(
                    this.source,
                    arg,
                    "formatter defaults need a string-key mapping",
                ))
            }
        })?;
        Ok(self.ordered_builtin(Builtin::Formatter(Op::New), Type::LogFormatter, args))
    }

    pub(super) fn formatter_attr(
        &self,
        f: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let (kind, ty) = match attr.attr.as_str() {
            "_fmt" => (Op::Text, Type::Str),
            "datefmt" => (Op::Date, string),
            "default_time_format" => (Op::DefaultTime, Type::Str),
            "default_msec_format" => (Op::DefaultMsec, string),
            _ => return Err(error(self.source, attr, "unsupported Formatter attribute")),
        };
        Ok(op(kind, ty, vec![f]))
    }

    pub(super) fn formatter_set(
        &self,
        f: Expr,
        attr: &ast::ExprAttribute,
        value: Expr,
    ) -> Result<Statement, String> {
        let (kind, ty) = match attr.attr.as_str() {
            "converter" => (Op::SetUtc, Type::Bool),
            "datefmt" => (Op::SetDate, self.types.optional(Type::Str)),
            "default_time_format" => (Op::SetDefaultTime, Type::Str),
            "default_msec_format" => (Op::SetDefaultMsec, self.types.optional(Type::Str)),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "unsupported Formatter attribute assignment",
                ));
            }
        };
        Ok(Statement::Expr(op(
            kind,
            Type::None,
            vec![f, self.coerce(value, ty, attr)?],
        )))
    }

    pub(super) fn formatter_method(
        &mut self,
        f: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let rec = param("record", Type::LogRecord, None);
        let (kind, ty, params) = match attr.attr.as_str() {
            "usesTime" => (Op::UsesTime, Type::Bool, vec![]),
            "format" => (Op::Format, Type::Str, vec![rec]),
            "formatMessage" => (Op::Message, Type::Str, vec![rec]),
            "formatTime" => {
                let s = self.types.optional(Type::Str);
                (
                    Op::Time,
                    Type::Str,
                    vec![rec, param("datefmt", s, Some(absent(s)))],
                )
            }
            "formatStack" => {
                let params = [param("stack_info", Type::Str, None)];
                let args = self.arguments(call, &params, Some(f))?;
                // This operation still evaluates and keeps its receiver before the argument.
                return Ok(self.ordered_builtin(Builtin::Formatter(Op::Stack), Type::Str, args));
            }
            _ => return Err(error(self.source, attr, "unsupported Formatter method")),
        };
        let args = self.arguments(call, &params, Some(f))?;
        Ok(self.ordered_builtin(Builtin::Formatter(kind), ty, args))
    }
}
