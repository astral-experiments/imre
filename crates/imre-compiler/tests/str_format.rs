#[test]
fn checks_a_named_json_field_and_reports_the_callers_assignment() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("writer.py"),
        r#"import json

def render(data: str) -> str:
    version = None
    for item in json.loads(data):
        version = item["version"]
    return "{a} {b} {version} {d}".format(a="A", b="B", version=version, d="D")
"#,
    )
    .unwrap();
    let entry = root.path().join("main.py");
    let call = r#"print(render('[{"version": "1.2"}]'))"#;
    std::fs::write(&entry, format!("from writer import render\n{call}\n")).unwrap();
    imre_compiler::check_file(&entry, None).unwrap();

    std::fs::write(
        &entry,
        format!("from writer import render\nmodule_wrong: str = 17\n{call}\n"),
    )
    .unwrap();
    let error = imre_compiler::check_file(&entry, None).unwrap_err();
    assert!(error.contains("main.py:2:21"), "{error}");
    assert!(error.contains("expected str, got int"), "{error}");
}
