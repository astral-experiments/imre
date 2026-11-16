use super::*;

impl Checker<'_> {
    pub(super) fn copy_file_range(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        use FdOperation::*;
        let optional = self.types.optional(Type::Int);
        let params = [
            calls::parameter("src", Type::Int, None),
            calls::parameter("dst", Type::Int, None),
            calls::parameter("count", Type::Int, None),
            calls::parameter("offset_src", optional, Some(Expr::absent(optional))),
            calls::parameter("offset_dst", optional, Some(Expr::absent(optional))),
        ];
        let mut scope = calls::ArgumentScope::default();
        // Evaluate and retain all supplied expressions in source order, before converting
        // anything. Argument Clinic then converts from src to offset_dst in signature order.
        let args = self.bind_supplied(
            call,
            &params,
            None,
            Some(&mut scope),
            &mut |checker, _, input| input.value(checker),
        )?;
        let arg = |i: usize| args.values[args.order[i]].clone();
        let fd_op = |op, ty, args| Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::process(ProcessOperation::Fd(op)), args),
        };
        let mut descriptor = |value| {
            let value = self.index_word(value, &call.func, false)?;
            Ok::<_, Failure>(scope.hold(self, fd_op(TransferFd, Type::Int, vec![value])))
        };
        let src = descriptor(arg(0))?;
        let dst = descriptor(arg(1))?;
        let count = self.index_word(arg(2), &call.func, false)?;
        let count = scope.hold(self, count);
        scope.hold(self, fd_op(CheckCopyCount, Type::None, vec![count.clone()]));
        let source = self.copy_range_offset(arg(3), &call.func)?;
        let source = scope.hold(self, source);
        scope.hold(
            self,
            fd_op(CheckCopyOffset, Type::None, vec![source.clone()]),
        );
        let target = self.copy_range_offset(arg(4), &call.func)?;
        Ok(scope.finish(fd_op(
            CopyFileRange,
            Type::Int,
            vec![src, dst, count, source, target],
        )))
    }

    fn copy_range_offset(&mut self, value: Expr, at: &ast::Expr) -> Result<Expr, String> {
        let optional = self.types.optional(Type::Int);
        if value.ty == Type::None || value.ty == optional {
            return self.coerce(value, optional, at);
        }
        if self.types.payload(value.ty).is_some() {
            let (slot, test, absent, present) = self.protocol_alternatives(value.ty).unwrap();
            let absent = self.coerce(absent, optional, at)?;
            let index = self.index_word(present, at, false)?;
            let present = self.coerce(index, optional, at)?;
            return Ok(Expr {
                ty: optional,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: optional,
                        kind: ExprKind::Conditional(
                            Box::new(test),
                            Box::new(absent),
                            Box::new(present),
                        ),
                    }),
                    clear: vec![slot],
                },
            });
        }
        let index = self.index_word(value, at, false)?;
        self.coerce(index, optional, at)
    }
}
