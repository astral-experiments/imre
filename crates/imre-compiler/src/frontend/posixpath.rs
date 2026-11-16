//! Static POSIX calls expose a native Path through the same boundary as os.fspath.
use super::*;
use crate::ir::PosixPathOperation;
use std::result::Result;

impl Checker<'_> {
    /// Consume a concrete path sequence once. Display elements use the closed
    /// str/bytes union when needed, so mixed types reach Python's runtime error.
    fn posix_commonpath(&mut self, expression: &ast::Expr) -> phase::Result<Expr, String> {
        let expression = if let ast::Expr::Call(call) = expression
            && self.is_builtin_iter(call)
            && call.arguments.keywords.is_empty()
            && let [sequence] = call.arguments.args.as_ref()
        {
            sequence
        } else {
            expression
        };
        let elements = match expression {
            ast::Expr::List(s) => Some(s.elts.as_slice()),
            ast::Expr::Tuple(s) => Some(s.elts.as_slice()),
            _ => None,
        };
        let value = if let Some(elements) = elements {
            let values = elements
                .iter()
                .map(|item| self.fspath_argument(item))
                .collect::<Result<Vec<_>, _>>()?;
            let ty = if values.is_empty() || values.iter().all(|v| v.ty == Type::Str) {
                Type::Str
            } else if values.iter().all(|v| v.ty == Type::Bytes) {
                Type::Bytes
            } else {
                self.types.union(&[Type::Str, Type::Bytes]).unwrap()
            };
            let values = values
                .into_iter()
                .zip(elements)
                .map(|(v, e)| self.coerce(v, ty, e))
                .collect::<Result<Vec<_>, _>>()?;
            Expr {
                ty: self.types.list(ty),
                kind: ExprKind::List(values),
            }
        } else {
            self.expr(expression)?
        };
        let ty = if value.ty == Type::None {
            Type::Str
        } else {
            match self.types.element(value.ty) {
                Some(Type::Path) => Type::Str,
                Some(ty @ (Type::Str | Type::Bytes)) => ty,
                Some(ty)
                    if self
                        .types
                        .members(ty)
                        .is_some_and(|parts| parts == [Type::Str, Type::Bytes]) =>
                {
                    ty
                }
                _ => {
                    return Err(error(
                        self.source,
                        expression,
                        "commonpath requires a list of str, bytes or pathlib.Path",
                    ));
                }
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(
                Builtin::PosixPath(PosixPathOperation::Commonpath),
                vec![value],
            ),
        })
    }

    pub(super) fn posix_path(
        &mut self,
        call: &ast::ExprCall,
        operation: PosixPathOperation,
    ) -> phase::Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let join = operation == PosixPathOperation::Join;
        let param = match operation {
            PosixPathOperation::Commonpath => "paths",
            PosixPathOperation::Join => "a",
            PosixPathOperation::Normpath | PosixPathOperation::Abspath => "path",
            _ => "p",
        };
        let written = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            (args @ [_, ..], []) if join || args.len() == 1 => args,
            ([], [keyword])
                if keyword
                    .arg
                    .as_ref()
                    .is_some_and(|name| name.as_str() == param) =>
            {
                std::slice::from_ref(&keyword.value)
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
        if operation == PosixPathOperation::Commonpath {
            return self.posix_commonpath(&written[0]);
        }
        let operands = registered::values::call_arguments(call, |arg| self.expr(arg))?;
        if (join || operation == PosixPathOperation::Abspath)
            && operands.iter().any(|value| value.ty == Type::PythonObject)
        {
            return self.call_import_with_operands(call, operands);
        }
        let bytes = operands[0].ty == Type::Bytes;
        let mixed = join
            && operands
                .iter()
                .any(|value| (value.ty == Type::Bytes) != bytes);
        // CPython's mixed-type diagnostic inspects the unconverted tail even
        // when the component that first failed appears earlier in the call.
        let path_in_tail = operands[1..].iter().any(|value| value.ty == Type::Path);
        let mut args = Vec::with_capacity(operands.len());
        for (index, (value, expression)) in operands.into_iter().zip(written).enumerate() {
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
