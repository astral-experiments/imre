//! Lower exact socket receivers and IPv4 address tuples without exposing raw fds.
use super::*;
use crate::ir::SocketOp;

pub(super) fn constant(name: &'static str) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Builtin(Builtin::Socket(SocketOp::Constant(name)), vec![]),
    }
}

impl Checker<'_> {
    pub(super) fn is_tcpserver(&self, class: usize) -> bool {
        std::iter::once(&class)
            .chain(&self.classes.declarations.definitions[class].bases)
            .any(|&id| {
                let record = &self.classes.declarations.definitions[id];
                let module = &self.graph.modules[record.definition.module];
                module.bundled && module.name == "socketserver" && record.name == "TCPServer"
            })
    }

    /// TCPServer's non-suppressing context delegates to the concrete server_close.
    pub(super) fn tcpserver_context_exit(
        &mut self,
        class: usize,
        receiver: Expr,
        site: &ast::Expr,
    ) -> Result<Expr, String> {
        self.dispatch_record(class, receiver, site, |checker, actual, receiver| {
            let close = &checker.classes.methods[actual]["server_close"];
            if close.params.len() != 1 || close.returns != Type::None {
                return Err(error(
                    checker.source,
                    site,
                    "TCPServer context requires a zero-argument server_close",
                ));
            }
            checker.calls.insert(close.id);
            Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Call(close.id, ir::CallArgs::positional(vec![receiver])),
            })
        })
    }

    pub(super) fn socket_constructor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [
            ("family", constant("af_inet")),
            ("type", constant("sock_stream")),
            ("proto", pipelines::integer(0)),
        ]
        .map(|(name, default)| Parameter {
            name: name.into(),
            ty: Type::Int,
            kind: ParameterKind::PositionalOrKeyword,
            default: Some(default),
            sign_only: false,
        });
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::Socket(SocketOp::New), Type::Socket, args))
    }

    pub(super) fn socket_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        use SocketOp::*;
        let address = self.types.tuple(vec![Type::Str, Type::Int]);
        let int = Type::Int;
        let zero = || Some(pipelines::integer(0));
        let (op, ty, params): (_, _, Vec<(&str, Type, Option<Expr>)>) = match attr.attr.as_str() {
            "bind" => (Bind, Type::None, vec![("address", address, None)]),
            "connect" => (Connect, Type::None, vec![("address", address, None)]),
            "shutdown" => (Shutdown, Type::None, vec![("how", int, None)]),
            "listen" => (
                Listen,
                Type::None,
                vec![("backlog", int, Some(constant("default_backlog")))],
            ),
            "getsockname" => (SockName, address, vec![]),
            "accept" => (
                Accept,
                self.types.tuple(vec![Type::Socket, address]),
                vec![],
            ),
            "recv" => (
                Recv,
                Type::Bytes,
                vec![("bufsize", int, None), ("flags", int, zero())],
            ),
            name @ ("send" | "sendall") => (
                if name == "send" { Send } else { SendAll },
                if name == "send" {
                    Type::Int
                } else {
                    Type::None
                },
                vec![("data", Type::Bytes, None), ("flags", int, zero())],
            ),
            "close" => (Close, Type::None, vec![]),
            "fileno" => (Fileno, int, vec![]),
            "setblocking" => (SetBlocking, Type::None, vec![("flag", int, None)]),
            "getblocking" => (GetBlocking, Type::Bool, vec![]),
            "dup" => (Dup, Type::Socket, vec![]),
            "get_inheritable" => (GetInheritable, Type::Bool, vec![]),
            "set_inheritable" => (SetInheritable, Type::None, vec![("inheritable", int, None)]),
            "getsockopt" => (
                GetSockOpt,
                int,
                vec![("level", int, None), ("optname", int, None)],
            ),
            "setsockopt" => (
                SetSockOpt,
                Type::None,
                vec![
                    ("level", int, None),
                    ("optname", int, None),
                    ("value", int, None),
                ],
            ),
            _ => return Err(error(self.source, call, "unsupported socket method")),
        };
        let params: Vec<_> = params
            .into_iter()
            .map(|(name, ty, default)| Parameter {
                name: name.into(),
                ty,
                kind: if op == SetInheritable {
                    ParameterKind::PositionalOrKeyword
                } else {
                    ParameterKind::PositionalOnly
                },
                default,
                sign_only: false,
            })
            .collect();
        let args = self.arguments_with(call, &params, Some(receiver), |checker, index, arg| {
            if params[index].ty == Type::Int {
                let value = checker.expr(arg)?;
                if op == SetSockOpt && index == 2 && value.ty == Type::Bytes {
                    return Ok(value);
                }
                if value.ty == Type::Bool {
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                    });
                }
                checker.coerce(value, Type::Int, arg)
            } else {
                checker.expr_expected(arg, params[index].ty)
            }
        })?;
        Ok(self.ordered_builtin(Builtin::Socket(op), ty, args))
    }
}
