#[test]
fn checks_pathlib_types_imports() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("main.py");
    for program in [
        "import pathlib.types\nprint(pathlib.types.PathInfo)",
        "from pathlib.types import PathInfo\nprint(PathInfo)",
        "from pathlib import types\nprint(types.PathInfo)",
    ] {
        std::fs::write(&source, program).unwrap();
        imre_compiler::check_file(&source, Some(root.path())).unwrap();
    }
}

#[test]
fn importing_pathlib_does_not_parse_unused_project_children() {
    let root = tempfile::tempdir().unwrap();
    let namespace = root.path().join("pathlib");
    std::fs::create_dir(&namespace).unwrap();
    std::fs::write(namespace.join("types.py"), "if !invalid: pass").unwrap();
    let source = root.path().join("main.py");
    std::fs::write(
        &source,
        "from sys import modules\nimport pathlib\nassert 'pathlib.types' not in modules",
    )
    .unwrap();
    imre_compiler::check_file(&source, Some(root.path())).unwrap();
}
