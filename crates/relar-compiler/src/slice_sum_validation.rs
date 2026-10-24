//! Native translation validation for the two slice-sum expressions in the fixture.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::ir::{Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::{CompileOptions, compile, frontend, llvm, modules::SourceGraph, ranges, specialize};

#[derive(Clone, Copy)]
enum Variant {
    Materialized,
    EraseFullSlice,
    StartFirst,
}

fn compile_variant(source: &str, variant: Variant) -> String {
    let mut module = frontend::lower(&SourceGraph::single(source).unwrap()).unwrap();
    let mut changed = 0;
    for function in &mut module.functions {
        let [Statement::Return(sum)] = function.body.as_mut_slice() else {
            continue;
        };
        let ExprKind::Binary(Binary::Add, subtotal, start) = &mut sum.kind else {
            continue;
        };
        let ExprKind::Builtin(Builtin::ListSliceSum, arguments) = &subtotal.kind else {
            continue;
        };
        match variant {
            Variant::Materialized => {
                // Evaluate the original Slice and start as ListSum arguments in order.
                let slice = Expr {
                    ty: Type::ListInt,
                    kind: ExprKind::Builtin(Builtin::Slice, arguments.clone()),
                };
                sum.kind = ExprKind::Builtin(Builtin::ListSum, vec![slice, *start.clone()]);
            }
            Variant::EraseFullSlice => {
                if !arguments[1..].iter().all(|arg| {
                    matches!(&arg.kind, ExprKind::Absent(default) if matches!(default.kind, ExprKind::None))
                }) {
                    continue;
                }
                sum.kind =
                    ExprKind::Builtin(Builtin::ListSum, vec![arguments[0].clone(), *start.clone()]);
            }
            Variant::StartFirst => std::mem::swap(subtotal, start),
        }
        changed += 1;
    }
    let expected = if matches!(variant, Variant::EraseFullSlice) {
        1
    } else {
        2
    };
    assert_eq!(
        changed, expected,
        "expected the fixture's typed slice-sum rewrite sites"
    );
    specialize(&mut module).unwrap();
    llvm::emit(&module, &ranges::analyze(&module), false).unwrap()
}

#[test]
#[ignore = "requires a built native runtime, Clang and CPython"]
fn bounded_slice_sums() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    let source = fs::read_to_string(tests.join("fixtures/bounded_slice_sum.py")).unwrap();
    let work = tempfile::tempdir().unwrap();
    // The production arm uses the public compile entry point unchanged.
    fs::write(
        work.path().join("fused.ll"),
        compile(&source, &CompileOptions::default()).unwrap(),
    )
    .unwrap();
    for (name, variant) in [
        ("materialized", Variant::Materialized),
        ("erase-full-slice", Variant::EraseFullSlice),
        ("start-first", Variant::StartFirst),
    ] {
        fs::write(
            work.path().join(format!("{name}.ll")),
            compile_variant(&source, variant),
        )
        .unwrap();
    }
    let status = Command::new("python3")
        .arg(tests.join("validate_slice_sum.py"))
        .current_dir(work.path())
        .status()
        .unwrap();
    assert!(status.success(), "slice-sum validator exited with {status}");
}
