#[test]
fn checks_the_replace_call_when_an_operand_needs_object_lowering() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.py");
    std::fs::write(
        &path,
        "try:\n    print(\"x\".replace(\"x\", (1 == \"q\",)))\nexcept TypeError:\n    pass\n",
    )
    .unwrap();

    let error = imre_compiler::check_file(&path, None).unwrap_err();
    assert!(error.contains("error[no-matching-overload]"), "{error}");
    assert!(error.contains("main.py:2:11"), "{error}");
    imre_compiler::compile_file(&path, None, &Default::default()).unwrap();
}
