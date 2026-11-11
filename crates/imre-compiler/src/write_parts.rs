//! Expose a discarded text write to the existing concatenation lowering.

use crate::ir::{
    Binary, Builtin, Expr, ExprKind, Function, Module, Statement, Type, rewrite_statements,
};

struct Write {
    file_parameter: usize,
    suffix: Option<Expr>,
}

/// A single write can consume the caller's fragments directly when its helper
/// has no observable frame and the receiver already precedes the text.
pub(crate) fn specialize(module: &mut Module) {
    let writes: Vec<_> = module
        .functions
        .iter()
        .enumerate()
        .map(|(id, function)| {
            if module.source_frames.get(id).is_some_and(Option::is_some) {
                None
            } else {
                write_wrapper(function)
            }
        })
        .collect();
    if writes.iter().any(Option::is_some) {
        for function in module.functions.iter_mut().chain([&mut module.entry]) {
            rewrite_statements(&mut function.body, &mut |stmt| fuse(stmt, &writes));
        }
    }
}

/// Accept exactly one write of a parameter with an optional literal suffix.
fn write_wrapper(function: &Function) -> Option<Write> {
    if function.params.len() != 2 || function.returns != Type::None {
        return None;
    }
    let body = match function.body.as_slice() {
        [
            body @ ..,
            Statement::Return(Expr {
                kind: ExprKind::None,
                ..
            }),
        ] => body,
        body => body,
    };
    let [
        Statement::Expr(Expr {
            kind: ExprKind::Builtin(Builtin::TextWrite, args),
            ..
        }),
    ] = body
    else {
        return None;
    };
    let [file, text] = args.as_slice() else {
        return None;
    };
    let ExprKind::Local(file) = file.kind else {
        return None;
    };
    let (value, suffix) = match text.string_addends() {
        Some((left, right)) if matches!(right.kind, ExprKind::Str(_)) => {
            (left, Some(right.clone()))
        }
        _ => (text, None),
    };
    let ExprKind::Local(value) = value.kind else {
        return None;
    };
    let file_parameter = function.params.iter().position(|&slot| slot == file)?;
    (function.params[1 - file_parameter] == value).then_some(Write {
        file_parameter,
        suffix,
    })
}

/// Keep the call when a value escapes, the argument order differs, or the
/// caller already has a materialized string.
fn fuse(statement: &mut Statement, writes: &[Option<Write>]) {
    if let Statement::Expr(call) = statement
        && let ExprKind::Call(id, args) = &mut call.kind
        && let Some(write) = &writes[*id]
        && args.values.len() == 2
        && args.order[write.file_parameter] == 0
        && args.values[1].string_addends().is_some()
    {
        let mut args = std::mem::take(&mut args.values);
        if let Some(suffix) = &write.suffix {
            let text = args.pop().unwrap();
            args.push(Expr {
                ty: Type::Str,
                kind: ExprKind::Binary(Binary::Add, Box::new(text), Box::new(suffix.clone())),
            });
        }
        *call = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::TextWrite, args),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuse_only_discarded_writes_in_source_order() {
        let source = "from typing import TextIO\ndef line(file: TextIO, text: str) -> None:\n    file.write(text + '\\n')\ndef report(file: TextIO, item: str) -> None:\n    line(file, 'A' + item)\n    line(text='B' + item, file=file)\n    line(file, item)\n";
        for tracked in [false, true] {
            let source = if tracked {
                format!("import sys\n{source}\nsys.setrecursionlimit(1100)\n")
            } else {
                source.to_owned()
            };
            let graph = crate::modules::SourceGraph::single(&source).unwrap();
            let mut module = crate::frontend::lower(&graph).unwrap();
            let target = module
                .functions
                .iter()
                .position(|function| write_wrapper(function).is_some())
                .unwrap();
            let count = |module: &Module| {
                let mut calls = 0;
                for function in &module.functions {
                    crate::ir::walk_expressions(&function.body, &mut |value| {
                        calls += usize::from(
                            matches!(value.kind, ExprKind::Call(id, _) if id == target),
                        );
                    });
                }
                calls
            };
            assert_eq!(count(&module), 3);
            assert_eq!(
                module
                    .source_frames
                    .get(target)
                    .is_some_and(Option::is_some),
                tracked
            );
            specialize(&mut module);
            assert_eq!(count(&module), if tracked { 3 } else { 2 });
            crate::verify::module(&module).unwrap();
        }
    }
}
