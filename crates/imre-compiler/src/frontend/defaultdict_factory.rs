//! Check the factory at the boundary of a native defaultdict constructor call.

use super::*;

impl Checker<'_> {
    /// A local named like the import (or its module) remains a normal callable.
    pub(super) fn is_defaultdict_constructor(&self, func: &ast::Expr) -> bool {
        let mut root = func;
        while let ast::Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        !root
            .as_name_expr()
            .is_some_and(|n| self.names.contains_key(n.id.as_str()))
            && self.resolve(func) == Some(Binding::Intrinsic(Intrinsic::DefaultDict))
    }

    /// Construct even empty argument containers before raising. Their elements
    /// cannot be read by a constructor that rejects the factory.
    fn defaultdict_argument(&mut self, arg: &ast::Expr) -> Result<Expr, String> {
        match arg {
            ast::Expr::Dict(value) if value.items.is_empty() => {
                self.expr_expected(arg, self.types.dict(Type::Str, Type::Int))
            }
            ast::Expr::List(value) if value.elts.is_empty() => {
                self.expr_expected(arg, Type::ListInt)
            }
            _ => self.expr(arg),
        }
    }

    /// Issue CPython's constructor TypeError only for a known non-callable
    /// factory. Evaluating a later argument can raise before factory validation.
    pub(super) fn defaultdict_factory_error(
        &mut self,
        call: &ast::ExprCall,
        result: Type,
    ) -> Result<Option<Expr>, String> {
        if !self.is_defaultdict_constructor(&call.func) {
            return Ok(None);
        }
        let Some((factory, rest)) = call.arguments.args.split_first() else {
            return Ok(None);
        };
        match factory {
            ast::Expr::NoneLiteral(_) | ast::Expr::Lambda(_) | ast::Expr::Starred(_) => {
                return Ok(None);
            }
            ast::Expr::Name(name) if !self.names.contains_key(name.id.as_str()) => return Ok(None),
            ast::Expr::Attribute(_) if self.resolve(factory).is_some() => return Ok(None),
            _ => {}
        }
        let first = self.defaultdict_argument(factory)?;
        let invalid = match first.ty {
            Type::Bool
            | Type::Int
            | Type::Float
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::ListInt
            | Type::List(_)
            | Type::Tuple(_)
            | Type::Dict(_)
            | Type::Set(_)
            | Type::Deque(_)
            | Type::PlainObject => true,
            Type::Class(class) => {
                !self.classes.has_concrete_subclasses(class)
                    && !self.classes.methods[class].contains_key("__call__")
            }
            _ => false,
        };
        if !invalid {
            return Ok(None);
        }
        calls::reject_unpacking(call, self.source)?;
        let mut inputs = vec![first];
        for value in rest {
            inputs.push(self.defaultdict_argument(value)?);
        }
        for kw in &call.arguments.keywords {
            inputs.push(self.defaultdict_argument(&kw.value)?);
        }
        inputs.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(
                    "first argument must be callable or None",
                ))),
            },
        });
        self.graph.checked_overload(self.module, call.range());
        Ok(Some(Expr {
            ty: result,
            kind: ExprKind::Builtin(Builtin::RaiseException, inputs),
        }))
    }
}
