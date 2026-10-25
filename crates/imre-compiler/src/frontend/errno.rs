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
    /// All import spellings project the same module object and target registry.
    pub(super) fn errno_value(&self, target: Binding, expression: &ast::Expr) -> Option<Expr> {
        match target {
            Binding::IntrinsicModule(crate::modules::IntrinsicModule::Errno) => Some(module()),
            Binding::Intrinsic(Intrinsic::ErrnoFrom(slot)) => {
                Some(self.errno_attribute(module(), &self.graph.errno_names[slot]))
            }
            Binding::Intrinsic(Intrinsic::ErrnoAttribute) => {
                let ast::Expr::Attribute(attr) = expression else {
                    return None;
                };
                Some(self.errno_attribute(module(), attr.attr.as_str()))
            }
            _ => None,
        }
    }

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
                Type::ObjectValue,
                vec![owner, unpacking::string(name)],
            ),
        }
    }

    /// Lower reflection on a statically typed module after evaluating its arguments once.
    pub(super) fn errno_builtin(
        &self,
        name: &str,
        call: &ast::ExprCall,
        args: &[Expr],
    ) -> Result<Expr, String> {
        let owner = args[0].clone();
        if name == "vars" && args.len() == 1 {
            return Ok(op(Op::Namespace, Type::ModuleNamespace, vec![owner]));
        }
        if name == "getattr"
            && let [_, key, default] = args
        {
            let key = self.coerce(key.clone(), Type::Str, &call.arguments.args[1])?;
            let default =
                self.coerce(default.clone(), Type::ObjectValue, &call.arguments.args[2])?;
            return Ok(op(
                Op::DefaultLookup,
                Type::ObjectValue,
                vec![owner, key, default],
            ));
        }
        let [_, key] = args else {
            return Err(error(
                self.source,
                call,
                "module attribute lookup requires a name string",
            ));
        };
        if name == "getattr"
            && let ast::Expr::StringLiteral(lit) = &call.arguments.args[1]
        {
            return Ok(self.errno_attribute(owner, lit.value.to_str()));
        }
        let key = self.coerce(key.clone(), Type::Str, &call.arguments.args[1])?;
        Ok(match name {
            "hasattr" => op(Op::Has, Type::Bool, vec![owner, key]),
            "getattr" => op(Op::Lookup, Type::ObjectValue, vec![owner, key]),
            _ => return Err(error(self.source, call, "vars takes exactly one module")),
        })
    }
}
