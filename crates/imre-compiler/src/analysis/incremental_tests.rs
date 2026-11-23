use std::fs;
use std::path::Path;

use super::{AnalysisCache, FILE_CHECKS};
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
fn only_the_placed_method_owns_its_local_diagnostic() {
    use super::RegisteredSourceError;
    use ruff_db::diagnostic::{Annotation, Diagnostic, DiagnosticId, LintName, Severity, Span};
    use ruff_text_size::{TextRange, TextSize};

    let graph = crate::modules::SourceGraph::single("x.group()").unwrap();
    let file = graph.session.files[graph.entry];
    let member = TextRange::up_to(TextSize::of("x.group"));
    let definition = TextRange::up_to(TextSize::of("x.group()"));
    let rule = DiagnosticId::Lint(LintName::of("unresolved-attribute"));
    let error = |id, span: Option<Span>| {
        let mut error = Diagnostic::new(id, Severity::Error, "test");
        if let Some(span) = span {
            error.annotate(Annotation::primary(span));
        }
        error
    };
    let local = || Some(Span::from(file).with_range(member));
    let covered = |errors: Vec<Diagnostic>| {
        let errors = errors
            .iter()
            .map(|error| RegisteredSourceError::new(file, error))
            .collect::<Vec<_>>();
        RegisteredSourceError::only_attributes(&errors, definition, |range| range == member)
    };
    assert!(covered(vec![error(rule, local())]));
    assert!(!covered(Vec::new()));
    for other in [
        error(rule, None),
        error(rule, Some(Span::from(file))),
        error(
            rule,
            Some(Span::from(graph.session.execution_names).with_range(member)),
        ),
        error(rule, Some(Span::from(file).with_range(definition))),
        error(DiagnosticId::InvalidSyntax, local()),
    ] {
        assert!(!covered(vec![error(rule, local()), other]));
    }
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
    let mut cache = AnalysisCache::default();
    // Exercise ty in isolation: ordinary Imre lowering also rejects this bad keyword.
    let mut ty_check = || {
        let mut graph = crate::modules::SourceGraph::load(&entry, None, &[]).unwrap();
        let fresh = graph.session.check_scopes(None, &[]);
        let (incremental, ran) = with_checked_files(|| {
            let owner = cache.fixed_session(&mut graph.session, &graph.inputs)?;
            owner.fixed_errors(0..owner.files.len(), None, &[])
        });
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

#[test]
fn scalar_hints_follow_edited_asts_and_are_retained_only_on_demand() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let dep = project.path().join("dep.py");
    fs::write(
        &dep,
        "def number() -> int:\n    return 7\ndef string() -> str:\n    return 'x'\n",
    )
    .unwrap();
    let simple = "from dep import number as get\nprint(get())\n";
    let collect = "from dep import number as get\ndef show() -> None:\n    items = []\n    items.append(get())\n    print(items[0])\nshow()\n";
    let mut compiler = IncrementalCompiler::default();
    let options = CompileOptions::default();
    let mut run = |source: &str, hints| {
        fs::write(&entry, source).unwrap();
        let retained = compiler
            .compile_file_units(&entry, None, &options)
            .map(llvm);
        assert_eq!(
            retained,
            compile_file_units(&entry, None, &options).map(llvm)
        );
        assert_eq!(compiler.cache.hints.is_some(), hints);
        retained
    };
    let early_error = "from dep import number as get\nprint(get(1))\n";
    assert!(run(early_error, false).is_err());
    run(simple, false).unwrap();
    let ints = run(collect, true).unwrap();
    assert!(run(early_error, true).is_err());
    assert_eq!(run(collect, true).unwrap(), ints);
    // Same-length replacement changes which imported return type the append sees.
    let strings = collect.replace("number", "string");
    let words = run(&strings, true).unwrap();
    assert!(ints != words);
    // Move the expression and change its parser node index as well as its source range.
    run(
        &strings.replace("    items = []", "    count = 1\n    items = []"),
        true,
    )
    .unwrap();
    run(simple, false).unwrap();
}

#[test]
fn quiet_never_checks_follow_an_imported_enum_and_failed_revisions() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let dep = project.path().join("dep.py");
    let two = "from enum import StrEnum\nclass Choice(StrEnum):\n    FIRST = 'first'\n    LAST = 'last'\n";
    let three = format!("{two}    EXTRA = 'extra'\n");
    let caller = "from typing import assert_never\nfrom dep import Choice\ndef show(x: Choice) -> None:\n    if x == Choice.FIRST:\n        print(1)\n    elif x == Choice.LAST:\n        print(2)\n    else:\n        assert_never(x)\nshow(Choice.FIRST)\n";
    fs::write(&entry, caller).unwrap();
    let mut compiler = IncrementalCompiler::default();
    let mut run = || {
        let result = compiler.check_file(&entry, None);
        assert_eq!(result, check_file(&entry, None));
        result
    };
    fs::write(&dep, two).unwrap();
    run().unwrap();
    fs::write(&dep, three.as_bytes()).unwrap();
    assert!(run().unwrap_err().contains("exhausted StrEnum"));
    // A failed fresh preflight must not replace the last valid source snapshot.
    fs::write(&dep, "bad\0source").unwrap();
    assert!(run().unwrap_err().contains("null bytes"));
    fs::write(&dep, two).unwrap();
    run().unwrap();
    // The existing dep.py is shadowed by an entirely new module identity.
    let package = project.path().join("dep");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("__init__.py"), &three).unwrap();
    assert!(run().unwrap_err().contains("exhausted StrEnum"));
    fs::write(package.join("__init__.py"), two).unwrap();
    run().unwrap();
}

#[test]
fn a_saved_initializer_call_is_rechecked_after_rebinding() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let source = "from argparse import ArgumentParser\n\
def stop() -> int: raise SystemExit(13)\n\
def ready() -> int: return 41\n\
def take(first: int, second: int) -> int: return first + second\n\
parser = ArgumentParser()\n\
saved = TARGET\n\
print(take(saved(), 'wrong'))\n";
    let mut compiler = IncrementalCompiler::default();
    for (target, expected) in [
        ("ready", "expected int, got str"),
        ("stop", "invalid-argument-type"),
        ("ready", "expected int, got str"),
    ] {
        fs::write(&entry, source.replace("TARGET", target)).unwrap();
        let error = compiler.check_file(&entry, None).unwrap_err();
        assert_eq!(check_file(&entry, None).unwrap_err(), error);
        assert!(
            error.contains(":7:21") && error.contains(expected),
            "{error}"
        );
    }
}

#[test]
fn file_and_saved_module_check_defaults_on_every_selected_revision() {
    let project = tempdir().unwrap();
    let entry = project.path().join("app.py");
    let named = CompileOptions {
        module_entry: Some("app".into()),
        require_checked_module_entry: true,
        script_startup: Some((Default::default(), "imre".into())),
        extension_paths: vec![project.path().to_owned()],
        pythonpath: 0..1,
        ..CompileOptions::default()
    };
    for options in [&CompileOptions::default(), &named] {
        let mut cache = IncrementalCompiler::default();
        for (prefix, value) in [
            ("", "1"),
            ("", "True"),
            ("import unittest\n", "True"),
            ("import unittest\n", "1"),
        ] {
            fs::write(
                &entry,
                format!("{prefix}def f(i: int = {value}): return i\nprint(f())\n"),
            )
            .unwrap();
            let result =
                crate::check_file_with(&entry, None, options, Some(&mut cache), Default::default());
            assert_eq!(
                result,
                crate::check_file_with_options(&entry, None, options)
            );
            if value == "True" {
                let error = result.unwrap_err();
                assert!(error.contains("int, got bool"), "{error}");
            } else {
                result.unwrap();
            }
        }
    }
}

#[test]
fn unread_import_effects_cannot_admit_an_uninitialized_local() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let provider = project.path().join("peripheral.py");
    let package = project.path().join("package");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("__init__.py"), "print(bytearray())\n").unwrap();
    fs::write(package.join("leaf.py"), "print(bytearray())\n").unwrap();
    fs::write(
        &provider,
        "value = bytearray(b'initialized')\nprint(value)\n",
    )
    .unwrap();
    let body = concat!(
        "from typing import Final\n",
        "COUNT: Final = 1\n",
        "def select(flag: bool) -> int:\n",
        "    value = (saved := 1) if flag else 0\n",
        "    return saved\n",
    );
    let mut compiler = IncrementalCompiler::default();
    let mut run = |text: &str| {
        fs::write(&entry, text).unwrap();
        let result = compiler.check_file(&entry, None);
        assert_eq!(result, check_file(&entry, None));
        result
    };
    for prefix in [
        "",
        "import unittest as extra\n",
        "import json\n",
        "import peripheral as extra\n",
        "from unittest import TestCase as extra\n",
        "from peripheral import value as extra\n",
        "import package as extra\n",
        "import package.leaf as extra\n",
        "import unittest.mock as extra\n",
    ] {
        let error = run(&format!("{prefix}{body}")).unwrap_err();
        assert!(error.contains("local may be uninitialized"), "{error}");
    }
    fs::write(
        &provider,
        "def make(): return bytearray(b'initialized')\nvalue = make()\n",
    )
    .unwrap();
    let error = run(&format!("from peripheral import value as unused\n{body}")).unwrap_err();
    assert!(error.contains("local may be uninitialized"), "{error}");
    let error = run("from peripheral import value, absent as unused\nprint(1)\n").unwrap_err();
    assert!(
        error.contains("has no static binding \"absent\""),
        "{error}"
    );
    fs::write(
        &provider,
        "import unittest\ndef side(): print(bytearray(b'initialized'))\nside()\n",
    )
    .unwrap();
    let error = run(&format!("import peripheral as extra\n{body}")).unwrap_err();
    assert!(error.contains("local may be uninitialized"), "{error}");
    let repaired = format!(
        "import peripheral as extra\n{}",
        body.replace("return saved", "return value")
    );
    run(&repaired).unwrap();
    fs::write(
        &provider,
        "print(bytearray())\ndef bad(i: int = True): return i\nbad()\n",
    )
    .unwrap();
    let error = run(&repaired).unwrap_err();
    assert!(
        error.contains("peripheral.py:2:18: Imre: default requires exact type int, got bool"),
        "{error}"
    );
    // Another import that reads the same module still sees its original declarations.
    fs::write(
        &provider,
        "def take(value: int) -> int: return value\nprint(bytearray())\n",
    )
    .unwrap();
    let reads = "import peripheral as extra\nfrom peripheral import take as unused\nfrom peripheral import take\nprint(take(3))\n";
    run(reads).unwrap();
    assert!(run(&reads.replace("take(3)", "take(True)")).is_err());
}

#[test]
fn a_named_import_does_not_consume_its_providers_other_imports() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let provider = project.path().join("provider.py");
    let body = concat!(
        "from provider import twice\n",
        "def select(flag: bool) -> int:\n",
        "    if flag: saved = 1\n",
        "    return saved\n",
        "print(twice(2))\n"
    );
    fs::write(&entry, body).unwrap();
    let mut compiler = IncrementalCompiler::default();
    for imports in ["", "import unittest\n"] {
        fs::write(
            &provider,
            format!("{imports}def twice(n: int): return n + n\n"),
        )
        .unwrap();
        let checked = compiler.check_file(&entry, None);
        assert_eq!(checked, check_file(&entry, None));
        let error = checked.unwrap_err();
        assert!(error.contains("local may be uninitialized"), "{error}");
    }
    let qualified = body
        .replace("from provider import twice", "import provider as lib")
        .replace("print(twice(2))", "print(lib.twice(2))");
    fs::write(&entry, &qualified).unwrap();
    assert!(
        compiler
            .check_file(&entry, None)
            .unwrap_err()
            .contains("local may be uninitialized")
    );
    fs::write(&entry, body.replace("return saved", "return 1")).unwrap();
    compiler.check_file(&entry, None).unwrap();
    // A later caller may read the re-export from this same provider.
    fs::write(
        &entry,
        "from provider import twice, unittest\nprint(twice(1), unittest.TestCase)\n",
    )
    .unwrap();
    compiler.check_file(&entry, None).unwrap();
}
