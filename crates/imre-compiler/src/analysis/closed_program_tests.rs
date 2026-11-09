use super::AnalysisCache;
use crate::dynamic::RegisteredSource;
use std::fs;
use tempfile::tempdir;

#[test]
fn registered_body_checks_its_declared_result() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let suspect = project.path().join("suspect.py");
    fs::write(&entry, "from ast import literal_eval\ntry:\n    import suspect\nexcept ImportError:\n    pass\nprint(literal_eval('27'))\n").unwrap();
    fs::write(&suspect, "def dormant() -> int:\n    return 'wrong'\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("suspect.py:2:12"), "{error}");
    assert!(error.contains("invalid-return-type"), "{error}");

    fs::write(&suspect, "def dormant() -> str:\n    return 'wrong'\n").unwrap();
    crate::check_file(&entry, None).unwrap();
}

#[test]
fn closed_program_keeps_first_party_contracts_across_cached_edits() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let scores = project.path().join("scores.py");
    fs::write(
        &entry,
        "import scores\nif False: print(t'x')\nprint(scores.measure())\n",
    )
    .unwrap();
    let mut cache = AnalysisCache::default();
    let mut check = || {
        let mut graph = crate::modules::SourceGraph::load(&entry, None, &[]).unwrap();
        let lowered = crate::frontend::lower_with_owners(&mut graph);
        let (skipped, proofs) = graph.diagnostic_scopes();
        let native = lowered.as_ref().err();
        let result = cache.check_closed_program(
            &mut graph.session,
            &graph.inputs,
            &graph.modules,
            skipped.as_deref(),
            &proofs,
            native,
        );
        let fresh =
            graph
                .session
                .check_closed_program(&graph.modules, skipped.as_deref(), &proofs, native);
        assert_eq!(result, fresh);
        result
    };
    let source = "def take(value: int) -> int:\n    return value\n\ndef measure() -> int:\n    return take(1)\n";
    fs::write(&scores, source).unwrap();
    check().unwrap();
    fs::write(&scores, source.replace("take(1)", "take('x')")).unwrap();
    let error = check().unwrap_err();
    assert!(error.contains("scores.py:5:17"), "{error}");
    fs::write(&scores, source).unwrap();
    check().unwrap();
}

#[test]
fn registered_parameter_changes_recheck_the_unchanged_caller() {
    let project = tempdir().unwrap();
    let root = project.path();
    let entry = root.join("main.py");
    let api = root.join("api.py");
    let caller = "from api import value\n\ndef score() -> int:\n    return value(1)\n";
    let number = "def value(n: int) -> int:\n    return 1\n";
    let text = "def value(n: str) -> int:\n    return 1\n";
    fs::write(&entry, caller).unwrap();
    fs::write(&api, number).unwrap();
    let mut graph = crate::modules::SourceGraph::load(&entry, None, &[]).unwrap();
    crate::checked_lower(&mut graph).unwrap();
    let mut registered = vec![
        ("__main__".into(), entry.clone(), caller.into()),
        ("api".into(), api.clone(), number.into()),
    ];
    let verify = |sources: &[_]| {
        super::check_registered_project(
            root,
            &[],
            sources.iter().map(|s| RegisteredSource::new(s, true, None)),
            None,
            Some(&graph),
        )
    };
    fs::remove_file(entry).unwrap();
    fs::remove_file(&api).unwrap();
    verify(&registered).unwrap();
    // Same path, stale snapshot digest and the original caller. The provider is
    // internally well typed, so only checking them together finds the error.
    registered[1].2 = text.into();
    let error = verify(&registered).unwrap_err();
    assert!(
        error.contains("main.py:4") && error.contains("invalid-argument-type"),
        "{error}"
    );

    registered[1].1 = root.join("shadow/api.py");
    let shadow_root = root.join("shadow");
    let error = super::check_registered_project(
        root,
        &[shadow_root],
        registered
            .iter()
            .enumerate()
            .map(|(i, s)| RegisteredSource::new(s, i == 0, None)),
        None,
        Some(&graph),
    )
    .unwrap_err();
    assert!(
        error.contains("main.py:4") && error.contains("invalid-argument-type"),
        "{error}"
    );
}

#[test]
fn new_registration_checks_dormant_bodies_against_registered_declarations() {
    let project = tempdir().unwrap();
    let root = project.path();
    let code = "from api import take\n\ndef unused() -> int:\n    return take('x')\n";
    let mut registrations = [
        (
            "api".into(),
            root.join("api.pyi"),
            "def take(n: int) -> int: ...\n".into(),
        ),
        ("new".into(), root.join("new.py"), code.into()),
    ];
    let error = super::check_registered_project(
        root,
        &[],
        registrations
            .iter()
            .enumerate()
            .map(|(i, s)| RegisteredSource::new(s, i == 1, None)),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("new.py:4") && error.contains("invalid-argument-type"),
        "{error}"
    );
    registrations[0].2 = "def take(n: str) -> int: ...\n".into();
    super::check_registered_project(
        root,
        &[],
        registrations
            .iter()
            .enumerate()
            .map(|(i, s)| RegisteredSource::new(s, i == 1, None)),
        None,
        None,
    )
    .unwrap();
}

#[test]
fn registered_python_shadow_provides_the_call_signature() {
    let project = tempdir().unwrap();
    let root = project.path();
    let extension = root.join("providers");
    let mut sources = vec![
        (
            "subprocess".into(),
            extension.join("subprocess.py"),
            "def run(n: int) -> int:\n    return n\n".into(),
        ),
        (
            "main".into(),
            root.join("main.py"),
            "import subprocess\n\ndef dormant() -> None:\n    subprocess.run(1)\n".into(),
        ),
    ];
    let verify = |sources: &[_]| {
        super::check_registered_project(
            root,
            std::slice::from_ref(&extension),
            sources
                .iter()
                .enumerate()
                .map(|(i, s)| RegisteredSource::new(s, i == 1, None)),
            None,
            None,
        )
    };
    verify(&sources).unwrap();
    sources[1].2 = "import subprocess\n\ndef dormant() -> None:\n    subprocess.run('x')\n".into();
    let error = verify(&sources).unwrap_err();
    assert!(
        error.contains("main.py:4") && error.contains("invalid-argument-type"),
        "{error}"
    );
}

#[test]
fn resolved_bundled_source_keeps_its_declaration() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    fs::write(&main, "from http import HTTPStatus\n\ndef label(value: HTTPStatus) -> str:\n    return 'ok'\n\nprint(label(HTTPStatus.OK))\n").unwrap();
    crate::check_file(&main, None).unwrap();
}

#[test]
fn functions_selected_from_a_mapping_keep_python_argument_binding() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        &entry,
        r#"import sys

def plus(value):
    return value + 1

def twice(value):
    return value * 2

operations = {"a": plus, "b": twice}
class Hooks:
    pass
setattr(Hooks, sys.argv[1], staticmethod(operations[sys.argv[2]]))
print(getattr(Hooks, sys.argv[1])(7))
"#,
    )
    .unwrap();
    crate::check_file(&entry, None).unwrap();
}

#[test]
fn computed_factory_preserves_dormant_declared_calls() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("main.py");
    let scores = tmp.path().join("scores.py");
    fs::write(&source, "import scores\nfrom dataclasses import dataclass, field\ndef one(): return 1\nf = {0: one}[0]\n@dataclass\nclass Result:\n    n: int = field(default_factory=f)\nprint(Result().n)\n").unwrap();
    fs::write(
        &scores,
        "def take(n: int) -> int: return n\nif False:\n    take('x')\n",
    )
    .unwrap();
    let error = crate::check_file(&source, None).unwrap_err();
    assert!(
        error.contains("scores.py:3:10") && error.contains("expected int, got str"),
        "{error}"
    );
    fs::write(
        &scores,
        "def take(n: int) -> int: return n\nif False:\n    take(1)\n",
    )
    .unwrap();
    crate::check_file(&source, None).unwrap();
    fs::write(&scores, "from typing import TypeVar\nT = TypeVar('T')\ndef take(n: T) -> T: return n\nif False:\n    take('x')\n").unwrap();
    crate::check_file(&source, None).unwrap();
}

#[test]
fn declared_import_checks_lexical_syntax() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        &entry,
        "import importlib, sys\nimportlib.import_module(sys.argv[1])\n",
    )
    .unwrap();
    fs::write(
        project.path().join("plugin.py"),
        "def f():\n    nonlocal missing\n",
    )
    .unwrap();
    let options = crate::CompileOptions {
        include_modules: ["plugin".to_owned()].into(),
        ..Default::default()
    };
    let error = crate::check_file_with_options(&entry, None, &options).unwrap_err();
    assert!(
        error.contains("invalid-syntax") && error.contains("plugin.py:2:5"),
        "{error}"
    );
}

#[test]
fn literal_import_checks_native_body_with_or_without_other_declarations() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        &entry,
        "import importlib, sys\nimport badsymbol\nimportlib.import_module(sys.argv[1])\n",
    )
    .unwrap();
    fs::write(
        project.path().join("badsymbol.py"),
        "def f():\n    nonlocal missing\n",
    )
    .unwrap();
    fs::write(project.path().join("innocent.py"), "value = 1\n").unwrap();
    for declared in [None, Some("innocent"), Some("badsymbol")] {
        let options = crate::CompileOptions {
            module_entry: Some("main".into()),
            include_modules: declared.into_iter().map(str::to_owned).collect(),
            ..Default::default()
        };
        let error = crate::compile_file_with_boxed_units(&entry, Some(project.path()), &options)
            .err()
            .unwrap();
        assert!(
            error.contains("badsymbol.py:2:5") && error.contains("invalid-syntax"),
            "{error}"
        );
    }
}

#[test]
fn contextual_errors_do_not_hide_the_winning_files_semantic_error() {
    let project = tempdir().unwrap();
    let entry = project.path().join("billing.py");
    fs::write(
        &entry,
        "import sys\nfrom importlib.machinery import SourceFileLoader\n\ndef declared():\n    import pricing\n    return pricing\n\nSourceFileLoader('pricing', sys.argv[1]).get_code('pricing')\n",
    )
    .unwrap();
    let file = project.path().join("pricing.py");
    for operand in ["__debug__", "(label, [__debug__])"] {
        fs::write(&file, format!("print('invoice')\ndel {operand}\n")).unwrap();
        let error = crate::compile_file_with_boxed_units(
            &entry,
            Some(project.path()),
            &crate::CompileOptions::default(),
        )
        .err()
        .unwrap();
        assert!(
            error.contains("invalid-syntax") && error.contains("pricing.py:2:"),
            "{error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn retained_native_candidate_registers_its_original_source_owner() {
    use std::os::unix::fs::symlink;
    let project = tempdir().unwrap();
    let root = project.path();
    let entry = root.join("main.py");
    let helper = root.join("helper.py");
    let before = root.join("before.py");
    let after = root.join("after.py");
    let original = "def value() -> int: return 1\n";
    let replacement = "def value() -> str: return 'other'\n";
    fs::write(&before, original).unwrap();
    fs::write(&after, replacement).unwrap();
    symlink(&before, &helper).unwrap();
    let vendor = root.join("vendor");
    let physical = root.join("vendor-A");
    fs::create_dir(&physical).unwrap();
    symlink(&physical, &vendor).unwrap();
    let external = physical.join("external.py");
    fs::write(&external, original).unwrap();
    fs::write(
        &entry,
        "import helper, external\nprint(helper.value(), external.value())\n",
    )
    .unwrap();
    let options = crate::CompileOptions {
        extension_paths: vec![vendor.clone()],
        ..Default::default()
    };
    let load =
        || crate::modules::SourceGraph::load(&entry, None, &options.extension_paths).unwrap();
    let mut graph = load();
    crate::checked_lower(&mut graph).unwrap();
    let helper_id = graph.python_file("helper").unwrap();
    let external_id = graph.python_file("external").unwrap();
    fs::remove_file(&helper).unwrap();
    symlink(&after, &helper).unwrap();
    fs::write(&external, replacement).unwrap();
    assert!(!graph.inputs.fresh());
    let compilation = crate::dynamic::compile(
        (&entry, None, Default::default()),
        &options,
        true,
        Some(crate::dynamic::ObjectOperation::CheckedEntry),
        Some(&graph),
        crate::BoxedLayout::Check,
        |_, sources| {
            let sources = sources.unwrap();
            for (name, id) in [("helper", helper_id), ("external", external_id)] {
                let donor = sources.winners.iter().find(|s| s.name == name).unwrap();
                assert_eq!(donor.checked_module, Some(id));
                assert_eq!(donor.source, original);
            }
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    assert!(compilation.source_inputs.files.contains(&(
        vendor.join("external.py"),
        external,
        blake3::hash(original.as_bytes()).to_hex().to_string(),
    )));
    let next = load();
    for name in ["helper", "external"] {
        let id = next.python_file(name).unwrap();
        assert_eq!(next.modules[id].original_source(), replacement);
    }
    let id = next.python_file("helper").unwrap();
    assert_eq!(next.inputs.files[id].1, after);

    let package = root.join("helper");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("__init__.py"), replacement).unwrap();
    let error = crate::dynamic::compile(
        (&entry, None, Default::default()),
        &options,
        true,
        Some(crate::dynamic::ObjectOperation::CheckedEntry),
        Some(&graph),
        crate::BoxedLayout::Check,
        |_, _| panic!("a different import kind reached object admission"),
    )
    .err()
    .unwrap();
    assert!(
        error.contains("package initializer appeared during compilation"),
        "{error}"
    );
}

#[test]
fn run_path_checks_the_bound_filename_and_linked_body() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let report = project.path().join("report.py");
    fs::write(&report, "print(__name__)\n").unwrap();
    let options = crate::CompileOptions {
        include_modules: ["report".to_owned()].into(),
        ..Default::default()
    };
    let check = |source| {
        fs::write(&entry, source).unwrap();
        crate::check_file_with_options(&entry, None, &options)
    };
    let prefix = "from runpy import run_path as run\nimport sys\n";
    check(format!(
        "{prefix}run(run_name='report', path_name={:?})",
        report.to_str().unwrap()
    ))
    .unwrap();
    let error = check(format!(
        "{prefix}run(run_name='report', path_name=sys.argv[1])"
    ))
    .unwrap_err();
    assert!(
        error.contains("main.py:3:34") && error.contains("handlers[key]()"),
        "{error}"
    );
    check(format!("{prefix}saved = run\nprint(saved.__name__)")).unwrap();
    check(
        "import runpy, sys\nprint(sys.modules['runpy'].__name__, runpy.run_path.__name__)"
            .to_string(),
    )
    .unwrap();
    let error = check(format!("{prefix}saved = run\nsaved(sys.argv[1])")).unwrap_err();
    assert!(
        error.contains("main.py:4:7") && error.contains("open executable"),
        "{error}"
    );
}

#[test]
fn run_path_selected_from_mixed_bindings_requires_a_rewrite() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(&entry, "from runpy import run_path as go\nimport sys\ndef other(path): print(path)\nif sys.argv[1]: go = other\ngo(sys.argv[2])\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("main.py:5:1") && error.contains("selected callable"),
        "{error}"
    );
}

#[test]
fn external_caller_checks_the_execution_operand() {
    let project = tempdir().unwrap();
    let other = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(&entry, "import invoice\ninvoice.run()\n").unwrap();
    fs::write(
        other.path().join("invoice.py"),
        "from runpy import run_path\nimport sys\ndef run(): run_path(sys.argv[1])\n",
    )
    .unwrap();
    let options = crate::CompileOptions {
        module_entry: Some("invoice".to_owned()),
        extension_paths: vec![other.path().to_owned()],
        ..Default::default()
    };
    let error = crate::check_file_with_options(&entry, None, &options).unwrap_err();
    assert!(
        error.contains("invoice.py:3:21") && error.contains("open executable filename"),
        "{error}"
    );
}

#[test]
fn a_script_named_runpy_does_not_own_the_imported_sdk_function() {
    let project = tempdir().unwrap();
    let root = project.path();
    let sources = [
        (
            "__main__".into(),
            root.join("runpy.py"),
            "def run_path(path): return path\nrun_path(input())\n".into(),
        ),
        (
            "worker".into(),
            root.join("worker.py"),
            "from runpy import run_path\nimport sys\nrun_path(sys.argv[1])\n".into(),
        ),
    ];
    let error = super::check_registered_project(
        root,
        &[],
        sources.iter().map(|s| RegisteredSource::new(s, true, None)),
        Some(&|_| false),
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("worker.py:3:10") && error.contains("open executable filename"),
        "{error}"
    );
}

#[test]
fn registered_program_reports_both_execution_boundaries() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        &entry,
        "import runpy, sys\noperations = {'run': exec}\noperations['run'](input())\nrunpy.run_path(sys.argv[1])\n",
    ).unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("builtins.exec")
            && error.contains("main.py:4:16")
            && error.contains("open executable filename"),
        "{error}"
    );
}

#[test]
fn native_lowering_keeps_the_unregistered_eval_operand_diagnostic() {
    let project = tempdir().unwrap();
    let entry = project.path().join("job.py");
    fs::write(
        &entry,
        "import subprocess\nimport sys\ndef run(command):\n    return subprocess.run(command, capture_output=True, text=True, check=True).stdout\nprint(eval(run(['/usr/bin/printf', '%s', sys.argv[1]])))\n",
    ).unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("job.py:5:12"), "{error}");
    assert!(error.contains("builtins.eval"), "{error}");
    assert!(error.contains("select one by name"), "{error}");
}
