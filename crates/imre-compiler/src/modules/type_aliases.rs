//! Import-time type expressions have no observable runtime object in this
//! subset: their names may annotate values, or call a sealed NewType constructor.

use super::*;

pub(super) fn type_statement(stmt: &ast::Stmt) -> bool {
    matches!(stmt, ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_))
        || matches!(stmt, ast::Stmt::Assign(assign) if matches!(assign.targets.as_slice(), [ast::Expr::Name(_)]))
}

/// Refuse dynamic subscriptions and unbound names: module-level aliases
/// really evaluate in Python and may raise before the next statement.
pub(super) fn static_type(modules: &[SourceModule], module: ModuleId, value: &ast::Expr) -> bool {
    match value {
        ast::Expr::Name(n) if !modules[module].bindings.contains_key(n.id.as_str()) => {
            matches!(
                n.id.as_str(),
                "str"
                    | "object"
                    | "bytes"
                    | "bool"
                    | "int"
                    | "float"
                    | "list"
                    | "dict"
                    | "set"
                    | "tuple"
                    | "type"
            )
        }
        ast::Expr::Name(_) | ast::Expr::Attribute(_) => matches!(
            resolve_binding(modules, module, value),
            Some(
                Binding::TypeAlias(..)
                    | Binding::Intrinsic(
                        Intrinsic::BinaryIO
                            | Intrinsic::TextIO
                            | Intrinsic::Iterable
                            | Intrinsic::Iterator
                            | Intrinsic::Sequence
                            | Intrinsic::NominalClass("collections.abc.Sequence")
                            | Intrinsic::Mapping { .. }
                            | Intrinsic::Declaration("collections.abc.Collection")
                            | Intrinsic::Literal
                    )
            )
        ),
        ast::Expr::Tuple(t) => t.elts.iter().all(|v| static_type(modules, module, v)),
        ast::Expr::NoneLiteral(_) | ast::Expr::EllipsisLiteral(_) | ast::Expr::StringLiteral(_) => {
            true
        }
        ast::Expr::Subscript(s) => {
            let builtin = s.value.as_name_expr().is_some_and(|n| {
                matches!(n.id.as_str(), "tuple" | "list" | "set" | "dict" | "type")
                    && !modules[module].bindings.contains_key(n.id.as_str())
            });
            let sealed = matches!(
                resolve_binding(modules, module, &s.value),
                Some(Binding::Intrinsic(
                    Intrinsic::Iterable
                        | Intrinsic::Iterator
                        | Intrinsic::Sequence
                        | Intrinsic::NominalClass("collections.abc.Sequence")
                        | Intrinsic::Mapping { .. }
                        | Intrinsic::Declaration("collections.abc.Collection")
                        | Intrinsic::Literal
                ))
            );
            (builtin || sealed) && static_type(modules, module, &s.slice)
        }
        _ => false,
    }
}

impl Loader<'_> {
    pub(super) fn add_alias(
        &mut self,
        module: ModuleId,
        value: &ast::Expr,
        constructor: bool,
    ) -> Binding {
        let aliases = &mut self.modules[module].aliases;
        let index = aliases.len();
        aliases.push(TypeAlias {
            value: value.clone(),
            constructor,
        });
        Binding::TypeAlias(module, index)
    }

    pub(super) fn alias_declaration(
        &mut self,
        module: ModuleId,
        stmt: &ast::Stmt,
    ) -> Result<bool, String> {
        let ast::Stmt::Assign(assign) = stmt else {
            return Ok(false);
        };
        let [ast::Expr::Name(name)] = assign.targets.as_slice() else {
            return Ok(false);
        };
        // A fixed class has one definition. Public aliases such as
        // selectors.DefaultSelector must construct that very same class.
        if matches!(&*assign.value, ast::Expr::Name(_))
            && let Some(binding @ Binding::Definition(def)) =
                resolve_binding(&self.modules, module, &assign.value)
            && matches!(
                &self.modules[def.module].parsed.syntax().body[def.statement],
                ast::Stmt::ClassDef(_)
            )
        {
            self.bind(module, name.id.as_str(), binding, name)?;
            return Ok(true);
        }
        let (value, constructor) = match &*assign.value {
            ast::Expr::Call(call)
                if resolve_binding(&self.modules, module, &call.func)
                    == Some(Binding::Intrinsic(Intrinsic::NewType)) =>
            {
                let [ast::Expr::StringLiteral(label), value] = call.arguments.args.as_ref() else {
                    return Ok(false);
                };
                if !call.arguments.keywords.is_empty()
                    || label.value.to_str() != name.id.as_str()
                    || !matches!(value, ast::Expr::Name(_))
                    || !static_type(&self.modules, module, value)
                {
                    return Ok(false);
                }
                (value, true)
            }
            value @ ast::Expr::Subscript(_) if static_type(&self.modules, module, value) => {
                (value, false)
            }
            _ => return Ok(false),
        };
        let binding = self.add_alias(module, value, constructor);
        self.bind(module, name.id.as_str(), binding, name)?;
        Ok(true)
    }
}
