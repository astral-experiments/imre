//! Prove field initialization by a virtual method without exposing a partial instance.

use super::*;

#[derive(Clone)]
pub(crate) struct Hook {
    pub(super) initialized: HashSet<String>,
    pub(super) writes: Vec<String>,
}

pub(crate) type Hooks = HashMap<String, Hook>;

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

/// Check the fields an initializer hook writes on a normal return.
fn writes(
    module: &SourceModule,
    method: &ast::StmtFunctionDef,
    kind: MethodKind,
    inherited: Option<&Hook>,
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
    let builtin_super = receiver != "super" && !module.bindings.contains_key("super");
    let mut initialized: HashSet<_> = inherited
        .into_iter()
        .flat_map(|hook| hook.initialized.iter().map(String::as_str))
        .collect();
    let mut fields = Vec::new();
    for stmt in &method.body {
        if matches!(stmt, ast::Stmt::Pass(_))
            || matches!(stmt, ast::Stmt::Expr(expr) if matches!(&*expr.value, ast::Expr::StringLiteral(_)))
        {
            continue;
        }
        if builtin_super {
            if let Some(parent) = inherited
                && let ast::Stmt::Expr(expr) = stmt
                && let ast::Expr::Call(call) = &*expr.value
                && call.arguments.is_empty()
                && super_method(call)
                    .is_some_and(|(attr, _)| attr.attr.as_str() == method.name.as_str())
            {
                fields.extend(parent.writes.iter().cloned());
                initialized.extend(parent.writes.iter().map(String::as_str));
                continue;
            }
            if ast::helpers::any_over_body(
                std::slice::from_ref(stmt),
                |expr| matches!(expr, ast::Expr::Call(call) if super_method(call).is_some()),
            ) {
                return Err(module.error(
                    stmt,
                    "a field initializer cannot read or expose the incomplete instance",
                ));
            }
        }
        if let ast::Stmt::Assign(assign) = stmt
            && let [ast::Expr::Attribute(attr)] = assign.targets.as_slice()
            && matches!(&*attr.value, ast::Expr::Name(n) if n.id == receiver)
        {
            let mut reads = SelfUse(receiver, None, &initialized);
            reads.visit_expr(&assign.value);
            if let Some(expr) = reads.1 {
                return Err(module.error(
                    expr,
                    "a field initializer cannot read or expose the incomplete instance",
                ));
            }
            fields.push(attr.attr.to_string());
            initialized.insert(attr.attr.as_str());
            continue;
        }
        if let Some(parent) = inherited {
            // Side effects and failures can observe fields initialized before this
            // call. On an exceptional exit no partially built instance is returned.
            let guarded_raise = matches!(stmt, ast::Stmt::If(branch)
                if branch.elif_else_clauses.is_empty()
                    && matches!(branch.body.as_slice(), [ast::Stmt::Raise(_)]));
            if guarded_raise || matches!(stmt, ast::Stmt::Expr(_) | ast::Stmt::Raise(_)) {
                let mut reads = SelfUse(receiver, None, &initialized);
                reads.visit_stmt(stmt);
                if let Some(expr) = reads.1 {
                    return Err(module.error(
                        expr,
                        "a field initializer cannot expose the incomplete instance",
                    ));
                }
                if matches!(stmt, ast::Stmt::Raise(_)) {
                    return Ok(parent.writes.clone());
                }
                continue;
            }
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
    inherited: Option<&Hook>,
) -> Option<Result<Vec<String>, String>> {
    let i = methods.iter().position(|m| m.name.as_str() == name)?;
    Some(writes(
        &graph.modules[origins[i].module],
        &methods[i],
        kinds[i],
        inherited,
    ))
}
