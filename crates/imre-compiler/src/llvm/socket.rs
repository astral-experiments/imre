//! Address and accept results use the caller's ordinary fixed-tuple layouts.
use super::*;
use crate::ir::SocketOp;

impl Emitter<'_, '_> {
    pub(super) fn socket_operation(
        &mut self,
        op: SocketOp,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        use SocketOp::*;
        if let Constant(name) = op {
            return Value {
                ty: Type::Int,
                operand: self.instruction(format!("load i64, ptr @imre_socket_{name}, align 8")),
                small_integer: true,
            };
        }
        if matches!(op, SockName | Accept) {
            let layout = self
                .output
                .global_ref(format!("layout_{}", record_name(returns)));
            let socket = &args[0].value.operand;
            let call = if op == Accept {
                let address = self.module.types.fields(returns).unwrap()[1];
                let address_layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(address)));
                format!(
                    "call ptr @imre_socket_accept(ptr {socket}, ptr @{layout}, ptr @{address_layout})"
                )
            } else {
                format!("call ptr @imre_socket_sockname(ptr {socket}, ptr @{layout})")
            };
            return Value {
                ty: returns,
                operand: self.checked_instruction(call),
                small_integer: false,
            };
        }
        let name = match op {
            New => "imre_socket_new",
            Bind => "imre_socket_bind",
            Listen => "imre_socket_listen",
            Connect => "imre_socket_connect",
            Shutdown => "imre_socket_shutdown",
            Recv => "imre_socket_recv",
            Send => "imre_socket_send",
            SendAll => "imre_socket_sendall",
            Close => "imre_socket_close",
            Fileno => "imre_socket_fileno",
            SetBlocking => "imre_socket_setblocking",
            GetBlocking => "imre_socket_getblocking",
            Dup => "imre_socket_dup",
            GetInheritable => "imre_socket_get_inheritable",
            SetInheritable => "imre_socket_set_inheritable",
            SetSockOpt if args[3].value.ty == Type::Bytes => "imre_socket_setsockopt_bytes",
            SetSockOpt => "imre_socket_setsockopt",
            GetSockOpt => "imre_socket_getsockopt",
            Constant(_) | Accept | SockName => unreachable!(),
        };
        let args: Vec<_> = args.iter().map(|a| &a.value).collect();
        if returns == Type::None {
            self.void_call(name, &args);
            none()
        } else {
            self.call(name, returns, &args)
        }
    }
}
