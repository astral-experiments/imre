//! Prove field initialization by a virtual method without exposing a partial instance.

use super::*;

pub(super) type Hooks = HashMap<String, Vec<String>>;

/// Recognize an ordinary, argument-free call on the initializer's receiver.
pub(super) fn called_method<'a>(statement: &'a ast::Stmt, receiver: &str) -> Option<&'a str> {
    let ast::Stmt::Expr(stmt) = statement else {
        return None;
    };
    let ast::Expr::Call(call) = &*stmt.value else {
        return None;
    };
    let ast::Expr::Attribute(attr) = &*call.func else {
        return None;
    };
    (call.arguments.is_empty() && matches!(&*attr.value, ast::Expr::Name(n) if n.id == receiver))
        .then(|| attr.attr.as_str())
}

/// A factory may raise or call arbitrary code on the right, but may never pass or read self.
/// The only effects on the incomplete instance are direct writes to declared storage.
fn writes(
    module: &SourceModule,
    method: &ast::StmtFunctionDef,
    kind: MethodKind,
) -> Result<Vec<String>, String> {
    let Some(parameter) = method.parameters.iter_non_variadic_params().next() else {
        return Err(module.error(method, "an initializer hook requires a receiver"));
    };
    if kind != MethodKind::Instance
        || method.is_async
        || method.parameters.len() != 1
        || !method.decorator_list.is_empty()
    {
        return Err(module.error(
            method,
            "an initializer hook must be an ordinary receiver-only method",
        ));
    }
    let receiver = parameter.name().as_str();
    let no_fields = HashSet::new();
    let mut fields = Vec::new();
    for stmt in &method.body {
        if matches!(stmt, ast::Stmt::Pass(_))
            || matches!(stmt, ast::Stmt::Expr(expr) if matches!(&*expr.value, ast::Expr::StringLiteral(_)))
        {
            continue;
        }
        if let ast::Stmt::Assign(assign) = stmt
            && let [ast::Expr::Attribute(attr)] = assign.targets.as_slice()
            && matches!(&*attr.value, ast::Expr::Name(n) if n.id == receiver)
        {
            let mut reads = SelfUse(receiver, None, &no_fields);
            reads.visit_expr(&assign.value);
            if let Some(expr) = reads.1 {
                return Err(module.error(
                    expr,
                    "a field initializer cannot read or expose the incomplete instance",
                ));
            }
            fields.push(attr.attr.to_string());
            continue;
        }
        return Err(module.error(stmt, "a field initializer must write fields directly"));
    }
    fields.sort_unstable();
    if fields.is_empty() || fields.windows(2).any(|p| p[0] == p[1]) {
        return Err(module.error(
            method,
            "a field initializer must write each initialized field once",
        ));
    }
    Ok(fields)
}

/// Resolve the method in the actual layout; ordinary virtual dispatch remains in the emitted code.
pub(super) fn method_writes(
    graph: &SourceGraph,
    methods: &[ast::StmtFunctionDef],
    kinds: &[MethodKind],
    origins: &[DefinitionId],
    name: &str,
) -> Option<Result<Vec<String>, String>> {
    let i = methods.iter().position(|m| m.name.as_str() == name)?;
    Some(writes(
        &graph.modules[origins[i].module],
        &methods[i],
        kinds[i],
    ))
}
