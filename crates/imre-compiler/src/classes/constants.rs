//! Descriptor-free values in a fixed class namespace.

use super::*;
use std::rc::Rc;

pub(super) fn attribute_writes(
    graph: &SourceGraph,
    names: &HashSet<&str>,
) -> HashMap<String, HashSet<Type>> {
    use ruff_python_ast::visitor::walk_stmt;
    if names.is_empty() {
        return HashMap::new();
    }
    struct Writes<'a>(HashMap<String, HashSet<Type>>, &'a HashSet<&'a str>);
    impl<'a> Visitor<'a> for Writes<'_> {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if let ast::Expr::Attribute(attr) = expr
                && attr.ctx != ast::ExprContext::Load
                && self.1.contains(attr.attr.as_str())
            {
                self.0.entry(attr.attr.to_string()).or_default();
            }
            walk_expr(self, expr);
        }
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if let ast::Stmt::Assign(assign) = stmt {
                let hint = match &*assign.value {
                    ast::Expr::NumberLiteral(n) => match &n.value {
                        ast::Number::Int(_) => Some(Type::Int),
                        ast::Number::Float(_) => Some(Type::Float),
                        _ => None,
                    },
                    ast::Expr::BooleanLiteral(_) => Some(Type::Bool),
                    ast::Expr::StringLiteral(_) => Some(Type::Str),
                    ast::Expr::BytesLiteral(_) => Some(Type::Bytes),
                    _ => None,
                };
                for target in &assign.targets {
                    if let (Some(ty), ast::Expr::Attribute(attr)) = (hint, target)
                        && self.1.contains(attr.attr.as_str())
                    {
                        self.0.entry(attr.attr.to_string()).or_default().insert(ty);
                    }
                }
            }
            walk_stmt(self, stmt);
        }
    }
    let mut writes = Writes(HashMap::new(), names);
    for module in &graph.modules {
        for stmt in &module.parsed.syntax().body {
            writes.visit_stmt(stmt);
        }
    }
    writes.0
}

#[derive(Clone)]
pub(crate) enum ClassValue {
    Binding(Binding),
    String(String),
    /// A plain value computed once while its defining class body executes.
    Runtime {
        owner: DefinitionId,
        ordinal: usize,
    },
    Strings {
        // An alias or inherited class member reads the original global.
        owner: DefinitionId,
        ordinal: usize,
        values: Rc<[String]>,
        literal: bool,
    },
}

/// The ordinary expression checker still verifies the type and all operands.
/// Nested scopes have different name lookup in a Python class body.
pub(super) fn runtime_expression(expr: &ast::Expr) -> bool {
    matches!(
        expr,
        ast::Expr::Name(_)
            | ast::Expr::Attribute(_)
            | ast::Expr::Call(_)
            | ast::Expr::List(_)
            | ast::Expr::Set(_)
            | ast::Expr::Dict(_)
            | ast::Expr::DictComp(_)
            | ast::Expr::NumberLiteral(_)
            | ast::Expr::BooleanLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::BinOp(_)
            | ast::Expr::UnaryOp(_)
            | ast::Expr::Subscript(_)
            | ast::Expr::If(_)
    )
}

/// A class body can concatenate a tuple it declared earlier with a literal
/// suffix. Unqualified names do not search inherited class attributes.
pub(super) fn value(
    graph: &SourceGraph,
    owner: DefinitionId,
    ordinal: usize,
    expr: &ast::Expr,
    locals: &HashSet<String>,
    prior: &HashMap<String, ClassValue>,
) -> Result<Option<ClassValue>, String> {
    let module = &graph.modules[owner.module];
    let prior_tuple = |node: &ast::Expr| {
        let name = node.as_name_expr()?.id.as_str();
        let prior = locals.contains(name).then(|| prior.get(name)).flatten()?;
        matches!(prior, ClassValue::Strings { .. }).then_some(prior)
    };
    if let ast::Expr::StringLiteral(value) = expr {
        crate::frontend::check_string(value, &module.source)?;
        return Ok(Some(ClassValue::String(value.value.to_str().to_owned())));
    }
    if let Some(alias) = prior_tuple(expr) {
        return Ok(Some(alias.clone()));
    }
    let (left, tuple) = match expr {
        ast::Expr::Tuple(tuple) => (None, tuple),
        ast::Expr::BinOp(add)
            if add.op == ast::Operator::Add
                && let Some(left) = prior_tuple(&add.left)
                && let ast::Expr::Tuple(right) = add.right.as_ref() =>
        {
            (Some(left), right)
        }
        _ => {
            return Ok(
                super::class_static_binding(graph, owner.module, expr, locals)
                    .map(ClassValue::Binding),
            );
        }
    };
    if let Some(alias) = left
        && tuple.elts.is_empty()
    {
        return Ok(Some(alias.clone()));
    }
    let prefix = match left {
        Some(ClassValue::Strings { values, .. }) => values.as_ref(),
        _ => &[],
    };
    // The compiler stores the exact fixed tuple shape; bound eager expansion.
    if prefix.len().saturating_add(tuple.elts.len()) > 1024 {
        return Err(module.error(expr, "a fixed class tuple supports at most 1024 strings"));
    }
    let mut values = prefix.to_vec();
    for item in &tuple.elts {
        let ast::Expr::StringLiteral(value) = item else {
            return Ok(None);
        };
        crate::frontend::check_string(value, &module.source)?;
        values.push(value.value.to_str().to_owned());
    }
    Ok(Some(ClassValue::Strings {
        owner,
        ordinal,
        values: values.into(),
        // The empty tuple's addition returns the literal suffix itself.
        literal: prefix.is_empty(),
    }))
}
