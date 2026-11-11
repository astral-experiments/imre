//! Bind logging formatters while retaining the caller's actual defaults dictionary.
use super::calls::parameter;
use super::*;
use ir::FormatterOperation as Op;

fn op(kind: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Formatter(kind), args),
    }
}
impl Checker<'_> {
    pub(super) fn log_formatter(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let mut defaults = parameter("defaults", Type::None, Some(Expr::none()));
        defaults.kind = ParameterKind::KeywordOnly;
        let params = [
            parameter("fmt", string, Some(Expr::absent(string))),
            parameter("datefmt", string, Some(Expr::absent(string))),
            parameter("style", Type::Str, Some(unpacking::string("%"))),
            parameter("validate", Type::Bool, Some(Expr::boolean(true))),
            defaults,
        ];
        let args = self.arguments_with(call, &params, None, |this, i, arg| {
            if i != 4 {
                return this.expr_expected(arg, params[i].ty);
            }
            if let ast::Expr::Dict(dict) = arg
                && dict.items.is_empty()
            {
                return this.dictionary(dict, Some((Type::Str, Type::None)), None);
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
        let rec = parameter("record", Type::LogRecord, None);
        let (kind, ty, params) = match attr.attr.as_str() {
            "usesTime" => (Op::UsesTime, Type::Bool, vec![]),
            "format" => (Op::Format, Type::Str, vec![rec]),
            "formatMessage" => (Op::Message, Type::Str, vec![rec]),
            "formatTime" => {
                let s = self.types.optional(Type::Str);
                (
                    Op::Time,
                    Type::Str,
                    vec![rec, parameter("datefmt", s, Some(Expr::absent(s)))],
                )
            }
            "formatStack" => {
                let params = [parameter("stack_info", Type::Str, None)];
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
