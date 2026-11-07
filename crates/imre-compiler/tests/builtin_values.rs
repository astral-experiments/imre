#[test]
fn builtin_function_values_preserve_checked_calls() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.py");
    let caller = "\
from typing import get_overloads, overload

def takes_int(value: int) -> None: pass

for function in (sum, print):
    overload(function)
    assert get_overloads(function) == []
";
    std::fs::write(&path, caller).unwrap();
    imre_compiler::check_file(&path, None).unwrap();

    std::fs::write(&path, format!("{caller}\ntakes_int('wrong')\n")).unwrap();
    let error = imre_compiler::check_file(&path, None).unwrap_err();
    assert!(
        error.contains("error[invalid-argument-type]")
            && error.contains("Argument to function `takes_int`"),
        "{error}"
    );
}

#[test]
fn math_perm_with_computed_none_keeps_imported_calls_checked() {
    let root = tempfile::tempdir().unwrap();
    let scores = root.path().join("scores.py");
    let entry = root.path().join("main.py");
    std::fs::write(&scores, "def take(value: int) -> int:\n    return value\n").unwrap();
    let source = "\
import math
from scores import take

def default_k():
    return None

def unused():
    return math.perm(6, default_k())
";
    std::fs::write(&entry, format!("{source}\nprint(take(7))\n")).unwrap();
    let program =
        imre_compiler::compile_file(&entry, Some(root.path()), &Default::default()).unwrap();
    assert!(!program.llvm_ir.contains("@imre_boxed_named_op"));

    std::fs::write(&entry, format!("{source}\nprint(take('x'))\n")).unwrap();
    let error = imre_compiler::check_file(&entry, Some(root.path())).unwrap_err();
    assert!(error.contains("int") && error.contains("str"), "{error}");
}
