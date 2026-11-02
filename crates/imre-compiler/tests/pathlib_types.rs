#[test]
fn checks_pathlib_child_imports() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("main.py");
    for program in [
        "import pathlib._local\nprint(pathlib._local.PurePath)",
        "from pathlib._local import PurePath\nprint(PurePath)",
        "from pathlib import _local\nprint(_local.PurePath)",
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
    for child in ["_local.py", "types.py"] {
        std::fs::write(namespace.join(child), "if !invalid: pass").unwrap();
    }
    let source = root.path().join("main.py");
    std::fs::write(
        &source,
        "from sys import modules\nimport pathlib\nassert 'pathlib.types' not in modules",
    )
    .unwrap();
    imre_compiler::check_file(&source, Some(root.path())).unwrap();
}
