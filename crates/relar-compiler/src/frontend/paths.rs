//! Sealed pathlib operations retain concrete path storage and explicit UTF-8 I/O.

use super::*;
use crate::ir::PathOperation;

pub(super) fn operation(kind: PathOperation, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Path(kind), args),
    }
}

impl Checker<'_> {
    /// Path-like values are exactly strings or native paths; no user protocol runs.
    pub(super) fn path_argument(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        self.path_text(value, expression)
    }

    fn path_text(&self, value: Expr, node: &impl Ranged) -> Result<Expr, String> {
        match value.ty {
            Type::Str => Ok(value),
            Type::Path => Ok(operation(PathOperation::Text, Type::Str, vec![value])),
            _ => Err(error(
                self.source,
                node,
                "a filesystem path requires str or pathlib.Path",
            )),
        }
    }

    pub(super) fn path_constructor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let mut args = call.arguments.args.iter();
        let first = if let Some(argument) = args.next() {
            self.path_argument(argument)?
        } else {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Str(String::new()),
            }
        };
        let mut result = operation(PathOperation::New, Type::Path, vec![first]);
        for argument in args {
            let next = self.path_argument(argument)?;
            result = operation(PathOperation::Join, Type::Path, vec![result, next]);
        }
        Ok(result)
    }

    /// Both operands are evaluated in source order, including reflected str / Path.
    pub(super) fn path_join(
        &self,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let left = if left.ty == Type::Path {
            left
        } else {
            operation(
                PathOperation::New,
                Type::Path,
                vec![self.path_text(left, node)?],
            )
        };
        let right = self.path_text(right, node)?;
        Ok(operation(
            PathOperation::Join,
            Type::Path,
            vec![left, right],
        ))
    }

    pub(super) fn path_attribute(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (kind, ty) = match attribute.attr.as_str() {
            "name" => (PathOperation::Name, Type::Str),
            "stem" => (PathOperation::Stem, Type::Str),
            "suffix" => (PathOperation::Suffix, Type::Str),
            "parent" => (PathOperation::Parent, Type::Path),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "supported Path attributes are name, stem, suffix, and parent",
                ));
            }
        };
        Ok(operation(kind, ty, vec![receiver]))
    }

    /// Literal encoding makes the text boundary independent of process locale.
    fn path_encoding(&self, call: &ast::ExprCall) -> Result<(), String> {
        let [keyword] = call.arguments.keywords.as_ref() else {
            return Err(error(
                self.source,
                call,
                "Path text I/O requires encoding='utf-8'",
            ));
        };
        if keyword.arg.as_ref().map(|name| name.as_str()) != Some("encoding") {
            return Err(error(
                self.source,
                keyword,
                "Path text I/O accepts only encoding='utf-8'",
            ));
        }
        let ast::Expr::StringLiteral(value) = &keyword.value else {
            return Err(error(
                self.source,
                keyword,
                "encoding must be the literal 'utf-8'",
            ));
        };
        check_string(value, self.source)?;
        if value.value.to_str() != "utf-8" {
            return Err(error(
                self.source,
                keyword,
                "encoding must be the literal 'utf-8'",
            ));
        }
        Ok(())
    }

    pub(super) fn path_method(
        &mut self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let name = attribute.attr.as_str();
        if matches!(name, "iterdir" | "glob") {
            return Err(error(
                self.source,
                call,
                "Path iterators require an immediate loop, comprehension, or iterable consumer",
            ));
        }
        let (kind, ty, arity) = match name {
            "exists" => (PathOperation::Exists, Type::Bool, 0),
            "is_file" => (PathOperation::IsFile, Type::Bool, 0),
            "is_dir" => (PathOperation::IsDir, Type::Bool, 0),
            "read_text" => (PathOperation::ReadText, Type::Str, 0),
            "write_text" => (PathOperation::WriteText, Type::Int, 1),
            _ => return Err(error(self.source, call, "unsupported native Path method")),
        };
        if call.arguments.args.len() != arity {
            return Err(error(
                self.source,
                call,
                &format!("Path.{name} requires {arity} positional arguments"),
            ));
        }
        if matches!(kind, PathOperation::ReadText | PathOperation::WriteText) {
            self.path_encoding(call)?;
        } else {
            self.reject_builtin_keywords(call)?;
        }
        let mut args = vec![receiver];
        if let Some(value) = call.arguments.args.first() {
            args.push(self.expr_expected(value, Type::Str)?);
        }
        Ok(operation(kind, ty, args))
    }

    /// Bounded glob syntax has no recursive traversal or dynamic matching protocol.
    /// Matching itself stays native and preserves pathlib's treatment of dotfiles.
    pub(super) fn path_iterator(
        &self,
        receiver: Expr,
        call: &ast::ExprCall,
        glob: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        self.reject_builtin_keywords(call)?;
        let mut args = vec![receiver];
        if glob {
            let [ast::Expr::StringLiteral(pattern)] = call.arguments.args.as_ref() else {
                return Err(error(
                    self.source,
                    call,
                    "Path.glob requires one literal single-component pattern",
                ));
            };
            check_string(pattern, self.source)?;
            let value = pattern.value.to_str();
            if value.is_empty() || value == "." || value.contains('/') || value.contains("**") {
                return Err(error(
                    self.source,
                    pattern,
                    "Path.glob supports nonempty single-component patterns without recursive '**'",
                ));
            }
            args.push(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(value.into()),
            });
        } else if !call.arguments.args.is_empty() {
            return Err(error(self.source, call, "Path.iterdir takes no arguments"));
        }
        Ok(operation(
            if glob {
                PathOperation::Glob
            } else {
                PathOperation::Iterdir
            },
            Type::Iterator,
            args,
        ))
    }
}
