//! Native memoization for statically decorated module functions.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct Policy {
    // Negative means unbounded; the decorator normalizes negative sizes to zero.
    capacity: i64,
}

pub(super) fn policy(
    graph: &SourceGraph,
    module: ModuleId,
    def: &ast::StmtFunctionDef,
) -> Result<Option<Policy>, String> {
    let [decorator] = def.decorator_list.as_slice() else {
        return Ok(None);
    };
    let expression = &decorator.expression;
    match graph.resolve(module, expression) {
        Some(Binding::Intrinsic(Intrinsic::FunctionCache)) => {
            return Ok(Some(Policy { capacity: -1 }));
        }
        Some(Binding::Intrinsic(Intrinsic::LruCache)) => return Ok(Some(Policy { capacity: 128 })),
        _ => {}
    }
    let ast::Expr::Call(call) = expression else {
        return Ok(None);
    };
    if graph.resolve(module, &call.func) != Some(Binding::Intrinsic(Intrinsic::LruCache)) {
        return Ok(None);
    }
    let invalid = || {
        graph.modules[module].error(call, "lru_cache requires a constant maxsize and typed flag")
    };
    let args = &call.arguments;
    if args.args.len() > 2 {
        return Err(invalid());
    }
    let mut size = args.args.first();
    let mut typed = args.args.get(1);
    for kw in &args.keywords {
        let slot = match kw.arg.as_deref() {
            Some("maxsize") => &mut size,
            Some("typed") => &mut typed,
            _ => return Err(invalid()),
        };
        if slot.replace(&kw.value).is_some() {
            return Err(invalid());
        }
    }
    if typed.is_some_and(|e| !matches!(e, ast::Expr::BooleanLiteral(_))) {
        return Err(invalid());
    }
    let size = match size {
        None => 128,
        Some(ast::Expr::NoneLiteral(_)) => -1,
        Some(ast::Expr::NumberLiteral(n)) if let ast::Number::Int(i) = &n.value => {
            i.as_i64().ok_or_else(invalid)?
        }
        Some(ast::Expr::UnaryOp(op))
            if op.op == ast::UnaryOp::USub
                && matches!(&*op.operand, ast::Expr::NumberLiteral(n) if matches!(n.value, ast::Number::Int(_))) =>
        {
            0
        }
        _ => return Err(invalid()),
    };
    Ok(Some(Policy { capacity: size }))
}

fn builtin(op: Builtin, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(op, args),
    }
}

pub(super) fn wrapper(
    signature: &Signature,
    raw: usize,
    types: &Types,
    policy: Policy,
) -> ir::Function {
    let id = signature.id;
    let value = signature.returns;
    let mut locals: Vec<_> = signature.params.iter().map(|p| p.ty).collect();
    let params: Vec<_> = (0..locals.len()).collect();
    let local = |slot, ty| Expr {
        ty,
        kind: ExprKind::Local(slot),
    };
    let args: Vec<_> = locals
        .iter()
        .enumerate()
        .map(|(i, &ty)| local(i, ty))
        .collect();
    let mut body = Vec::new();
    let key = match args.as_slice() {
        [] => pipelines::integer(0),
        [arg] => arg.clone(),
        _ => {
            let ty = types.tuple(locals.clone());
            let slot = locals.len();
            locals.push(ty);
            body.push(Statement::Assign(
                slot,
                Expr {
                    ty,
                    kind: ExprKind::Tuple(args.clone()),
                },
            ));
            local(slot, ty)
        }
    };
    let table = types.dict(key.ty, value);
    let table_slot = locals.len();
    let result_slot = table_slot + 1;
    locals.extend([table, value]);
    let cache = || builtin(Builtin::FunctionCache(id), table, vec![]);
    body.push(Statement::Assign(table_slot, cache()));
    body.push(Statement::If(
        builtin(
            Builtin::FunctionCacheProbe(id, policy.capacity),
            Type::Bool,
            vec![key.clone()],
        ),
        vec![Statement::Return(builtin(
            Builtin::DictGet,
            value,
            vec![local(table_slot, table), key.clone()],
        ))],
        vec![],
    ));
    body.push(Statement::Clear(table_slot));
    body.push(Statement::Assign(
        result_slot,
        Expr {
            ty: value,
            kind: ExprKind::Call(raw, ir::CallArgs::positional(args)),
        },
    ));
    if policy.capacity != 0 {
        // Reacquire after user code: it may have cleared this cache while running.
        body.push(Statement::Assign(table_slot, cache()));
        if policy.capacity < 0 {
            body.push(Statement::SetItem(
                local(table_slot, table),
                key,
                local(result_slot, value),
            ));
        } else {
            body.push(Statement::Expr(builtin(
                Builtin::FunctionCacheInsert(id, policy.capacity),
                Type::None,
                vec![key, local(result_slot, value)],
            )));
        }
    }
    body.push(Statement::Return(local(result_slot, value)));
    ir::Function {
        params,
        returns: value,
        locals,
        body,
    }
}

impl Checker<'_> {
    pub(super) fn cache_control(&self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        let ast::Expr::Attribute(attr) = &*call.func else {
            return Ok(None);
        };
        if !matches!(attr.attr.as_str(), "cache_info" | "cache_clear") {
            return Ok(None);
        }
        let Some(Binding::Definition(def)) = self.resolve(&attr.value) else {
            return Ok(None);
        };
        let Some(sig) = self.functions.get(&def) else {
            return Ok(None);
        };
        let Some(policy) = self.classes.cached.get(&sig.id) else {
            return Ok(None);
        };
        if !call.arguments.is_empty() {
            return Err(error(
                self.source,
                call,
                "cache_info and cache_clear take no arguments",
            ));
        }
        if attr.attr.as_str() == "cache_clear" {
            return Ok(Some(builtin(
                Builtin::FunctionCacheClear(sig.id),
                Type::None,
                vec![],
            )));
        }
        let stat = |field| builtin(Builtin::FunctionCacheStat(sig.id, field), Type::Int, vec![]);
        let size = if policy.capacity < 0 {
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }
        } else {
            pipelines::integer(policy.capacity)
        };
        let fields = vec![stat(0), stat(1), size, stat(3)];
        Ok(Some(Expr {
            ty: self.types.tuple(fields.iter().map(|v| v.ty).collect()),
            kind: ExprKind::Tuple(fields),
        }))
    }
}
