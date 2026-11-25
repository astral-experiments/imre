//! Native-only admission is shared by checking, IR generation, and incremental builds.
use imre_compiler::{CodegenLayout, CompileOptions, IncrementalCompiler};

const TYPED: &str = "def inc(n: int) -> int:\n    return n + 1\n\nprint(inc(41))\n";
const COLD: &str = "from math import sqrt\n\ndef inc(n: int) -> int:\n    if n < 0:\n        return int(sqrt(-n))\n    return n + 1\n\nprint(inc(41))\n";
const REFUSAL: &str = "native-only compilation refused the general-purpose Python runtime";

#[test]
fn fixed_entry_is_accepted() {
    imre_compiler::check(TYPED).unwrap();
    let ir = imre_compiler::compile(TYPED, &CompileOptions::default()).unwrap();
    assert!(ir.contains("@imre_typed_start("));
    assert!(
        !ir.lines()
            .any(|line| line.contains("call ") && line.contains("@imre_dyn_"))
    );
}

#[test]
fn check_and_compile_agree_on_the_cold_branch() {
    let checked = imre_compiler::check(COLD);
    match imre_compiler::compile(COLD, &CompileOptions::default()) {
        Ok(ir) => {
            // A future fixed lowering for sqrt may legitimately accept this source.
            checked.unwrap();
            assert!(ir.contains("@imre_typed_start("));
            assert!(
                !ir.lines()
                    .any(|line| line.contains("call ") && line.contains("@imre_dyn_"))
            );
        }
        Err(built) => {
            let checked = checked.unwrap_err();
            assert!(checked.contains(REFUSAL), "{checked}");
            assert!(built.contains(REFUSAL), "{built}");
        }
    }
}

#[test]
fn all_layouts_and_incremental_revisions_obey_the_same_policy() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("entry.py");
    let options = CompileOptions::default();
    let mut compiler = IncrementalCompiler::default();
    std::fs::write(&path, TYPED).unwrap();
    compiler.check_file(&path, None).unwrap();
    compiler.compile_file_units(&path, None, &options).unwrap();
    std::fs::write(&path, COLD).unwrap();
    let cold_is_typed = imre_compiler::check_file(&path, None).is_ok();
    for layout in [
        CodegenLayout::Whole,
        CodegenLayout::Split,
        CodegenLayout::SplitWithWhole,
        CodegenLayout::ThinLto,
    ] {
        match imre_compiler::compile_file_with_layout(&path, None, &options, layout) {
            Ok((units, _)) => {
                assert!(cold_is_typed);
                assert!(
                    units
                        .units
                        .iter()
                        .any(|unit| unit.llvm_ir.contains("@imre_typed_start("))
                );
                assert!(units.units.iter().all(|unit| {
                    !unit
                        .llvm_ir
                        .lines()
                        .any(|line| line.contains("call ") && line.contains("@imre_dyn_"))
                }));
            }
            Err(error) => {
                assert!(!cold_is_typed);
                assert!(error.contains(REFUSAL), "{error}");
            }
        }
    }
    assert_eq!(compiler.check_file(&path, None).is_ok(), cold_is_typed);
    let revised = compiler.compile_file_units(&path, None, &options);
    assert_eq!(revised.is_ok(), cold_is_typed);
    if let Err(error) = revised {
        assert!(error.contains(REFUSAL), "{error}");
    }
    std::fs::write(&path, TYPED).unwrap();
    compiler.check_file(&path, None).unwrap();
    compiler.compile_file_units(&path, None, &options).unwrap();
}

#[test]
fn syntax_errors_keep_their_original_diagnostic() {
    let error = imre_compiler::check("def broken(:\n    pass\n").unwrap_err();
    assert!(!error.contains(REFUSAL), "{error}");
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("broken.py");
    std::fs::write(&path, "def broken(:\n    pass\n").unwrap();
    let file_error = imre_compiler::check_file(&path, None).unwrap_err();
    assert!(!file_error.contains(REFUSAL), "{file_error}");
}
