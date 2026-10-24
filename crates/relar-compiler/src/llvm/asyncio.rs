//! An async thunk borrows exact arguments from the coroutine's captured tuple.
//! Indirect calls also load its callable and bound receiver from that frame.

use super::*;
use crate::ir::ProcessOperation;

pub(super) fn adapters(module: &Module, sanitize: bool) -> String {
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
    let mut output = String::new();
    let attrs = if sanitize { " sanitize_address" } else { "" };
    for (id, frame) in direct {
        let function = &module.functions[id];
        writeln!(
            output,
            "define internal i64 @callable_{id}_async(ptr %frame){attrs} {{"
        )
        .unwrap();
        let args = function
            .params
            .iter()
            .enumerate()
            .map(|(index, &slot)| load_arg(&mut output, frame, index, function.locals[slot]))
            .collect();
        invoke(&mut output, format!("@fn_{id}"), args, function.returns);
    }
    for (id, (frame, result)) in indirect {
        writeln!(
            output,
            "define internal i64 @async_invoke_{id}(ptr %frame){attrs} {{"
        )
        .unwrap();
        let callable = load_arg(&mut output, frame, 0, Type::Callable(id));
        writeln!(output, "  %target = load ptr, {callable}\n  %slot = getelementptr %callable, {callable}, i32 0, i32 1\n  %context = load ptr, ptr %slot").unwrap();
        let mut args = vec!["ptr %context".to_owned()];
        let (params, _) = module.types.signature(Type::Callable(id)).unwrap();
        args.extend(
            params
                .iter()
                .enumerate()
                .map(|(index, &ty)| load_arg(&mut output, frame, index + 1, ty)),
        );
        invoke(&mut output, "%target".to_owned(), args, result);
    }
    output
}

fn load_arg(output: &mut String, frame: Type, index: usize, ty: Type) -> String {
    let llvm = ty.llvm();
    let storage = storage_type(ty);
    writeln!(
        output,
        "  %field{index} = getelementptr %{}, ptr %frame, i32 0, i32 {index}",
        record_name(frame)
    )
    .unwrap();
    writeln!(output, "  %word{index} = load {storage}, ptr %field{index}").unwrap();
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
