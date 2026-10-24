//! Select native byte storage for a fresh append-only local consumed by bytes().

use crate::ir::{Builtin, Expr, ExprKind, Function, Statement, Type, walk_expressions};
use crate::reductions::count_writes;

#[derive(Clone, Default)]
struct Uses {
    reads: usize,
    private: usize,
    appends: usize,
}

/// Counts every local read, including eager expression scopes and exceptional
/// exits. Only the append receiver and the compiler's loop guards may observe
/// the local between its unique empty initializer and its terminal conversion.
pub(super) fn select(function: &Function, storage: &mut [Type]) {
    if function.returns != Type::Bytes {
        return;
    }
    let mut uses = vec![Uses::default(); storage.len()];
    let mut converts = false;
    let mut captures = vec![Vec::new(); storage.len()];
    walk_expressions(&function.body, &mut |expr| match &expr.kind {
        ExprKind::Local(slot) => uses[*slot].reads += 1,
        // extend() captures its receiver before evaluating its input. Only
        // this scope-owned, append-only copy can share the private storage.
        ExprKind::Scope { body, clear, .. } => {
            if let Some(Statement::Assign(
                to,
                Expr {
                    ty: Type::ListInt,
                    kind: ExprKind::Local(from),
                },
            )) = body.first()
                && clear.contains(to)
                && to != from
            {
                captures[*to].push(*from);
            }
        }
        ExprKind::Builtin(Builtin::Bytes, args) => {
            converts |= args.first().is_some_and(|arg| arg.ty == Type::ListInt);
        }
        ExprKind::Builtin(op @ (Builtin::Append | Builtin::ListReserveRange), args) => {
            if let ExprKind::Local(slot) = args[0].kind {
                uses[slot].private += 1;
                uses[slot].appends += usize::from(matches!(op, Builtin::Append));
            }
        }
        ExprKind::Builtin(Builtin::ListDistinct, args) => {
            for arg in args {
                if let ExprKind::Local(slot) = arg.kind {
                    uses[slot].private += 1;
                }
            }
        }
        _ => {}
    });
    if !converts {
        return;
    }
    let mut writes = vec![0; storage.len()];
    count_writes(&function.body, &mut writes);
    for &param in &function.params {
        writes[param] += 1;
    }
    let mut aliases = Vec::new();
    for (slot, sources) in captures.iter().enumerate() {
        if let Some(&from) = sources.first()
            && sources.iter().all(|&source| source == from)
            && writes[slot] == 2 * sources.len() // one capture and one scope cleanup
            && uses[slot].reads == uses[slot].appends
        {
            aliases.push((slot, from));
        }
    }
    for &(slot, from) in &aliases {
        uses[from].private += captures[slot].len();
        uses[from].appends += uses[slot].appends;
    }
    block(&function.body, storage, &uses, &writes);
    for (slot, from) in aliases {
        if storage[from] == Type::ByteOutput {
            storage[slot] = Type::ByteOutput;
        }
    }
}

fn block(body: &[Statement], storage: &mut [Type], uses: &[Uses], writes: &[usize]) {
    for (i, stmt) in body.iter().enumerate() {
        match stmt {
            Statement::Return(Expr {
                kind: ExprKind::Builtin(Builtin::Bytes, args),
                ..
            }) => {
                let [
                    Expr {
                        kind: ExprKind::Local(slot),
                        ty: Type::ListInt,
                    },
                ] = args.as_slice()
                else {
                    continue;
                };
                let use_ = &uses[*slot];
                if writes[*slot] == 1 && use_.appends != 0 && use_.reads == use_.private + 1
                    && body[..i].iter().any(|stmt| matches!(stmt, Statement::Assign(to, Expr { kind: ExprKind::List(values), .. }) if to == slot && values.is_empty()))
                {
                    storage[*slot] = Type::ByteOutput;
                }
            }
            Statement::Scope { body, .. } => block(body, storage, uses, writes),
            Statement::If(_, yes, no)
            | Statement::While(_, yes, no)
            | Statement::For {
                body: yes,
                orelse: no,
                ..
            } => {
                block(yes, storage, uses, writes);
                block(no, storage, uses, writes);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                block(body, storage, uses, writes);
                for handler in handlers {
                    block(&handler.body, storage, uses, writes);
                }
                block(orelse, storage, uses, writes);
                block(finalbody, storage, uses, writes);
            }
            _ => {}
        }
    }
}
