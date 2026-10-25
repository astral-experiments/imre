//! Bind text stream operations without losing the cursor when a StringIO is used as TextIO.

use super::*;
use ir::StringIoOperation as Op;

/// The C constructor accepts keywords; its cursor and read methods take positional arguments only.
fn parameter(name: &str, ty: Type, kind: ParameterKind, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        kind,
        default,
        sign_only: false,
    }
}

impl Checker<'_> {
    /// Distinguish the omitted newline (LF) from a supplied None (universal newlines).
    pub(super) fn string_io(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let none = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            string,
            call,
        )?;
        let lf = self.coerce(
            Expr {
                ty: Type::Str,
                kind: ExprKind::Str("\n".into()),
            },
            string,
            call,
        )?;
        let params = [
            parameter(
                "initial_value",
                string,
                ParameterKind::PositionalOrKeyword,
                Some(none),
            ),
            parameter(
                "newline",
                string,
                ParameterKind::PositionalOrKeyword,
                Some(lf),
            ),
        ];
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::StringIo(Op::New), Type::StringIO, args))
    }

    /// Keep the receiver rooted while evaluating explicit arguments in source order.
    pub(super) fn string_io_method(
        &mut self,
        file: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let integer = self.types.optional(Type::Int);
        let none = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            integer,
            call,
        )?;
        let pos = ParameterKind::PositionalOnly;
        let size = || parameter("size", integer, pos, Some(none.clone()));
        let (op, returns, params) = match name {
            "getvalue" => (Op::Value, Type::Str, vec![]),
            "read" | "readline" => (Op::Read, Type::Str, vec![size()]),
            "seek" => (
                Op::Seek,
                Type::Int,
                vec![
                    parameter("pos", Type::Int, pos, None),
                    parameter("whence", Type::Int, pos, Some(logging::number(0))),
                ],
            ),
            "tell" => (Op::Position, Type::Int, vec![]),
            "truncate" => (Op::Position, Type::Int, vec![size()]),
            _ => unreachable!("only StringIO-specific method names reach this binder"),
        };
        let mut args = self.arguments(call, &params, Some(file))?;
        let mut append = |value| {
            args.order.push(args.values.len());
            args.values.push(value);
        };
        if name == "tell" {
            append(none);
        }
        if matches!(op, Op::Read | Op::Position) {
            append(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(matches!(name, "readline" | "truncate")),
            });
        }
        Ok(self.ordered_builtin(Builtin::StringIo(op), returns, args))
    }
}
