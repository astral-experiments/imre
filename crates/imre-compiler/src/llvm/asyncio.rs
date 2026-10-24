//! One-shot coroutine and context-manager thunks pass arguments from their
//! captured tuple using the target's native ownership convention.

use super::*;
use crate::ir::ProcessOperation;

pub(super) fn adapters(
    module: &Module,
    sanitize: bool,
    resource_types: &HashSet<Type>,
    units: &mut Output,
) {
    let mut direct = BTreeMap::new();
    let mut indirect = BTreeMap::new();
    for f in module.functions.iter().chain([&module.entry]) {
        crate::ir::walk_expressions(&f.body, &mut |expr| {
            let ExprKind::Builtin(Builtin::Process(op), args) = &expr.kind else {
                return;
            };
            match op {
                ProcessOperation::Coroutine { function, .. }
                | ProcessOperation::ContextNew { function } => {
                    direct.insert(*function, args[0].ty);
                }
                ProcessOperation::IndirectCoroutine {
                    signature: Type::Callable(signature),
                    result,
                } => {
                    indirect.insert(*signature, (args[0].ty, *result));
                }
                _ => {}
            }
        });
    }
    let attrs = if sanitize { " sanitize_address" } else { "" };
    for (id, frame) in direct {
        let function = &module.functions[id];
        let target = units.function_name(id);
        let thunk = units.async_name(id);
        let output = units.function(units.function_owner(id), &thunk, "i64", "ptr %frame", attrs);
        let args = function
            .params
            .iter()
            .enumerate()
            .map(|(index, &slot)| {
                let ty = function.locals[slot];
                load_arg(output, frame, index, ty, resource_types.contains(&ty))
            })
            .collect();
        invoke(output, format!("@{target}"), args, function.returns);
    }
    for (id, (frame, result)) in indirect {
        let output = units.function(
            units.entry,
            &format!("async_invoke_{id}"),
            "i64",
            "ptr %frame",
            attrs,
        );
        let callable = load_arg(output, frame, 0, Type::Callable(id), true);
        writeln!(output, "  %target = load ptr, {callable}").unwrap();
        let mut args = vec![callable];
        let (params, _) = module.types.signature(Type::Callable(id)).unwrap();
        args.extend(params.iter().enumerate().map(|(index, &ty)| {
            load_arg(output, frame, index + 1, ty, resource_types.contains(&ty))
        }));
        invoke(output, "%target".to_owned(), args, result);
    }
}

fn load_arg(output: &mut String, frame: Type, index: usize, ty: Type, transfer: bool) -> String {
    let llvm = ty.llvm();
    let storage = storage_type(ty);
    writeln!(
        output,
        "  %field{index} = getelementptr %{}, ptr %frame, i32 0, i32 {index}",
        record_name(frame)
    )
    .unwrap();
    writeln!(output, "  %word{index} = load {storage}, ptr %field{index}").unwrap();
    if transfer {
        // Move this private frame's owner into the parameter. Rebinding can
        // now finalize it while the body is still running or suspended.
        writeln!(
            output,
            "  store {storage} zeroinitializer, ptr %field{index}"
        )
        .unwrap();
    }
    if storage == llvm {
        format!("{llvm} %word{index}")
    } else {
        writeln!(
            output,
            "  %arg{index} = trunc {storage} %word{index} to {llvm}"
        )
        .unwrap();
        format!("{llvm} %arg{index}")
    }
}

fn invoke(output: &mut String, target: String, args: Vec<String>, result: Type) {
    let ty = result.llvm();
    writeln!(
        output,
        "  %result = call {ty} {target}({})",
        args.join(", ")
    )
    .unwrap();
    if ty == "i64" {
        output.push_str("  ret i64 %result\n}\n");
    } else {
        let op = match ty {
            "ptr" => "ptrtoint",
            "double" => "bitcast",
            _ => "zext",
        };
        writeln!(
            output,
            "  %word = {op} {ty} %result to i64\n  ret i64 %word\n}}"
        )
        .unwrap();
    }
}
