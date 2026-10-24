use std::fs;
use std::path::Path;

use super::{DiagnosticCache, FILE_CHECKS};
use crate::{CompileOptions, IncrementalCompiler, UnitCompilation, check_file, compile_file_units};
use tempfile::tempdir;

fn checked_files(f: impl FnOnce()) -> Vec<String> {
    FILE_CHECKS.with_borrow_mut(|checks| *checks = Some(Vec::new()));
    f();
    FILE_CHECKS.with_borrow_mut(|checks| {
        checks
            .take()
            .unwrap()
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    })
}

fn llvm(result: UnitCompilation) -> Vec<(String, String)> {
    result
        .units
        .into_iter()
        .map(|unit| (unit.name, unit.llvm_ir))
        .collect()
}

#[test]
fn body_and_unrelated_export_reuse_importer_diagnostics() {
    let project = tempdir().unwrap();
    let dep = project.path().join("dep.py");
    let entry = project.path().join("main.py");
    fs::write(&entry, "from dep import score\nprint(score())\n").unwrap();
    let mut compiler = IncrementalCompiler::default();
    let options = CompileOptions::default();
    let mut previous = None;
    for (source, checked) in [
        (
            "def score(base: int = 1) -> int:\n    return base + 1\n",
            &["main.py", "dep.py"][..],
        ),
        (
            "def score(base: int = 1) -> int:\n    return base + 2\n",
            &["dep.py"][..],
        ),
        (
            "def score(base: int = 1) -> int:\n    return base + 2\ndef unrelated() -> int:\n    return 3\n",
            &["dep.py"][..],
        ),
        (
            "def score(base: int = 2) -> int:\n    return base + 2\ndef unrelated() -> int:\n    return 3\n",
            &["dep.py"][..],
        ),
    ] {
        fs::write(&dep, source).unwrap();
        let mut current = None;
        assert_eq!(
            checked_files(|| {
                current = Some(llvm(
                    compiler.compile_file_units(&entry, None, &options).unwrap(),
                ));
            }),
            checked
        );
        let current = current.unwrap();
        assert_eq!(
            current,
            llvm(compile_file_units(&entry, None, &options).unwrap())
        );
        if let Some(previous) = previous {
            assert_ne!(
                current, previous,
                "the changed value must reach code generation"
            );
        }
        previous = Some(current);
    }
    assert!(checked_files(|| compiler.check_file(&entry, None).unwrap()).is_empty());
}

#[test]
fn ty_rechecks_an_importer_when_a_used_parameter_is_renamed() {
    let project = tempdir().unwrap();
    let dep = project.path().join("dep.py");
    let entry = project.path().join("main.py");
    fs::write(&entry, "from dep import score\nprint(score(base=1))\n").unwrap();
    fs::write(&dep, "def score(base: int) -> int:\n    return base\n").unwrap();
    let mut cache = DiagnosticCache::default();
    // Exercise ty in isolation: ordinary Relar lowering also rejects this bad keyword.
    let mut ty_check = || {
        let mut graph = crate::modules::SourceGraph::load(&entry, None).unwrap();
        let fresh = graph.session.check();
        let incremental = cache.check(&mut graph.session, &graph.inputs);
        assert_eq!(incremental, fresh);
        incremental
    };
    ty_check().unwrap();
    fs::write(&dep, "def score(other: int) -> int:\n    return other\n").unwrap();
    let error = ty_check().unwrap_err();
    assert!(error.contains("base"), "{error}");
    fs::write(&dep, "def score(base: int) -> int:\n    return base\n").unwrap();
    ty_check().unwrap();
}

#[test]
fn reachable_invalid_edits_and_import_topology_match_clean_diagnostics() {
    let project = tempdir().unwrap();
    let dep = project.path().join("dep.py");
    let entry = project.path().join("main.py");
    let good = "def score() -> int:\n    return 1\n";
    let caller = "from dep import score\nprint(score())\n";
    fs::write(&entry, caller).unwrap();
    fs::write(&dep, good).unwrap();
    let mut compiler = IncrementalCompiler::default();
    let mut compare = || {
        let incremental = compiler.check_file(&entry, None);
        assert_eq!(incremental, check_file(&entry, None));
        incremental
    };
    compare().unwrap();
    for invalid in [
        // A formerly valid positive lookup now misses.
        "def renamed() -> int:\n    return 1\n",
        // The caller never uses this invalid definition.
        "def score() -> int:\n    return 1\ndef unused() -> int:\n    return 'bad'\n",
        "from helper import other\ndef score() -> int:\n    return other()\n",
    ] {
        fs::write(&dep, invalid).unwrap();
        assert!(compare().is_err());
    }
    let helper = project.path().join("helper.py");
    fs::write(
        &helper,
        "from dep import score\ndef other() -> int:\n    return score()\n",
    )
    .unwrap();
    assert!(compare().is_err());
    fs::write(&helper, "def other() -> int:\n    return 4\n").unwrap();
    compare().unwrap();
    // The loader searches for a package before a module. A previously absent package can
    // change which definition the same import resolves to.
    let pkg = project.path().join("helper");
    fs::create_dir(&pkg).unwrap();
    fs::write(
        pkg.join("__init__.py"),
        "def missing() -> int:\n    return 4\n",
    )
    .unwrap();
    assert!(compare().is_err());
    fs::write(
        pkg.join("__init__.py"),
        "def other() -> int:\n    return 5\n",
    )
    .unwrap();
    compare().unwrap();
}
