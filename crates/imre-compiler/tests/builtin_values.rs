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
