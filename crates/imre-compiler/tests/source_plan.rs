use imre_compiler::{CompileOptions, check, check_file, compile, compile_file};

#[test]
fn callback_uses_a_registered_entry_from_source_and_file() {
    let source = r#"def deliver(n: int, send):
    send(n + 1)

def first(value):
    print(value)

def second(value):
    print(value + 2)

deliver(4, {"one": first, "two": second}[input()])
"#;
    let options = CompileOptions::default();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("callback.py");
    std::fs::write(&file, source).unwrap();

    check(source).unwrap();
    check_file(&file, None).unwrap();
    let memory = compile(source, &options).unwrap();
    let physical = compile_file(&file, None, &options).unwrap();
    assert!(memory.contains("call ptr @imre_dyn_new("));
    assert!(physical.source_inputs.boxed_imports);

    let invalid = source.replace("deliver(4,", "deliver('x',");
    let expected =
        "/input.py:10:9: Imre: expected int, got str; annotations are enforced contracts";
    assert_eq!(check(&invalid).unwrap_err(), expected);
    assert_eq!(compile(&invalid, &options).unwrap_err(), expected);
    std::fs::write(&file, invalid).unwrap();
    assert_eq!(
        check_file(&file, None).unwrap_err(),
        expected.replace("/input.py", file.to_str().unwrap())
    );
}

#[test]
fn a_named_comprehension_checks_its_conditional_capture_before_the_filter() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("main.py"),
        r#"def accepts(value: int) -> bool:
    return bool(value)
def rows(items: list[int], flag: bool) -> list[str]:
    lookup: dict[int, int] = {item: item for item in items}
    if flag:
        word = "kept"
    return [word for item in items if accepts("bad")]
print(rows([], False))
"#,
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_imre"))
        .current_dir(project.path())
        .args(["-B", "-m", "main"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("error[invalid-argument-type]"), "{error}");
    assert!(error.contains("main.py:7:47"), "{error}");
}

#[test]
fn eager_filters_use_the_checked_result_of_the_outer_call() {
    let source = r#"
def options() -> list[str]:
    return ["alpha", "beta"]
def text() -> str:
    return "ab"
def choose(prefix: str) -> list[str]:
    return [s for s in options() if s.startswith(prefix)]
def characters(keys: set[str]) -> set[str]:
    return {c for c in text() if c in keys}
"#;
    let llvm = compile(source, &CompileOptions::default()).unwrap();
    for kernel in ["imre_str_list_filter", "imre_filter_character_set"] {
        assert!(llvm.contains(&format!("call ptr @{kernel}(")), "{kernel}");
    }
}

#[test]
fn a_deferred_nullable_parameter_keeps_the_imported_call_and_source_contract() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("sink.py"),
        "def send(value):\n    print(value)\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("provider.py"),
        r#"from __future__ import annotations
from collections.abc import Sequence
import sys

def forward(argv: Sequence[str] | None = None) -> None:
    from sink import send
    send(argv or sys.argv[1:])
"#,
    )
    .unwrap();
    let main = root.path().join("main.py");
    std::fs::write(&main, "from provider import forward\nforward(('ok',))\n").unwrap();
    check_file(&main, Some(root.path())).unwrap();
    std::fs::write(&main, "from provider import forward\nforward(42)\n").unwrap();
    let error = check_file(&main, Some(root.path())).unwrap_err();
    assert!(error.contains("main.py:2:9"), "{error}");
    assert!(error.contains("Expected `Sequence[str] | None`"), "{error}");
}

#[test]
fn an_incoming_union_keeps_its_written_argument_contract() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.py");
    let header = "from collections.abc import Sequence\n\
                  def forward(value: Sequence[str] | int | None) -> None:\n    print(value)\n";
    std::fs::write(&main, format!("{header}forward(('ok',))\n")).unwrap();
    check_file(&main, Some(root.path())).unwrap();
    std::fs::write(&main, format!("{header}forward(1.5)\n")).unwrap();
    let error = check_file(&main, Some(root.path())).unwrap_err();
    assert!(error.contains("main.py:4:9"), "{error}");
    assert!(
        error.contains("Expected `Sequence[str] | int | None`"),
        "{error}"
    );
}

#[test]
fn a_bottom_return_keeps_the_python_call_and_the_written_contract() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.py");
    let child = root.path().join("child.py");
    std::fs::write(
        &main,
        "from child import stop\ndef consume(value: int) -> int: return value + 1\nprint(consume(stop()))\n",
    )
    .unwrap();
    let header = "import sys\nfrom typing import Never as End\ndef stop() -> End:\n";
    std::fs::write(&child, format!("{header}    return sys.exit(7)\n")).unwrap();
    check_file(&main, None).unwrap();
    compile_file(&main, None, &CompileOptions::default()).unwrap();

    for (body, location) in [("return 42", "4:12"), ("pass", "3:15")] {
        std::fs::write(&child, format!("{header}    {body}\n")).unwrap();
        let error = check_file(&main, None).unwrap_err();
        assert!(error.contains(&format!("child.py:{location}")), "{error}");
        assert!(error.contains("Never"), "{error}");
    }
}
