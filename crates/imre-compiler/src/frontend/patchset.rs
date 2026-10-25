//! The sealed unidiff surface provides immutable objects and ordered iteration.

use super::*;
use ir::{PatchKind, PatchOperation};

fn operation(op: PatchOperation, returns: Type, input: Expr) -> Expr {
    Expr {
        ty: returns,
        kind: ExprKind::Builtin(Builtin::Patch(op), vec![input]),
    }
}

impl Checker<'_> {
    pub(super) fn parse_patch(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let input = match (&*call.arguments.args, &*call.arguments.keywords) {
            ([input], []) => input,
            ([], [keyword]) if keyword.arg.as_ref().is_some_and(|key| key.as_str() == "f") => {
                &keyword.value
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "PatchSet requires one unified-diff string",
                ));
            }
        };
        Ok(operation(
            PatchOperation::Parse,
            Type::Patch(PatchKind::Set),
            self.expr_expected(input, Type::Str)?,
        ))
    }

    pub(super) fn patch_attribute(
        &self,
        kind: PatchKind,
        value: Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (op, returns) = match (kind, attribute.attr.as_str()) {
            (PatchKind::Set, "modified_files") => (
                PatchOperation::ModifiedFiles,
                self.types.list(Type::Patch(PatchKind::File)),
            ),
            (PatchKind::File, "path") => (PatchOperation::Path, Type::Str),
            (PatchKind::Hunk, "source_start") => (PatchOperation::SourceStart, Type::Int),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "unsupported unified-diff attribute",
                ));
            }
        };
        Ok(operation(op, returns, value))
    }

    pub(super) fn patch_iteration(&self, value: Expr) -> Expr {
        let (op, item) = match value.ty {
            Type::Patch(PatchKind::Set) => (PatchOperation::Files, PatchKind::File),
            Type::Patch(PatchKind::File) => (PatchOperation::Hunks, PatchKind::Hunk),
            _ => return value,
        };
        operation(op, self.types.list(Type::Patch(item)), value)
    }

    pub(super) fn patch_builtin(&self, name: &str, value: &Expr) -> Option<Expr> {
        match (name, value.ty) {
            ("str", Type::Patch(PatchKind::Hunk)) => Some(operation(
                PatchOperation::HunkText,
                Type::Str,
                value.clone(),
            )),
            ("len", Type::Patch(_)) => {
                Some(operation(PatchOperation::Len, Type::Int, value.clone()))
            }
            ("len", _)
                if let ExprKind::Builtin(Builtin::Patch(PatchOperation::ModifiedFiles), args) =
                    &value.kind =>
            {
                Some(operation(
                    PatchOperation::ModifiedCount,
                    Type::Int,
                    args[0].clone(),
                ))
            }
            _ => None,
        }
    }
}
