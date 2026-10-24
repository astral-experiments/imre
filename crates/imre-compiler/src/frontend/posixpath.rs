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
        let param = if join { "a" } else { "p" };
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
        let first = self.fspath_argument(first)?;
        let ty = first.ty;
        if join && rest.is_empty() {
            return Ok(first);
        }
        let mut args = vec![first];
        let mut path_in_tail = false;
        for child in rest {
            let raw = self.expr(child)?;
            path_in_tail |= raw.ty == Type::Path;
            args.push(self.fspath_value(raw, child)?);
        }
        // CPython's mixed-type diagnostic inspects the unconverted tail even
        // when the component that first failed appears earlier in the call.
        let operation = if join && args.iter().any(|arg| arg.ty != ty) {
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
