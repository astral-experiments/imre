use std::fs;
use std::path::Path;

use super::{DiagnosticCache, FILE_CHECKS};
use crate::{CompileOptions, IncrementalCompiler, UnitCompilation, check_file, compile_file_units};
use tempfile::tempdir;

fn with_checked_files<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    FILE_CHECKS.with_borrow_mut(|checks| *checks = Some(Vec::new()));
    let result = f();
    let files = FILE_CHECKS.with_borrow_mut(|checks| {
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
    });
    (result, files)
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
    let mut previous: Option<Vec<(String, String)>> = None;
    for (index, (source, checked)) in [
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
        (
            "def score(base: int = 2) -> int:\n    return 6 // base\ndef unrelated() -> int:\n    return 3\n",
            &["dep.py"][..],
        ),
    ].into_iter().enumerate() {
        fs::write(&dep, source).unwrap();
        let (current, ran) = with_checked_files(|| {
            llvm(compiler.compile_file_units(&entry, None, &options).unwrap())
        });
        assert_eq!(ran, checked);
        assert_eq!(
            current,
            llvm(compile_file_units(&entry, None, &options).unwrap())
        );
        if let Some(previous) = previous {
            assert_eq!(current[0].0, "main");
            if index == 1 {
                assert!(current[0] == previous[0], "a body-only edit kept the caller's LLVM");
            } else if index >= 3 {
                // An inlined default value, then a newly throwing callee, each affects its caller.
                assert!(current[0] != previous[0], "the caller's LLVM must change");
            }
        }
        previous = Some(current);
    }
    let (result, ran) = with_checked_files(|| compiler.check_file(&entry, None));
    result.unwrap();
    assert!(ran.is_empty());
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
        let (incremental, ran) =
            with_checked_files(|| cache.check(&mut graph.session, &graph.inputs));
        assert_eq!(incremental, fresh);
        // The first request, the rename that makes the call invalid, and the repair all
        // execute the importer's diagnostic query as well as the provider's.
        assert_eq!(ran, ["main.py", "dep.py"]);
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
    fs::write(
        &helper,
        "def other() -> int:\n    return 4\ndef bad() -> int:\n    return 'bad'\n",
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
