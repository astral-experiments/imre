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
