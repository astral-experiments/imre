//! Typed projections of a sealed native module; register values are read from the target.
use super::*;
use crate::ir::ErrnoOperation as Op;

pub(super) fn op(op: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Errno(op), args),
    }
}
pub(super) fn module() -> Expr {
    op(Op::Module, Type::NativeModule, vec![])
}

impl Checker<'_> {
    /// Build the same structured exception as Python's from-import machinery.
    pub(super) fn check_errno_import(&self, name: &str) -> Statement {
        let condition = op(Op::Has, Type::Bool, vec![module(), unpacking::string(name)]);
        let message = format!("cannot import name '{name}' from 'errno' (unknown location)");
        let err = self.import_error_value(
            41,
            vec![unpacking::string(message)],
            vec![unpacking::string("errno"), unpacking::string(name)],
            [Some(1), None, Some(2)],
        );
        Statement::If(condition, Vec::new(), vec![Statement::Raise(Some(err))])
    }

    pub(super) fn errno_attribute(&self, owner: Expr, name: &str) -> Expr {
        match name {
            "__dict__" => op(Op::Namespace, Type::ModuleNamespace, vec![owner]),
            "errorcode" => op(
                Op::Errorcode,
                self.types.dict(Type::Int, Type::Str),
                vec![owner],
            ),
            "__name__" | "__doc__" | "__package__" => {
                op(Op::Text, Type::Str, vec![owner, unpacking::string(name)])
            }
            name if (name.starts_with('E') || name.starts_with("WSA"))
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') =>
            {
                op(Op::Number, Type::Int, vec![owner, unpacking::string(name)])
            }
            _ => op(
                Op::Lookup,
                Type::ModuleMember,
                vec![owner, unpacking::string(name)],
            ),
        }
    }

    pub(super) fn errno_builtin(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        let Some(name) = call.func.as_name_expr() else {
            return Ok(None);
        };
        if self.resolve(&call.func).is_some()
            || self.names.contains_key(name.id.as_str())
            || !matches!(name.id.as_str(), "getattr" | "hasattr" | "vars")
        {
            return Ok(None);
        }
        let Some(first) = call.arguments.args.first() else {
            return Ok(None);
        };
        let owner = self.expr(first)?;
        if owner.ty != Type::NativeModule {
            return Ok(None);
        }
        self.reject_builtin_keywords(call)?;
        if name.id == "vars" && call.arguments.args.len() == 1 {
            return Ok(Some(op(Op::Namespace, Type::ModuleNamespace, vec![owner])));
        }
        if name.id == "getattr"
            && let [_, key, default] = call.arguments.args.as_ref()
        {
            let key = self.expr_expected(key, Type::Str)?;
            let default = self.expr_expected(default, Type::ModuleMember)?;
            return Ok(Some(op(
                Op::DefaultLookup,
                Type::ModuleMember,
                vec![owner, key, default],
            )));
        }
        let [_, key] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "module attribute lookup requires a name string",
            ));
        };
        if name.id == "getattr"
            && let ast::Expr::StringLiteral(lit) = key
        {
            return Ok(Some(self.errno_attribute(owner, lit.value.to_str())));
        }
        let key = self.expr_expected(key, Type::Str)?;
        Ok(Some(match name.id.as_str() {
            "hasattr" => op(Op::Has, Type::Bool, vec![owner, key]),
            "getattr" => op(Op::Lookup, Type::ModuleMember, vec![owner, key]),
            _ => return Err(error(self.source, call, "vars takes exactly one module")),
        }))
    }
}
