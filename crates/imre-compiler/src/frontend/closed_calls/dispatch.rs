//! Keep correlated callable and argument alternatives together across a finite dispatch.

use super::*;

/// Besides separating native locals, a statically chosen callable retains its
/// parameter names and defaults. Replace a call only while the binding is known.
struct Aliases {
    locals: HashMap<String, ast::name::Name>,
    callees: HashMap<String, ast::Expr>,
}
impl Transformer for Aliases {
    fn visit_expr(&self, expr: &mut ast::Expr) {
        if let ast::Expr::Call(call) = expr
            && let ast::Expr::Name(n) = &*call.func
            && let Some(callee) = self.callees.get(n.id.as_str())
        {
            *call.func = callee.clone();
        }
        if let ast::Expr::Name(n) = expr
            && let Some(alias) = self.locals.get(n.id.as_str())
        {
            n.id = alias.clone();
        } else {
            transformer::walk_expr(self, expr);
        }
    }
}

/// Returns the per-arm locals for one complete branch table, or leaves the
/// ordinary checker in control when any binding can change before the call.
fn dispatch_plans(
    graph: &SourceGraph,
    module: ModuleId,
    branch: &ast::StmtMatch,
    tail: &[ast::Stmt],
    prior: &HashSet<String>,
    writes: &HashSet<String>,
) -> Option<Vec<Option<Aliases>>> {
    let wildcard = |p: &ast::Pattern| {
        matches!(p, ast::Pattern::MatchAs(v)
        if v.pattern.is_none() && v.name.is_none())
    };
    if !branch
        .cases
        .last()
        .is_some_and(|case| wildcard(&case.pattern))
        || branch.cases.iter().any(|case| {
            case.guard.is_some()
                || !(wildcard(&case.pattern)
                    || matches!(
                        case.pattern,
                        ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_)
                    ))
        })
    {
        return None;
    }
    let mut mentioned = Mentions::default();
    mentioned.visit_body(tail);
    if mentioned.1 {
        return None;
    } // A nested scope may bind or capture a selected name.
    // A match keeps the saved subject alive through its case. Moving the tail
    // inside that case cannot extend a visible lifetime if the original local
    // is still bound for the entire tail.
    let subject = branch.subject.as_name_expr()?.id.as_str();
    let rebound = narrowing::assigned_names(tail);
    if rebound.contains(subject) || narrowing::deleted_names(tail).contains(subject) {
        return None;
    }
    let mut names = None;
    let mut plans = Vec::new();
    for (arm, case) in branch.cases.iter().enumerate() {
        if matches!(
            case.body.as_slice(),
            [ast::Stmt::Raise(_) | ast::Stmt::Return(_)]
        ) {
            plans.push(None);
            continue;
        }
        let [ast::Stmt::Assign(a)] = case.body.as_slice() else {
            return None;
        };
        let [ast::Expr::Tuple(targets)] = a.targets.as_slice() else {
            return None;
        };
        let ast::Expr::Tuple(values) = &*a.value else {
            return None;
        };
        let targets: Vec<_> = targets
            .elts
            .iter()
            .map(|e| Some(e.as_name_expr()?.id.to_string()))
            .collect::<Option<_>>()?;
        let mut rhs = Mentions::default();
        rhs.visit_expr(&a.value);
        if targets.len() != values.elts.len()
            || rhs.1
            || targets
                .iter()
                .any(|n| prior.contains(n) || rebound.contains(n) || rhs.0.contains(n))
            || targets.iter().collect::<HashSet<_>>().len() != targets.len()
            || names.as_ref().is_some_and(|ns| ns != &targets)
        {
            return None;
        }
        let mut callees = HashMap::new();
        for (target, value) in targets.iter().zip(&values.elts) {
            if let ast::Expr::Name(n) = value
                && !writes.contains(n.id.as_str())
                && !prior.contains(n.id.as_str())
                && let Some(Binding::Definition(id)) = graph.resolve(module, value)
                && matches!(
                    graph.modules[id.module].parsed.syntax().body[id.statement],
                    ast::Stmt::FunctionDef(_)
                )
            {
                callees.insert(target.clone(), value.clone());
            }
        }
        if callees.is_empty() {
            return None;
        }
        names = Some(targets.clone());
        let locals = targets
            .into_iter()
            .map(|n| {
                let private = format!("@match{}:{arm}:{n}", branch.start().to_u32()).into();
                (n, private)
            })
            .collect();
        plans.push(Some(Aliases { locals, callees }));
    }
    (plans.iter().flatten().count() >= 2).then_some(plans)
}

pub(super) fn specialize(
    function: &mut ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
) {
    let mut prior = Mentions::default();
    prior
        .0
        .extend(parameters(&function.parameters).map(|p| p.name.to_string()));
    let writes = narrowing::assigned_names(&function.body);
    for index in 0..function.body.len().saturating_sub(1) {
        let stmt = &function.body[index];
        if let ast::Stmt::Match(branch) = stmt
            && let Some(plans) = dispatch_plans(
                graph,
                module,
                branch,
                &function.body[index + 1..],
                &prior.0,
                &writes,
            )
        {
            let tail = function.body.split_off(index + 1);
            let ast::Stmt::Match(branch) = &mut function.body[index] else {
                unreachable!()
            };
            for (case, plan) in branch.cases.iter_mut().zip(plans) {
                if let Some(plan) = plan {
                    case.body.extend(tail.iter().cloned());
                    plan.visit_body(&mut case.body);
                }
            }
            break;
        }
        prior.visit_stmt(stmt);
    }
}
