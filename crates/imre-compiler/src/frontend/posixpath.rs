//! Static POSIX calls expose a native Path through the same boundary as os.fspath.
use super::*;
use crate::ir::PosixPathOperation;

impl Checker<'_> {
    pub(super) fn posix_path(
        &mut self,
        call: &ast::ExprCall,
        operation: PosixPathOperation,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let join = operation == PosixPathOperation::Join;
        let param = match operation {
            PosixPathOperation::Join => "a",
            PosixPathOperation::Normpath | PosixPathOperation::Abspath => "path",
            _ => "p",
        };
        let (first, rest) = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([first, rest @ ..], []) if join || rest.is_empty() => (first, rest),
            ([], [keyword])
                if keyword
                    .arg
                    .as_ref()
                    .is_some_and(|name| name.as_str() == param) =>
            {
                (&keyword.value, &[][..])
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    if join {
                        "os.path.join requires a path and fixed positional components"
                    } else {
                        "the os.path function requires one path"
                    },
                ));
            }
        };
        let mut operands = Vec::with_capacity(1 + rest.len());
        for expression in std::iter::once(first).chain(rest) {
            operands.push((expression, self.expr(expression)?));
        }
        let bytes = operands[0].1.ty == Type::Bytes;
        let mixed = join
            && operands
                .iter()
                .any(|(_, value)| (value.ty == Type::Bytes) != bytes);
        // CPython's mixed-type diagnostic inspects the unconverted tail even
        // when the component that first failed appears earlier in the call.
        let path_in_tail = operands
            .iter()
            .skip(1)
            .any(|(_, value)| value.ty == Type::Path);
        let mut args = Vec::with_capacity(operands.len());
        for (index, (expression, value)) in operands.into_iter().enumerate() {
            let value = if mixed
                && index > 0
                && self
                    .types
                    .members(value.ty)
                    .is_some_and(|members| members == [Type::Str, Type::Path])
            {
                value
            } else if join && matches!(value.ty, Type::Union(_) | Type::JsonRoot | Type::JsonValue)
            {
                self.path_text(value, expression)?
            } else {
                self.fspath_value(value, expression)?
            };
            args.push(value);
        }
        if join && args.len() == 1 {
            return Ok(args.pop().unwrap());
        }
        let ty = args[0].ty;
        let operation = if mixed {
            PosixPathOperation::MixedJoin { path_in_tail }
        } else {
            operation
        };
        let ty = if operation == PosixPathOperation::Splitext {
            self.types.tuple(vec![ty, ty])
        } else {
            ty
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::PosixPath(operation), args),
        })
    }
}
