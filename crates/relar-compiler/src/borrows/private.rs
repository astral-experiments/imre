//! Identify function-private accumulators whose mutations cannot invalidate
//! projections from incoming parameters. A fresh constructor must also keep its
//! receiver private throughout initialization.

use super::{statements, written, written_except};
use crate::ir::{self, Builtin, Expr, ExprKind, Function, Module, Statement, Type};

pub(super) fn analyze(module: &Module) -> Vec<Vec<bool>> {
    let functions = module.functions.iter().chain([&module.entry]);
    let confined = functions.clone().map(confined_locals).collect::<Vec<_>>();
    let private_receiver = functions
        .clone()
        .zip(&confined)
        .map(|(function, confined)| {
            function
                .params
                .first()
                .is_some_and(|&slot| confined[slot] && !written(function)[slot])
        })
        .collect::<Vec<_>>();
    functions
        .zip(&confined)
        .map(|(function, confined)| {
            let invalid = written_except(function, |value| match value.kind {
                ExprKind::Construct { initializer, .. } => {
                    private_receiver.get(initializer) == Some(&true)
                }
                ExprKind::Set { .. } => true,
                _ => false,
            });
            function
                .locals
                .iter()
                .enumerate()
                .map(|(slot, ty)| {
                    matches!(ty, Type::Class(_) | Type::Set(_))
                        && confined[slot]
                        && !invalid[slot]
                        && !function.params.contains(&slot)
                })
                .collect()
        })
        .collect()
}

/// Each use must be a direct field receiver, a set-insertion receiver or a
/// final return. Passing or storing the object itself exposes the whole object.
fn confined_locals(function: &Function) -> Vec<bool> {
    fn account(uses: &mut [isize], expr: &Expr, count: isize) {
        if let ExprKind::Local(slot) = expr.kind {
            uses[slot] += count;
        }
    }
    let mut uses = vec![0; function.locals.len()];
    statements(&function.body, &mut |statement| match statement {
        Statement::SetField(receiver, _, _) | Statement::AugmentField(receiver, _, _, _) => {
            account(&mut uses, receiver, -1);
        }
        Statement::Return(value) => {
            if let ExprKind::Local(slot) = value.kind
                && !function.params.contains(&slot)
            {
                uses[slot] -= 1;
            }
        }
        _ => {}
    });
    ir::walk_expressions(&function.body, &mut |expr| {
        account(&mut uses, expr, 1);
        match &expr.kind {
            ExprKind::Field(receiver, _) => account(&mut uses, receiver, -1),
            ExprKind::Builtin(Builtin::SetAdd, args) => account(&mut uses, &args[0], -1),
            _ => {}
        }
    });
    uses.into_iter().map(|escaping| escaping == 0).collect()
}
