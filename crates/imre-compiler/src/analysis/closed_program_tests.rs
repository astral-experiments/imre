use super::AnalysisCache;
use crate::dynamic::RegisteredSource;
use std::fs;
use tempfile::tempdir;

#[test]
fn closed_checker_preserves_exact_reachable_return_contracts() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    // Exact 89-byte historical fixture (SHA256 af6ffed6682117e309d33a8c56f9736bd963428cfc74bf5d7bfe4896df7ff74e).
    let original = "buffer = bytearray()\nbuffer.append(97)\nprint(buffer)\ndef f() -> int:\n    return True\nf()\n";
    fs::write(&entry, original).unwrap();
    for error in [
        crate::check_file(&entry, None).unwrap_err(),
        crate::compile_file(&entry, None, &Default::default())
            .err()
            .unwrap(),
    ] {
        assert!(
            error.contains("main.py:5:12: Imre: expected int, got bool"),
            "{error}"
        );
    }

    let dynamic = "print(bytearray())\n";
    for (annotation, value, expected) in [
        ("int", "True", "expected int, got bool"),
        ("float", "1", "expected float, got int"),
    ] {
        fs::write(
            &entry,
            format!("{dynamic}def f() -> {annotation}:\n    return {value}\nf()\n"),
        )
        .unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    for (annotation, value) in [("int", "42"), ("int | bool", "True"), ("float", "1.0")] {
        fs::write(
            &entry,
            format!("{dynamic}def f() -> {annotation}:\n    return {value}\nf()\n"),
        )
        .unwrap();
        crate::check_file(&entry, None).unwrap();
    }
    fs::write(
        &entry,
        format!("{dynamic}def f() -> int:\n    return 'x'\nf()\n"),
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("invalid-return-type"), "{error}");
    assert!(!error.contains("Imre: expected int, got str"), "{error}");
}

#[test]
fn closed_return_contracts_cover_imports_and_preserve_dormant_policy() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let provider = project.path().join("provider.py");
    fs::write(project.path().join("dynamic.py"), "print(bytearray())\n").unwrap();
    fs::write(&entry, "import dynamic\nimport provider\nprovider.f()\n").unwrap();
    fs::write(&provider, "def f() -> int:\n    return True\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("provider.py:2:12: Imre: expected int, got bool"),
        "{error}"
    );

    fs::write(
        project.path().join("contracts.py"),
        "type Choice = int | bool\ntype Count = int\n",
    )
    .unwrap();
    fs::write(
        &provider,
        "from contracts import Choice\ndef f() -> Choice:\n    return True\n",
    )
    .unwrap();
    crate::check_file(&entry, None).unwrap();
    fs::write(
        &provider,
        "from contracts import Count\ndef f() -> Count:\n    return True\n",
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("provider.py:3:12: Imre: expected int, got bool"),
        "{error}"
    );

    fs::write(
        &provider,
        "def f() -> int:\n    if False:\n        return True\n    return 1\n",
    )
    .unwrap();
    crate::check_file(&entry, None).unwrap();
    fs::write(
        &provider,
        "def f() -> int:\n    if False:\n        return 'x'\n    return 1\n",
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("provider.py:3:16: Imre: expected int, got str"),
        "{error}"
    );
}

#[test]
fn closed_return_waits_for_its_operand_and_respects_operand_masks() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    for (value, inner_error) in [
        ("True", "expected int, got bool"),
        ("'x'", "invalid-argument-type"),
    ] {
        fs::write(&entry, format!("print(bytearray())\ndef take(value: int) -> bool:\n    return True\ndef f() -> int:\n    return take({value})\nf()\n")).unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(error.contains(inner_error), "{error}");
        assert!(
            !error.contains("main.py:5:12: Imre: expected int, got bool"),
            "{error}"
        );
    }
    fs::write(&entry, "print(bytearray())\ndef boom() -> bool:\n    raise RuntimeError()\ndef f() -> int:\n    return boom()\nf()\n").unwrap();
    crate::check_file(&entry, None).unwrap();

    let source = "print(bytearray())\ndef f() -> int:\n    return True\nf()\n";
    fs::write(&entry, source).unwrap();
    let mut graph = crate::modules::SourceGraph::load(&entry, None, &[]).unwrap();
    let (_, proofs) = graph.diagnostic_scopes();
    let mut skipped = vec![Vec::new(); graph.modules.len()];
    let start = ruff_text_size::TextSize::new(source.find("True").unwrap() as u32);
    skipped[graph.entry].push(ruff_text_size::TextRange::at(
        start,
        ruff_text_size::TextSize::new(4),
    ));
    let owner = graph
        .session
        .closed_program_session(None, &graph.inputs)
        .unwrap();
    let roots = owner.closed_file_roots(&graph.modules, Some(&skipped), &proofs, None);
    owner.closed_program_errors(&roots, None).unwrap();
}

#[test]
fn closed_file_role_can_contribute_without_emitting_diagnostics() {
    let root = std::path::Path::new("/project");
    let helper = root.join("helper.py");
    let main = root.join("main.py");
    let mut session = super::Session::new(root, &[]).unwrap();
    session
        .add(
            &helper,
            "def take(value: int) -> int:\n    return value\nstatus: int = 'x'\n",
            crate::SourceKind::File,
        )
        .unwrap();
    session
        .add(
            &main,
            "from helper import take\ntake(True)\n",
            crate::SourceKind::File,
        )
        .unwrap();
    session.set_closed_program_rules();
    let sources = [(helper, true, None), (main, true, None)];
    let mut roots = session.closed_file_roots(&sources, None, &[], None);
    roots[0].diagnostic_path = None;
    let error = session.closed_program_errors(&roots, None).unwrap_err();
    assert!(
        error.contains("main.py:2") && error.contains("expected int, got bool"),
        "{error}"
    );
    assert!(!error.contains("invalid-assignment"), "{error}");
}

#[test]
fn fallback_call_bytearray_preserves_exact_argument_contract() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(&entry, "buffer = bytearray()\nprint(buffer)\ndef score(x: int) -> int:\n    return x\nscore(True)\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("expected int, got bool"), "{error}");
    let error = crate::compile_file(&entry, None, &Default::default())
        .err()
        .unwrap();
    assert!(error.contains("expected int, got bool"), "{error}");
}

#[test]
fn source_arguments_hold_for_file_and_named_entries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("app.py");
    let named = crate::CompileOptions {
        module_entry: Some("app".into()),
        require_checked_module_entry: true,
        script_startup: Some((Default::default(), "imre".into())),
        extension_paths: vec![dir.path().to_owned()],
        pythonpath: 0..1,
        ..Default::default()
    };
    let source = "class Box:\n    def method(self, x: int) -> int: return x + 1\nb = Box()\nf = b.method\nprint(f(True))\n";
    for prefix in ["", "import unittest\n"] {
        fs::write(&path, format!("{prefix}{source}")).unwrap();
        for options in [&crate::CompileOptions::default(), &named] {
            let path = if options.module_entry.is_some() {
                std::path::Path::new("-m")
            } else {
                path.as_path()
            };
            let error = crate::compile_file_with_boxed_units(path, None, options)
                .err()
                .expect("a bound method retains its parameter contract");
            assert!(
                error.contains(":9: Imre: expected int, got bool"),
                "{error}"
            );
        }
    }
}

#[test]
fn source_arguments_follow_the_selected_callable() {
    let dir = tempdir().unwrap();
    let app = dir.path().join("app.py");
    let api = dir.path().join("api.py");
    let declarations = r#"type Count = int
def take(n: Count) -> int: return n
class Box:
    def take(self, skip=0, /, *, n: Count) -> int: return n
    @staticmethod
    def static(n: int) -> int: return n
    @classmethod
    def factory(cls, n: int) -> int: return n
class Other:
    def take(self, *, n: bool) -> bool: return n
"#;
    fs::write(&api, declarations).unwrap();
    let prefix = "import api\nfrom api import Box as B\nprint(bytearray())\nb = B()\nf = b.take\n";
    let mut compiler = crate::IncrementalCompiler::default();
    for call in [
        "f(n=True)",
        "b.take(n=True)",
        "B.take(b, n=True)",
        "m = B.static\nm(True)",
        "m = B.factory\nm(True)",
        "api.take(True)",
        "alias = api.take\nalias(True)",
        "saved = f\nf = api.Other().take\nsaved(n=True)",
    ] {
        fs::write(&app, format!("{prefix}{call}\n")).unwrap();
        let error = compiler.check_file(&app, None).expect_err(call);
        assert!(error.contains("expected int, got bool"), "{call}: {error}");
    }
    fs::write(&app, format!("{prefix}f(n=7)\nf = api.Other().take\nf(n=True)\ndef run(f): return f(n=True)\nrun(f)\n")).unwrap();
    compiler.check_file(&app, None).unwrap();
}

#[test]
fn source_arguments_capture_inherited_methods_at_lookup() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("app.py");
    let source = r#"print(bytearray())
class T:
    def take(self, x: int) -> int: return x + 1
class U(T): pass
def other(self, x): return x
obj = U()
saved = obj.take
T.take = other
live = obj.take
print(live(True), saved(ARG))
"#;
    fs::write(&path, source.replace("ARG", "1")).unwrap();
    crate::check_file(&path, None).unwrap();
    fs::write(&path, source.replace("ARG", "True")).unwrap();
    let error = crate::check_file(&path, None).unwrap_err();
    assert!(
        error.contains("app.py:10:25: Imre: expected int, got bool"),
        "{error}"
    );
    assert!(!error.contains("app.py:10:12:"), "{error}");
}

#[test]
fn source_arguments_evaluate_bound_operands_in_call_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("app.py");
    let source = r#"print(bytearray())
class Box:
    def take(self, a: int, b: int): pass
    def stop(self) -> int: raise ValueError()
obj = Box()
f = obj.take
stop = obj.stop
try: f(ARGS)
except ValueError: pass
"#;
    fs::write(&path, source.replace("ARGS", "b=stop(), a=True")).unwrap();
    crate::check_file(&path, None).unwrap();
    fs::write(&path, source.replace("ARGS", "b=True, a=stop()")).unwrap();
    let error = crate::check_file(&path, None).unwrap_err();
    assert!(
        error.contains("app.py:8:10: Imre: expected int, got bool"),
        "{error}"
    );
}

#[test]
fn source_arguments_check_display_operands_before_contextual_widening() {
    let dir = tempdir().unwrap();
    let entry = dir.path().join("app.py");
    let mut compiler = crate::IncrementalCompiler::default();
    for (annotation, operand) in [
        ("list[int]", "[True]"),
        ("dict[str, list[int]]", "{'v': [1, True]}"),
        ("tuple[bool, int]", "(False, True)"),
        ("tuple[int, ...]", "(1, True)"),
        ("set[int]", "{True}"),
    ] {
        fs::write(&entry, format!("type Items = {annotation}\ndef take(a: Items): pass\nprint(bytearray())\nf = take\nf({operand})\n")).unwrap();
        let error = compiler.check_file(&entry, None).expect_err(annotation);
        assert!(
            error.contains("expected int, got bool"),
            "{annotation}: {error}"
        );
    }
    fs::write(
        &entry,
        "def take(a: list[int | bool]): pass\nprint(bytearray())\nf = take\nf([True, 2])\n",
    )
    .unwrap();
    compiler.check_file(&entry, None).unwrap();
}

#[test]
fn fallback_call_aliases_and_nested_operands() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        project.path().join("takes.py"),
        "def take(value: int) -> int:\n    return value\n",
    )
    .unwrap();
    fs::write(
        &entry,
        "from takes import take as score\nprint(bytearray())\nscore(value=True)\n",
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains(":3:13: Imre: expected int, got bool"),
        "{error}"
    );
    fs::write(&entry, "def inner(x: int) -> bool:\n    return False\ndef outer(x: int):\n    pass\nprint(bytearray())\nouter(inner(True))\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains(":6:13: Imre: expected int, got bool"),
        "{error}"
    );
    assert!(!error.contains(":6:7:"), "{error}");
    fs::write(&entry, "def inner(x: int) -> bool:\n    return False\ndef outer(x: bool, y: int):\n    pass\nprint(bytearray())\nouter(inner(True))\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.starts_with(&format!(
            "{}:6:13: Imre: expected int, got bool",
            fs::canonicalize(&entry).unwrap().display()
        )),
        "{error}"
    );
    assert!(error.contains("missing-argument"), "{error}");
}

#[test]
fn fallback_call_partial_binding_and_deferred_source_policy() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    for args in ["'x', *[]", "'x', 2"] {
        fs::write(
            &entry,
            format!(
                "def take(value: int):\n    pass\nprint(bytearray())\nif False:\n    take({args})\n"
            ),
        )
        .unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }
    fs::write(
        &entry,
        "def take(value: int, other: int):\n    pass\nprint(bytearray())\ntake(True)\n",
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("missing-argument") && !error.contains("got bool"),
        "{error}"
    );
    for extra in ["", "marker = (other := 1)\n"] {
        fs::write(&entry, format!("def take(value: int) -> int:\n    return value\nprint(bytearray())\n{extra}def f(value: take(True)):\n    pass\n")).unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(
            error.contains("unsupported type annotation") && !error.contains("got bool"),
            "{error}"
        );
    }
    let header = "from typing import TYPE_CHECKING\ndef take(value: int):\n    pass\nprint(bytearray())\nif False:\n    take(True)\nif TYPE_CHECKING:\n    take(True)\n";
    fs::write(&entry, format!("{header}take(*(1,))\n")).unwrap();
    crate::check_file(&entry, None).unwrap();
    fs::write(&entry, format!("{header}take(*(True,))\n")).unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains(":9:8: Imre: expected int, got bool"),
        "{error}"
    );
}

#[test]
fn unannotated_raise_stops_only_later_argument_checks() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    for prefix in ["", "import unittest\n"] {
        let source = format!(
            "{prefix}def boom():raise ValueError()\ndef take(a:int,b:int):return 0\ntry:take(boom(),True)\nexcept ValueError:pass\n"
        );
        fs::write(&entry, &source).unwrap();
        crate::check_file(&entry, None).unwrap();
        crate::compile_file(&entry, None, &Default::default()).unwrap();

        fs::write(&entry, source.replace("boom(),True", "True,boom()")).unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(
            error.contains(":10: Imre: expected int, got bool"),
            "{error}"
        );
    }
}

#[test]
fn unannotated_nonreturning_calls_require_stable_source() {
    // Check fallback directly so native refusals (e.g. decorators) cannot mask
    // a lost argument diagnostic.
    let check = |setup: &str| {
        let root = std::path::Path::new("/project");
        let main = root.join("main.py");
        let helper = root.join("helper.py");
        let mut session = super::Session::new(root, &[]).unwrap();
        session
            .add(
                &helper,
                "value = 1\ndef stop(): raise ValueError()\n",
                crate::SourceKind::File,
            )
            .unwrap();
        let source = format!(
            "def take(a: int, b: int): return 0\ndef ok(): return 1\n{setup}\ntake(boom(), True)\n"
        );
        session
            .add(&main, &source, crate::SourceKind::File)
            .unwrap();
        session.set_closed_program_rules();
        let sources = [(helper, true, None), (main, true, None)];
        let roots = session.closed_file_roots(&sources, None, &[], None);
        session.closed_program_errors(&roots, None)
    };
    for setup in [
        "def boom(x=1): raise ValueError()",
        "def boom() -> int: raise ValueError()\nsaved = boom",
        "def boom(): raise ValueError()\nsaved = boom",
        "from helper import stop as boom",
        "import helper\nhelper.value = 2\nfrom helper import stop as boom\nx = helper.value",
        "from helper import stop\ndef boom(): return stop()",
    ] {
        check(setup).unwrap_or_else(|e| panic!("{setup}: {e}"));
    }
    for setup in [
        "def boom(x=True):\n    if x: return 1\n    raise ValueError()",
        "def boom(): raise ValueError()\nboom = ok",
        "def boom(): raise ValueError()\nglobals()['boom'] = ok",
        "def boom(): raise ValueError()\ndef change(): globals()['boom'] = ok\nchange()",
        "def boom(): raise ValueError()\nok.__globals__['boom'] = ok",
        "import sys\ndef boom(): raise ValueError()\nsys.modules[__name__].boom = ok",
        "def boom(): raise ValueError()\ndef change():\n    global boom\n    boom = ok\nchange()",
        "def replace(f): return ok\n@replace\ndef boom(): raise ValueError()",
        "def boom(x=1): raise ValueError()\nboom.__defaults__ = (2,)",
        "def boom(): raise ValueError()\nsaved = boom\nsaved.__code__ = ok.__code__",
        "def boom(): raise ValueError()\nsetattr(boom, '__code__', ok.__code__)",
        "def boom(): raise ValueError()\ndef patch(f): f.__code__ = ok.__code__\npatch(boom)",
        "import helper\nhelper.stop = ok\nfrom helper import stop as boom",
    ] {
        let error = check(setup).expect_err(setup);
        assert!(
            error.contains(":14: Imre: expected int, got bool"),
            "{setup}: {error}"
        );
    }
}

#[test]
fn fallback_call_archived_thread_keeps_original_argument_diagnostic() {
    let project = tempdir().unwrap();
    let entry = project.path().join("worker_unused.py");
    fs::write(project.path().join("scores.py"), "def take(value: int) -> int:\n    return value\n\ndef measure() -> int:\n    return take('x')\n").unwrap();
    fs::write(&entry, "import socket\nimport threading\n\nimport scores\n\ndef receive(stream):\n    with stream as source:\n        print(source.recv(2))\n\nif False:\n    reader, writer = socket.socketpair()\n    writer.sendall(b\"Rx\")\n    task = threading.Thread(target=receive, args=(reader,))\n    task.start()\n    task.join()\n    writer.close()\nscores.measure()\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("scores.py:5:17: Imre: expected int, got str"),
        "{error}"
    );
}

#[test]
fn object_fallback_checks_first_party_scalar_stores() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let dynamic = "buffer = bytearray()\nbuffer.append(97)\nprint(buffer)\n";
    let reject = || {
        for error in [
            crate::check_file(&main, None).unwrap_err(),
            crate::compile_file(&main, None, &Default::default())
                .err()
                .unwrap(),
        ] {
            assert!(error.contains("expected int, got bool"), "{error}");
        }
    };
    for store in [
        "status: int = True\n",
        "status: int = 1\nstatus = True\n",
        "def run():\n    status: int = True\nrun()\n",
    ] {
        fs::write(&main, format!("{dynamic}{store}")).unwrap();
        reject();
    }
    for store in ["status: int = str(1)\n", "status: int = 1\nstatus = 'x'\n"] {
        fs::write(&main, format!("{dynamic}{store}")).unwrap();
        let error = crate::check_file(&main, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }
    fs::write(&main, format!("{dynamic}status: int = 'x'\n")).unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(error.contains("invalid-assignment"), "{error}");
    assert!(!error.contains("Imre: expected int, got str"), "{error}");
    fs::write(project.path().join("dynamic.py"), dynamic).unwrap();
    fs::write(project.path().join("bad.py"), "status: int = True\n").unwrap();
    fs::write(&main, "import dynamic\nimport bad\n").unwrap();
    reject();
    for (value, expected) in [
        ("1", "expected float, got int"),
        ("True", "expected float, got bool"),
    ] {
        fs::write(&main, format!("{dynamic}status: float = {value}\n")).unwrap();
        let error = crate::check_file(&main, None).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    fs::write(&main, format!("from typing import TYPE_CHECKING\n{dynamic}status: int = 1\nchoice: int | bool = True\ntype Choice = int | bool\nother: Choice = True\nif False:\n    dead: int = True\nif TYPE_CHECKING:\n    hidden: int = True\n")).unwrap();
    crate::check_file(&main, None).unwrap();
}

#[test]
fn object_fallback_checks_unique_parameter_reassignments() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let dynamic = "buffer = bytearray()\nbuffer.append(97)\nprint(buffer)\n";
    fs::write(
        &main,
        format!("{dynamic}def change(status: int) -> None:\n    status = True\nchange(1)\n"),
    )
    .unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(error.contains("expected int, got bool"), "{error}");

    for body in [
        "status = 2",
        "if False:\n        status = True",
        "return\n    status = True",
        "if TYPE_CHECKING:\n        status = True",
    ] {
        fs::write(&main, format!("from typing import TYPE_CHECKING\n{dynamic}def change(status: int) -> None:\n    {body}\nchange(1)\n")).unwrap();
        crate::check_file(&main, None).unwrap_or_else(|e| panic!("{body}: {e}"));
    }
    fs::write(&main, format!("{dynamic}first: int = 'first'\ndef change(flag: bool) -> None:\n    if flag:\n        status: int\n    else:\n        status: str\n    status = True\nlast: int = 'last'\nchange(True)\n")).unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(!error.contains("Imre: expected int, got bool"), "{error}");
    let first = error.find("Literal[\"first\"]").unwrap();
    let last = error.find("Literal[\"last\"]").unwrap();
    assert!(first < last, "{error}");
}

#[test]
fn source_stores_and_results_survive_registration() {
    let project = tempdir().unwrap();
    let root = project.path();
    let entry = root.join("main.py");
    let helper = "class Box:\n    def __init__(self): self.value: int = 2\ndef flag() -> bool: return True\n";
    fs::write(root.join("helper.py"), helper).unwrap();
    fs::write(root.join("peer.py"), "print(bytearray())\n").unwrap();
    for (body, expected) in [
        ("box = Box()\nbox.value = flag()", "expected int, got bool"),
        (
            "def result() -> int:\n    value = flag()\n    return value\nresult()",
            "expected int, got bool",
        ),
        ("value: int = 2\nvalue += 1.5", "float"),
    ] {
        for import in ["", "import unittest\n", "import peer\n"] {
            fs::write(
                &entry,
                format!("{import}from helper import Box, flag\n{body}\n"),
            )
            .unwrap();
            let error = crate::compile_file(&entry, None, &Default::default())
                .err()
                .unwrap_or_else(|| panic!("accepted {import}{body}"));
            assert!(
                error.contains("main.py:") && error.contains(expected),
                "{error}"
            );
        }
    }
    // Neither file contributes a function or class to the selected graph.
    fs::write(root.join("helper.py"), "count: int = 1\n").unwrap();
    fs::write(&entry, "import peer\nimport helper\nhelper.count = True\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("expected int, got bool"), "{error}");
}

#[test]
fn registered_stores_follow_declarations_and_container_elements() {
    let project = tempdir().unwrap();
    let check = |source: &str| {
        let module = ("main".into(), project.path().join("main.py"), source.into());
        super::check_registered_project(
            project.path(),
            &[],
            std::iter::once(RegisteredSource::new(&module, true, None)),
            None,
            None,
            None,
            None,
        )
    };
    for body in [
        "class Box:\n    n: int = False",
        "class Box:\n    def __init__(self): self.n: int = False",
        "def local():\n    n: int\n    n = 1 == 1",
        "n: int = 0\ndef change():\n    global n\n    n = True",
        "def outer():\n    n: int = 1\n    def inner():\n        nonlocal n\n        n = False",
        "def optional() -> int | None: return False",
        "n: int = 1\npair = (True, 2)\nn, other = pair",
        "items: list[int] = [1]\nitems = [True]",
        "rows: list[list[int]] = [[1]]\nrows[0][0] = True",
        "def change(items: list[int], i: int): items[i] = False",
        "type Rows = list[tuple[str, int]]\ndef rows() -> Rows: return [('a', True)]",
        "values: dict[str, int] = {'x': False}",
    ] {
        let error = check(body).expect_err(body);
        assert!(error.contains("expected int, got bool"), "{body}: {error}");
    }
    for body in [
        "items: list[int] = [1]\nother = ['a']\nitems = other",
        "def change(n: int): n /= 2",
        "class Box:\n    def __init__(self): self.n: int = 2\nb = Box()\nb.n += 1.5",
    ] {
        let error = check(body).expect_err(body);
        assert!(error.contains("invalid-assignment"), "{body}: {error}");
    }
    check(
        "n: int = 0
def f(x) -> int:
    n = False
    n = 'other'
    return 1
f(1)
f('a')
class A:
    def __init__(self): self.n: int = 1
class B:
    def __init__(self): self.n = 1
b = B()
b.n = True
amount: float = 1.0
amount += 1
items: list[int | bool] = [0, True]
nullable: int | None = None
",
    )
    .unwrap();
}

#[test]
fn heterogeneous_unannotated_lists_use_object_storage_without_erasing_contracts() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    fs::write(
        &main,
        "import sys\nsys.path.append('unused')\nvalues = [None, 1, 'text', [2]]\nprint(values)\n",
    )
    .unwrap();
    crate::check_file(&main, None).unwrap();

    fs::write(&main, "values: list[int] = [1, True]\n").unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(error.contains("expected int, got bool"), "{error}");
}

#[test]
fn explicitly_written_bool_and_int_union_arms_keep_both_contracts() {
    let dir = tempdir().unwrap();
    let entry = dir.path().join("main.py");
    for annotation in ["int | bool", "bool | int", "Union[int, bool]"] {
        fs::write(
            &entry,
            format!("from typing import Union\ndef population(value: {annotation}) -> int:\n    return value.bit_count()\nprint(population(False), population(True), population(2))\n"),
        )
        .unwrap();
        crate::check_file(&entry, None).unwrap_or_else(|e| panic!("{annotation}: {e}"));
    }
    for (annotation, argument, expected) in [
        ("int", "False", "expected int, got bool"),
        ("int | bool", "'bad'", "str"),
    ] {
        fs::write(
            &entry,
            format!("def take(value: {annotation}) -> int: return 0\nprint(take({argument}))\n"),
        )
        .unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn module_qualified_class_value_uses_its_python_binding() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    fs::write(
        project.path().join("scores.py"),
        "class Item:\n    value: int\n    def __init__(self) -> None:\n        self.value = 0\n",
    )
    .unwrap();
    fs::write(
        &main,
        "import scores\ninstance = scores.Item()\nsetattr(scores.Item, 'value', property(None, lambda _, value: print(value)))\ninstance.value = 2\n",
    )
    .unwrap();
    crate::check_file(&main, None).unwrap();

    fs::write(&main, "import scores\nprint(scores.Item().missing)\n").unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(
        error.contains("main.py:2:7: Imre: unknown field"),
        "{error}"
    );
}

#[test]
fn a_dormant_async_method_preserves_an_unrelated_definitions_field_contract() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let scores = project.path().join("scores.py");
    fs::write(
        &main,
        "import scores\nclass Dormant:\n    async def __aiter__(self):\n        yield 1\n",
    )
    .unwrap();
    let item = "class Item:\n    values: list[int]\n    def __init__(self) -> None:\n        self.values = [1]\n";
    for (function, location) in [
        (
            "def store(x: Item) -> None:\n    x.values.append('wrong')\n",
            "scores.py:7:21",
        ),
        (
            "def store(x: Item) -> None:\n    def write() -> None:\n        x.values.append('wrong')\n    write()\n",
            "scores.py:8:25",
        ),
    ] {
        fs::write(&scores, format!("{item}\n{function}")).unwrap();
        let error = crate::check_file(&main, None).unwrap_err();
        assert!(error.contains(location), "{error}");
        assert!(error.contains("expected int, got str"), "{error}");
    }
}

#[test]
fn a_json_write_to_an_inferred_local_preserves_the_next_declared_store() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let reader = project.path().join("reader.py");
    fs::write(
        &main,
        "from reader import select\nselect('{\"packages\":[]}')\n",
    )
    .unwrap();
    let source = concat!(
        "import json\n\n",
        "def select(raw: str):\n",
        "    packages = json.loads(raw)['packages']\n",
        "    current = None\n",
        "    for package in packages:\n",
        "        current = package['version']\n",
        "    return current\n",
    );
    fs::write(&reader, source).unwrap();
    crate::check_file(&main, None).unwrap();

    let invalid = source.replace("    return", "    later: str = 17\n    return");
    fs::write(&reader, &invalid).unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(
        error.contains("reader.py:8:18: Imre: expected str, got int"),
        "{error}"
    );

    fs::write(
        &reader,
        invalid.replace("current = None", "current: None = None"),
    )
    .unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(
        error.contains("reader.py:7:19: Imre: expected None, got JSON value"),
        "{error}"
    );
}

#[test]
fn an_inferred_loop_local_accepts_none_and_its_written_scalar() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let reader = project.path().join("reader.py");
    fs::write(
        &main,
        "from reader import select\nprint(select(['body', '']))\n",
    )
    .unwrap();
    let source = concat!(
        "def select(items: list[str]) -> str:\n",
        "    for item in items:\n",
        "        value = None\n",
        "        if item:\n",
        "            value = item\n",
        "        if value is not None:\n",
        "            return value\n",
        "    return 'empty'\n",
    );
    fs::write(&reader, source).unwrap();
    crate::check_file(&main, None).unwrap();

    fs::write(
        &reader,
        source.replace("value = None", "value: None = None"),
    )
    .unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(
        error.contains("reader.py:5:21: Imre: expected None, got str"),
        "{error}"
    );
}

#[test]
fn rebound_imported_cells_keep_their_storage_through_checking() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let storage = project.path().join("storage.py");
    fs::write(&main, "from storage import seen\nprint(seen())\n").unwrap();
    fs::write(
        &storage,
        "from typing import Optional\nclass Console: pass\ncache: Optional[Console] = None\ncache = Console()\ndef seen() -> bool: return cache is cache\n",
    )
    .unwrap();
    let mut graph = crate::modules::SourceGraph::load(&main, Some(project.path()), &[]).unwrap();
    assert!(matches!(
        crate::checked_lower(&mut graph).unwrap(),
        crate::CheckedProgram::Native(_)
    ));

    fs::write(
        &storage,
        "cache = object()\ncache = object()\ndef seen() -> bool: return cache is cache\n",
    )
    .unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(
        error.contains("storage.py:1:1: Imre: a rebindable module value requires"),
        "{error}"
    );

    fs::write(project.path().join("left.py"), "value = 1\n").unwrap();
    fs::write(project.path().join("right.py"), "value = 2\n").unwrap();
    fs::write(
        &storage,
        r#"import left
import right

def choose(first: bool):
    return left if first else right

cache = choose(True)

def seen() -> bool:
    global cache
    saved = cache
    cache = choose(False)
    return saved is left and cache is right
"#,
    )
    .unwrap();
    let mut graph = crate::modules::SourceGraph::load(&main, Some(project.path()), &[]).unwrap();
    assert!(matches!(
        crate::checked_lower(&mut graph).unwrap(),
        crate::CheckedProgram::Native(_)
    ));
}

#[test]
fn an_imported_function_can_read_a_registered_type_alias_as_a_value() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        project.path().join("aliases.py"),
        include_str!("../../../../tests/fixtures/boxed_type_alias_values.py"),
    )
    .unwrap();
    fs::write(&entry, "from aliases import current\ncurrent()\n").unwrap();
    crate::check_file(&entry, None).unwrap();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn selector_file_objects_use_the_registered_provider_and_integer_fds_stay_native() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/cpython_stdlib/full/select");
    crate::check_file(&fixtures.join("boxed_fileobjs.py"), None).unwrap();

    let integer_fds = fixtures.join("selector_operations.py");
    let mut graph = crate::modules::SourceGraph::load(&integer_fds, None, &[]).unwrap();
    assert!(matches!(
        crate::checked_lower(&mut graph).unwrap(),
        crate::CheckedProgram::Native(_)
    ));
}

#[test]
fn dormant_import_preserves_the_next_call_diagnostic() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(project.path().join("child.py"), "value = 17\n").unwrap();
    fs::write(
        &entry,
        "if False:\n    import child\nprint(__import__('child', bogus=1))\n",
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("main.py:3:7: Imre: keyword arguments"),
        "{error}"
    );
}

#[test]
fn registered_enum_keeps_its_finite_member_domain() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let client = r#"from enum import Enum
from unittest.mock import Mock

class Box:
    class State(Enum):
        OPEN = 1
        CLOSED = 2

def read(state: Box.State) -> int:
    match state:
        case Box.State.OPEN: return 1
        case Box.State.CLOSED: return 2

print(Mock(return_value=read(Box.State.OPEN))())
"#;
    fs::write(&entry, client).unwrap();
    crate::check_file(&entry, None).unwrap();
    fs::write(
        &entry,
        client.replace("        case Box.State.CLOSED: return 2\n", ""),
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("implicitly return") && error.contains("main.py:9:"),
        "{error}"
    );
}

#[test]
fn registered_return_uses_the_selected_abc_metaclass() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(&entry, "from ast import literal_eval\ntry:\n    import suspect\nexcept ImportError:\n    pass\nprint(literal_eval('27'))\n").unwrap();
    fs::write(
        project.path().join("suspect.py"),
        "from abc import ABCMeta\ndef create() -> ABCMeta:\n    return 42\n",
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("suspect.py:3:12"), "{error}");

    fs::write(project.path().join("abc.py"), "ABCMeta = int\n").unwrap();
    crate::check_file(&entry, None).unwrap();
}

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

    fs::write(
        &suspect,
        "from typing import Any as Open\ndef fresh() -> Open: return object()\ndef number(value: Open) -> int: return value\n",
    )
    .unwrap();
    crate::check_file(&entry, None).unwrap();

    fs::write(project.path().join("typing.py"), "class Any: pass\n").unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(error.contains("suspect.py:2:29"), "{error}");
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
        let mut check = |cache| {
            let owner = graph.session.closed_program_session(cache, &graph.inputs)?;
            let roots = owner.closed_file_roots(&graph.modules, skipped.as_deref(), &proofs, None);
            owner.closed_program_errors(&roots, native)
        };
        let result = check(Some(&mut cache));
        let fresh = check(None);
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
    let mut cache = AnalysisCache::default();
    let mut verify = |sources: &[_]| {
        super::check_registered_project(
            root,
            &[],
            sources.iter().enumerate().map(|(i, s)| {
                let id = if i == 0 {
                    graph.entry
                } else {
                    graph.python_file("api").unwrap()
                };
                RegisteredSource::new(s, true, Some(id))
            }),
            None,
            Some((&graph, &Default::default(), None)),
            None,
            Some((&mut cache, true)),
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

    registered[1].2 = number.into();
    verify(&registered).unwrap();
    registered[1].2 = text.into();
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
        Some((&graph, &Default::default(), None)),
        None,
        Some((&mut cache, true)),
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
fn registered_sdk_interface_belongs_to_its_original_source() {
    let project = tempdir().unwrap();
    let root = project.path();
    let sdk = std::path::PathBuf::from(crate::dynamic::BUNDLED_SOURCE_ROOT);
    let (path, original) = crate::modules::bundled_runtime_source("http").unwrap();
    let mut sources = [
        ("http".into(), sdk.join(path), original.into()),
        (
            "main".into(),
            root.join("main.py"),
            "from http import HTTPStatus\ndef code() -> int:\n    return HTTPStatus.CREATED\n"
                .into(),
        ),
    ];
    let mut cache = AnalysisCache::default();
    let mut check = |sources: &[_]| {
        super::check_registered_project(
            root,
            std::slice::from_ref(&sdk),
            sources
                .iter()
                .enumerate()
                .map(|(i, source)| RegisteredSource::new(source, i == 1, None)),
            None,
            None,
            None,
            Some((&mut cache, true)),
        )
    };

    check(&sources).unwrap();
    sources[0].2 = "class HTTPStatus:\n    CREATED = (201, 'Created', 'example')\n".into();
    let error = check(&sources).unwrap_err();
    assert!(
        error.contains("invalid-return-type") && error.contains("main.py:3"),
        "{error}"
    );
    sources[0].2 = original.into();
    check(&sources).unwrap();
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
fn a_selected_handler_must_accept_all_keywords_together() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let check = |call: &str| {
        let handlers = r#"def first(x: int, y: str) -> int:
    return x
def second(y: int, x: str) -> int:
    return y
handlers = {"first": first, "second": second}
"#;
        fs::write(&entry, format!("{handlers}{call}\n")).unwrap();
        crate::check_file(&entry, None)
    };
    check("print(handlers[input()](x=1, y='ok'))").unwrap();
    let error = check("print(handlers[input()](x=1, y=2))").unwrap_err();
    assert!(error.contains("invalid-argument-type"), "{error}");
}

#[test]
fn a_module_dictionary_write_only_replaces_a_live_module_name() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("main.py");
    let functions = "def target(n: int): return n\ndef replacement(): return 37\n";
    fs::write(tmp.path().join("helper.py"), functions).unwrap();
    let options = crate::CompileOptions {
        script_startup: Some((crate::CommandFlags::default(), "imre".into())),
        ..Default::default()
    };
    let compile = |source: String| {
        fs::write(&path, source).unwrap();
        crate::compile_file(&path, None, &options)
    };
    compile(format!("{functions}def enter(): return target()\nprint(set([1, 2]))\nglobals()['target'] = replacement\nprint(enter())\n")).unwrap();
    compile(format!(
        "{functions}print(set([1, 2]))\nexec(\"target = replacement\")\nprint(target())\n"
    ))
    .unwrap();
    let mut compiler = crate::IncrementalCompiler::default();
    compiler.check_file(&path, None).unwrap();
    fs::write(
        &path,
        format!("{functions}print(set([1, 2]))\nexec(\"other = replacement\")\nprint(target())\n"),
    )
    .unwrap();
    let error = compiler.check_file(&path, None).unwrap_err();
    assert!(error.contains("No argument provided"), "{error}");
    for source in [
        "def replacement(): return 37\nglobals()['target'] = replacement\ndef target(n: int): return n\nprint(set([1, 2]))\nprint(target())\n".to_owned(),
        "from helper import replacement\ndef enter():\n    from helper import target\n    return target()\nprint(set([1, 2]))\nglobals()['target'] = replacement\nprint(enter())\n".to_owned(),
        format!("{functions}print(set([1, 2]))\nprint(target())\nglobals()['target'] = replacement\n"),
        "from helper import replacement\ndef enter():\n    from helper import target as local\n    return local()\nprint(set([1, 2]))\nglobals()['local'] = replacement\n".to_owned(),
    ] {
        let error = compile(source).err().expect("the original target still requires n");
        assert!(error.contains("No argument provided"), "{error}");
    }
}

#[test]
fn a_source_function_can_gain_a_python_default() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("main.py");
    fs::write(&path, "def target(n: int): return n\ndef enter(): return target()\nprint(set([1, 2]))\ntarget.__defaults__ = (37,)\nprint(enter())\n").unwrap();
    let options = crate::CompileOptions {
        script_startup: Some((crate::CommandFlags::default(), "imre".into())),
        ..Default::default()
    };
    crate::compile_file(&path, None, &options).unwrap();
}

#[test]
fn a_cold_call_keeps_its_first_party_argument_shape_across_layouts() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        project.path().join("scores.py"),
        "def take(value: int) -> int: return value\n",
    )
    .unwrap();
    for (imports, operation) in [
        ("", "print(set([1, 2]))"),
        (
            "from fractions import Fraction\n",
            "print(Fraction(1, 2) < 1)",
        ),
    ] {
        fs::write(
            &entry,
            format!("from scores import take\n{imports}def unused(): return take()\n{operation}\n"),
        )
        .unwrap();
        let error = crate::compile_file_with_layout(
            &entry,
            None,
            &crate::CompileOptions::default(),
            crate::CodegenLayout::Whole,
        )
        .err()
        .expect("the written call omits value");
        assert!(
            error.contains("missing-argument") && error.contains("value"),
            "{error}"
        );
    }
}

#[test]
fn a_copied_function_layout_does_not_declare_its_argument() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(
        project.path().join("provider.py"),
        "def take(n: int) -> int: return n\n",
    )
    .unwrap();
    let scores = project.path().join("scores.py");
    let source = "import provider\ndef app() -> int:\n    method = provider.take\n    if False:\n        return method('x')\n    return 42\n";
    for neighbor in ["", "def unused(): return locals()\n"] {
        fs::write(
            &entry,
            format!("import scores\n{neighbor}print(set([1,2]))\nprint(scores.app())\n"),
        )
        .unwrap();
        fs::write(&scores, source).unwrap();
        crate::check_file(&entry, None).unwrap();

        fs::write(&scores, source.replace("return 42", "return 'wrong'")).unwrap();
        let error = crate::check_file(&entry, None).unwrap_err();
        assert!(
            error.contains("scores.py:6:12")
                && (error.contains("expected int, got str")
                    || error.contains("invalid-return-type")),
            "{error}"
        );
    }
}

#[test]
fn a_dormant_module_call_keeps_its_required_imported_argument() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let scores = project.path().join("scores.py");
    let provider = project.path().join("provider.py");
    fs::write(&provider, "def take(n: int) -> int:\n    return n\n").unwrap();
    fs::write(&scores, "import provider\nif False:\n    provider.take()\n").unwrap();
    let mut compiler = crate::IncrementalCompiler::default();
    for neighbor in ["", "def f(): return locals()\n"] {
        fs::write(
            &entry,
            format!("import scores\n{neighbor}print(set([1,2]))\nprint(42)\n"),
        )
        .unwrap();
        let error = compiler.check_file(&entry, None).unwrap_err();
        assert_eq!(
            error,
            format!(
                "{}:3:5: Imre: missing required argument \"n\"",
                scores.canonicalize().unwrap().display()
            )
        );
    }
    let prior = project.path().join("prior.py");
    fs::write(&prior, "def wrong() -> int:\n    return 'wrong'\n").unwrap();
    let main = fs::read_to_string(&entry).unwrap();
    fs::write(&entry, format!("import prior\n{main}")).unwrap();
    let error = compiler.check_file(&entry, None).unwrap_err();
    assert!(error.contains("prior.py:2:12"), "{error}");
    fs::write(prior, "def wrong() -> int:\n    return 0\n").unwrap();
    fs::write(provider, "def take(n: int = 37) -> int:\n    return n\n").unwrap();
    compiler.check_file(&entry, None).unwrap();
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
fn included_module_uses_the_checked_wrapper_representation() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    fs::write(project.path().join("plugin.py"), "value = 1\n").unwrap();
    fs::write(
        &entry,
        r#"from functools import update_wrapper
from typing import Callable

class Counter:
    def plus(self, value: int) -> int:
        return value + 1

def plain(value: int) -> int:
    return value * 2

def wrapped(fn: Callable[[int], int]) -> int:
    try:
        update_wrapper(fn, plain, (), ())
    except AttributeError:
        return fn(10)
    return fn.__wrapped__(10)

print(wrapped(Counter().plus), wrapped(plain))
"#,
    )
    .unwrap();
    let options = crate::CompileOptions {
        include_modules: ["plugin".to_owned()].into(),
        ..Default::default()
    };
    crate::check_file_with_options(&entry, None, &options).unwrap();
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
    let inputs = crate::dynamic::compile(
        (
            crate::ProgramEntry::File(&entry, None),
            None,
            Default::default(),
            Default::default(),
        ),
        &options,
        true,
        Some(crate::dynamic::ObjectOperation::CheckedEntry),
        &mut Some(graph),
        None,
        |_, _, sources| {
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
    assert!(inputs.files.contains(&(
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
        (
            crate::ProgramEntry::File(&entry, None),
            None,
            Default::default(),
            Default::default(),
        ),
        &options,
        true,
        Some(crate::dynamic::ObjectOperation::CheckedEntry),
        &mut Some(next),
        None,
        |_, _, _| panic!("a different import kind reached object admission"),
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
    let root = project.path().join("files");
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    let root = {
        let alias = project.path().join("invoked");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        alias
    };
    let entry = root.join("main.py");
    let report = root.join("report.py");
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
        "{prefix}run(run_name='report', path_name={:?})\nrun({:?})",
        report.to_str().unwrap(),
        report.canonicalize().unwrap().to_str().unwrap(),
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
    check(format!("{prefix}run(sys.argv[1], path_name=sys.argv[2])")).unwrap();
    let nested = check(format!(
        "{prefix}run(sys.argv[1], path_name=run(sys.argv[2]))"
    ))
    .unwrap_err();
    assert!(nested.contains("main.py:3:32"), "{nested}");
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
fn run_path_checks_finally_after_a_return() {
    let project = tempdir().unwrap();
    let entry = project.path().join("main.py");
    let source = r#"import runpy, sys
def answer():
    try:
        return 73
        def dormant(): return runpy.run_path(sys.argv[1])
    finally:
        pass
print(answer())
"#;
    fs::write(&entry, source).unwrap();
    crate::check_file(&entry, None).unwrap();

    fs::write(
        &entry,
        source.replace("pass", "runpy.run_path(sys.argv[1])"),
    )
    .unwrap();
    let error = crate::check_file(&entry, None).unwrap_err();
    assert!(
        error.contains("main.py:7:24") && error.contains("open executable filename"),
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
        None,
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

#[test]
fn dormant_lexical_checks_keep_known_parameters_of_partial_calls() {
    let project = tempdir().unwrap();
    let root = project.path();
    for (parameters, call) in [
        ("n: int, *, label: str", "take('x')"),
        ("n: int, *rest", "take('x', 2)"),
        ("n: int, *rest", "take('x', *())"),
        ("n: int, /, **kw", "take('x', n=2)"),
    ] {
        let source = (
            "scores".to_owned(),
            root.join("scores.py"),
            format!("def take({parameters}): pass\nif False:\n    {call}\n").into(),
        );
        let error = super::check_registered_project(
            root,
            &[],
            [RegisteredSource::new(&source, true, None)],
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(
            error.contains("scores.py:3:10: Imre: expected int, got str"),
            "{call}: {error}"
        );
    }
}

#[test]
fn initializer_checks_record_the_startup_namespace_lookup() {
    let root = tempdir().unwrap();
    let path = root.path().join("app.py");
    let mut session = super::Session::new(root.path(), &[]).unwrap();
    session
        .add(
            &path,
            "from __main__ import receive\n",
            crate::SourceKind::File,
        )
        .unwrap();
    for runs_as_main in [false, true] {
        let check = session.initializer_call_error((0, &path), runs_as_main, |_| {
            panic!("the startup namespace uses its own resolver")
        });
        assert!(check.read_import_binding);
    }
}

#[test]
fn execution_operands_follow_the_selected_initializer() {
    let project = tempdir().unwrap();
    let root = project.path();
    let app = root.join("app.py");
    fs::write(
        &app,
        "import bridge\nfrom producer import consume\nconsume('ok')\n",
    )
    .unwrap();
    fs::write(root.join("bridge.py"), "import leaf\n").unwrap();
    fs::write(
        root.join("producer.py"),
        "def consume(value):\n    exec(input())\n",
    )
    .unwrap();
    let mut compiler = crate::IncrementalCompiler::default();
    let leaf = root.join("leaf.py");
    let raising = "print('enter leaf')\nraise LookupError()\n";
    fs::write(&leaf, raising).unwrap();
    compiler.check_file(&app, None).unwrap();
    for (source, error) in [
        ("exec(input())\nraise LookupError()\n", "leaf.py:1:6"),
        ("pass\n", "producer.py:2:10"),
    ] {
        fs::write(&leaf, source).unwrap();
        let actual = compiler.check_file(&app, None).unwrap_err();
        assert!(actual.contains(error), "{actual}");
    }

    fs::write(&leaf, raising).unwrap();
    fs::write(
        root.join("bridge.py"),
        "try:\n    import leaf\nexcept LookupError:\n    print('caught leaf')\n",
    )
    .unwrap();
    let actual = compiler.check_file(&app, None).unwrap_err();
    assert!(actual.contains("producer.py:2:10"), "{actual}");
    fs::write(
        root.join("producer.py"),
        "def consume(value):\n    exec(\"print('producer')\")\n",
    )
    .unwrap();
    compiler.check_file(&app, None).unwrap();

    fs::write(
        root.join("bridge.py"),
        "try:\n    pass\nfinally:\n    try:\n        import leaf\n    except LookupError:\n        pass\n",
    )
    .unwrap();
    compiler.check_file(&app, None).unwrap();
}

#[test]
fn imported_exec_aliases_follow_the_binding_held_at_the_call() {
    let project = tempdir().unwrap();
    let root = project.path();
    let app = root.join("app.py");
    let leaf = root.join("leaf.py");
    let producer = root.join("producer.py");
    fs::write(
        &app,
        "import bridge\nfrom producer import consume\nconsume('ok')\n",
    )
    .unwrap();
    fs::write(&leaf, "raise LookupError()\n").unwrap();
    fs::write(root.join("bridge.py"), "import leaf\n").unwrap();
    fs::write(
        &producer,
        "launch = exec\ndef consume(value):\n    launch(input())\n",
    )
    .unwrap();
    let mut compiler = crate::IncrementalCompiler::default();
    compiler.check_file(&app, None).unwrap();

    fs::write(&leaf, "pass\n").unwrap();
    let error = compiler.check_file(&app, None).unwrap_err();
    assert!(
        error.contains("producer.py:3:12: Imre: this call may select builtins.exec"),
        "{error}"
    );

    let bindings =
        "launch = exec\nheld = launch\ndef record(value): print(value)\nlaunch = record\n";
    let mut check = |name: &str| {
        fs::write(
            &producer,
            format!("{bindings}def consume(value):\n    {name}(input())\n"),
        )
        .unwrap();
        compiler.check_file(&app, None)
    };
    let error = check("held").unwrap_err();
    assert!(
        error.contains("producer.py:6:10: Imre: this call may select builtins.exec"),
        "{error}"
    );
    check("launch").unwrap();

    fs::write(
        &producer,
        "def consume(value):\n    exec(\"print('producer')\")\n",
    )
    .unwrap();
    let compiled = crate::compile_file(&app, None, &crate::CompileOptions::default()).unwrap();
    assert_eq!(
        compiled
            .llvm_ir
            .matches("call i64 @imre_dyn_exec_operand(")
            .count(),
        1
    );
}

#[test]
fn a_nested_import_resumes_the_checked_caller_only_on_its_normal_edge() {
    let root = tempdir().unwrap();
    let paths = ["app.py", "bridge.py", "leaf.py"].map(|name| root.path().join(name));
    let caller = "from bridge import take\ntake('wrong')\n";
    let bridge = "import leaf\ndef take(value: int): return value\n";
    for (leaf, reached) in [
        ("value = 3\n", true),
        ("import bridge\n", true),
        ("from bridge import take\n", false),
        ("raise SystemExit(19)\n", false),
        ("import bridge\nbridge.take = replacement\n", false),
    ] {
        let mut session = super::Session::new(root.path(), &[]).unwrap();
        for (path, source) in paths.iter().zip([caller, bridge, leaf]) {
            session.add(path, source, crate::SourceKind::File).unwrap();
        }
        let result = session.initializer_call_error((0, &paths[0]), true, |name| match name {
            "bridge" => Some((1, &paths[1])),
            "leaf" => Some((2, &paths[2])),
            _ => None,
        });
        assert_eq!(result.error.is_some(), reached, "{leaf}");
        if let Some(error) = result.error {
            assert!(
                error
                    .to_string()
                    .contains("app.py:2:6: Imre: expected int, got str")
            );
        }
    }
}

#[test]
fn grouped_imports_resume_at_each_alias_and_keep_its_binding() {
    let root = tempdir().unwrap();
    let paths = ["app.py", "leaf.py", "bridge.py"].map(|name| root.path().join(name));
    let caller = "import leaf as first, bridge as second\nsecond.take('wrong')\n";
    let bridge = "import leaf\ndef take(value: int): return value\n";
    for (leaf, reached) in [
        ("def take(value: str): return value\n", true),
        ("raise SystemExit(19)\n", false),
    ] {
        let mut session = super::Session::new(root.path(), &[]).unwrap();
        for (path, source) in paths.iter().zip([caller, leaf, bridge]) {
            session.add(path, source, crate::SourceKind::File).unwrap();
        }
        let result = session.initializer_call_error((0, &paths[0]), true, |name| match name {
            "leaf" => Some((1, &paths[1])),
            "bridge" => Some((2, &paths[2])),
            _ => None,
        });
        assert_eq!(result.error.is_some(), reached);
        if let Some(error) = result.error {
            assert!(
                error
                    .to_string()
                    .contains("app.py:2:13: Imre: expected int, got str")
            );
        }
    }
}

#[test]
fn initializer_calls_use_original_bindings_and_all_arguments() {
    let root = tempdir().unwrap();
    let paths = [
        (root.path().join("main.py"), true),
        (root.path().join("scores.py"), true),
    ];
    let number = "def take(value: int, count=0): return value\n";
    let check = |caller: &str, scores: &str| {
        let mut session = super::Session::new(root.path(), &[]).unwrap();
        for ((path, _), source) in paths.iter().zip([caller, scores]) {
            session.add(path, source, crate::SourceKind::File).unwrap();
        }
        session
            .initializer_call_error((0, &paths[0].0), true, |name| {
                (name == "scores").then_some((1, &paths[1].0))
            })
            .error
            .map(|e| e.to_string())
    };
    let copied = format!("{number}def text(value: str): return value\nsaved = take\ntake = text\n");
    let error = check(&format!("{copied}saved('x')\n"), "").unwrap();
    assert!(
        error.contains("main.py:5:7: Imre: expected int, got str"),
        "{error}"
    );
    assert!(check(&format!("{copied}take('x')\n"), "").is_none());
    let alias = "from scores import take\noriginal = take\nflag = True\n";
    let error = check(&format!("{alias}original(value=flag)\n"), number).unwrap();
    assert!(
        error.contains("main.py:4:16: Imre: expected int, got bool"),
        "{error}"
    );
    assert!(check(&format!("{alias}if False: original(value=flag)\n"), number).is_none());
    let read = "def consume():\n    exec(input('source: '))\n";
    let imported = "from scores import consume\n";
    let error = check(&format!("{imported}consume()\n"), read).unwrap();
    assert!(
        error.contains("scores.py:2:10") && error.contains("builtins.exec"),
        "{error}"
    );
    for caller in [
        "if False: consume()",
        "def ready(): return 7\nconsume.__code__ = ready.__code__\nconsume()",
    ] {
        assert!(
            check(&format!("{imported}{caller}\n"), read).is_none(),
            "{caller}"
        );
    }
    let compiled = "def consume():\n    exec(compile(input(), 'own.py', 'exec'))\n";
    let caller = "import scores\nscores.consume()\n";
    assert!(check(caller, compiled).unwrap().contains("scores.py:2:10"));
    assert!(check("import scores\n", compiled).is_none());
    assert!(check(caller, &format!("__builtins__ = 0\n{compiled}")).is_none());
    let generator = "def rows():\n    yield eval(input())\n";
    let caller = "from scores import rows\nfor value in rows(): pass\n";
    let message = check(caller, generator).unwrap();
    assert!(
        message.contains("scores.py:2:16")
            && message.contains("builtins.eval without a linked body"),
        "{message}"
    );
    let replaced = "import scores\ninstall_hook()\nfor value in scores.rows(): pass\n";
    assert!(check(replaced, generator).is_none());
    let docstring = "'application setup'\nimport scores\nif False:\n    scores.take('x')\n";
    assert!(
        check(docstring, number)
            .unwrap()
            .contains("main.py:4:17: Imre: expected int, got str")
    );
    let caller = "import scores\nif False:\n    scores.take('x')\n";
    assert!(
        check(caller, number)
            .unwrap()
            .contains("main.py:3:17: Imre: expected int, got str")
    );
    let variable = format!("{number}bad = 'x'\nif False:\n    take(bad)\n");
    assert!(
        check("import scores\n", &variable)
            .unwrap()
            .contains("scores.py:4:10: Imre: expected int, got str")
    );
    let imported = "import scores as selected\nbad = 'x'\nif 1 == 2:\n    selected.take(bad)\n";
    let inert = format!("{number}if False:\n    take(3)\n");
    assert!(
        check(imported, &inert)
            .unwrap()
            .contains("main.py:4:19: Imre: expected int, got str")
    );
    for expr in ["receipt = scores.take('x')", "print(scores.take('x'))"] {
        assert!(
            check(&format!("import scores\nif False:\n    {expr}\n"), number).is_some(),
            "{expr}"
        );
    }
    let heading = "import scores\nheading = 'value'\n";
    for call in [
        "print(heading, scores.take('x'))",
        "print(end=scores.take('x'))",
    ] {
        assert!(
            check(&format!("{heading}{call}\n"), number).is_some(),
            "{call}"
        );
        assert!(
            check(&format!("{heading}if False: {call}\n"), number).is_none(),
            "{call}"
        );
    }
    for expr in [
        "missing(scores.take('x'))",
        "print(observe(), scores.take('x'))",
        "print(end=scores.take('x'), *stop())",
        "print(**'not a mapping', end=scores.take('x'))",
    ] {
        assert!(
            check(&format!("import scores\nif False:\n    {expr}\n"), number).is_none(),
            "{expr}"
        );
    }
    assert!(check("install_hook()\nimport scores\n", &variable).is_none());
    assert!(
        check(
            "import scores\n",
            &format!("token = make()\n{number}token = None\ntake('x')\n")
        )
        .is_none()
    );
    for expr in [
        "scores.take('x', later())",
        "scores.take('x', value=7)",
        "scores.take('x', missing=0)",
        "scores.take('x', 0, 1)",
        "scores.take('x', *())",
        "scores.take(7)",
    ] {
        assert!(
            check(&format!("import scores\nif False:\n    {expr}\n"), number).is_none(),
            "{expr}"
        );
    }
    let mixed = "def take(value: int, /, scale: int = 1, *, label: str = '', **more): pass\n";
    let check_mixed = |args| {
        check(
            &format!("import scores\nif False:\n    scores.take({args})\n"),
            mixed,
        )
    };
    let first = check_mixed("1, label=3, scale='later'").unwrap();
    assert!(
        first.contains("main.py:3:26: Imre: expected str, got int"),
        "{first}"
    );
    for args in [
        "'x', value=2", // The extra positional-only name belongs to **more.
        "1, 2, scale='x'",
        "value='x'", // Missing the required positional argument.
        "1, label=3, scale=observe()",
        "'x', **{}",
    ] {
        assert!(check_mixed(args).is_none(), "{args}");
    }
    for prefix in [
        "setattr(scores, key, replacement)",
        "import patcher",
        "observe()",
    ] {
        assert!(
            check(
                &format!("import scores\n{prefix}\nif False:\n    scores.take('x')\n"),
                number
            )
            .is_none(),
            "{prefix}"
        );
    }
}

#[test]
fn a_linked_signature_requires_a_reaching_initializer_call() {
    let project = tempdir().unwrap();
    let root = project.path();
    let check = |main: &str, dependency: &str, declares: bool| {
        let sources = [
            ("job".to_owned(), root.join("job.py"), main.into()),
            ("rules".to_owned(), root.join("rules.py"), dependency.into()),
        ];
        super::check_registered_project(
            root,
            &[],
            sources.iter().enumerate().map(|(index, source)| {
                let mut registered = RegisteredSource::new(source, false, None);
                registered.initial = (index == 0).then_some(crate::dynamic::EntryStart::Immediate);
                registered.main_body = index == 0;
                registered.declares_signatures = index == 0 || declares;
                registered
            }),
            None,
            None,
            None,
            None,
        )
    };
    let read = "from __main__ import take as use\nif False: use('x')\n";
    let take = "def take(n: int): return n\n";
    let main = format!("{take}import rules\n");
    let child = |guard| {
        format!(
            "from __main__ import take\nheading = 'value'\nif {guard}: print(heading, end=take('wrong'))\n"
        )
    };
    let error = check(&main, &child("1 == 1"), true).unwrap_err();
    assert!(error.contains("rules.py:3:36"), "{error}");
    check(&main, &child("1 == 2"), true).unwrap();
    assert!(
        check(&format!("{take}import rules\n"), read, true)
            .unwrap_err()
            .contains("rules.py:2:15")
    );
    check(&format!("import rules\n{take}"), read, true).unwrap();
    check(
        "def __class__(n: int): pass\nfrom __main__ import __class__ as cls\ncls('child')\n",
        "",
        true,
    )
    .unwrap();
    let bad = "def take(n: int): return n\ntake('x')\n";
    let error = check("from rules import take\n", bad, true).unwrap_err();
    assert!(error.contains("rules.py:2:6"), "{error}");
    let from = "from rules import take as apply\nif False: apply('x')\n";
    let plain = "def take(n: int): return n\n";
    assert!(
        check(from, plain, true)
            .unwrap_err()
            .contains("job.py:2:17")
    );
    check(
        from,
        &format!("{plain}def __getattr__(k): return install_hook(k)\n"),
        true,
    )
    .unwrap();
    check("import rules\n", bad, false).unwrap();
    let declared = "class C: pass\ndef bad() -> C:\n    return 42\ndef good() -> C: return C()\n";
    let saved = "from rules import bad as read\nsaved = read\ndef read(): return 0\nsaved()\n";
    let error = check(saved, declared, false).unwrap_err();
    assert!(error.contains("rules.py:3:12"), "{error}");
    check("from rules import good as read\nread()\n", declared, false).unwrap();
    let observed = "import rules\nprint(type(rules.bad()).__name__)\n";
    let error = check(observed, declared, false).unwrap_err();
    assert!(error.contains("rules.py:3:12"), "{error}");
    check(&observed.replace("bad", "good"), declared, false).unwrap();
    check(&observed.replace("type(", "missing("), declared, false).unwrap();

    let caller = "from rules import bad\nbad()\n";
    let child = "class C: pass\ndef step(): raise LookupError()\ndef bad() -> C:\n    step()\n    return 42\n";
    check(caller, child, false).unwrap();
    for normal in ["pass", "raise LookupError(); yield 1"] {
        let error =
            check(caller, &child.replace("raise LookupError()", normal), false).unwrap_err();
        assert!(error.contains("rules.py:5:12"), "{error}");
    }

    check("install_hook()\nimport rules\n", bad, true).unwrap();
    check(
        "import rules\n",
        "def take(n: int, last): return n\ntake('x', raises())\n",
        true,
    )
    .unwrap();
}

#[test]
fn selected_startup_precedes_the_package_and_main_body() {
    let project = tempdir().unwrap();
    let root = project.path();
    let startup = root.join("external/sitecustomize.py");
    let check = |hook: &str, parent: &str, selected: &std::path::Path| {
        let sources = [
            ("sitecustomize".into(), startup.clone(), hook.into()),
            ("pkg".into(), root.join("pkg/__init__.py"), parent.into()),
            (
                "pkg.job".into(),
                root.join("pkg/job.py"),
                "from rules import take as apply\napply('x')\n".into(),
            ),
            (
                "rules".into(),
                root.join("rules.py"),
                "def take(n: int): return n\n".into(),
            ),
        ];
        super::check_registered_project(
            root,
            &[],
            sources.iter().enumerate().map(|(i, source)| {
                let mut registration = RegisteredSource::new(source, false, None);
                registration.declares_signatures = i != 0;
                registration.main_body = i == 2;
                registration.initial =
                    (i == 1).then_some(crate::dynamic::EntryStart::After(selected));
                registration
            }),
            None,
            None,
            None,
            None,
        )
    };
    let error = check("pass\n", "pass\n", &startup).unwrap_err();
    assert!(error.contains("job.py:2:7"), "{error}");
    check("raise SystemExit(19)\n", "pass\n", &startup).unwrap();
    check("pass\n", "raise SystemExit(13)\n", &startup).unwrap();
    check("pass\n", "pass\n", &root.join("other/sitecustomize.py")).unwrap();
}

#[test]
fn registered_program_checks_scalar_locals_across_suspension() {
    let project = tempdir().unwrap();
    let path = project.path().join("main.py");
    let source = "def rows():\n    yield 1\n    score: int = 'wrong'\n    yield score\n";
    let module = ("__main__".to_owned(), path, source.into());
    let error = super::check_registered_project(
        project.path(),
        &[],
        std::iter::once(RegisteredSource::new(&module, true, None)),
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("invalid-assignment") && error.contains("main.py:3:18"),
        "{error}"
    );
}

#[test]
fn registered_generators_require_a_linked_source_body() {
    let project = tempdir().unwrap();
    let path = project.path().join("main.py");
    let source = "def rows():\n    yield eval(input())\nfor row in rows(): print(row)\n";
    fs::write(&path, source).unwrap();
    let module = ("__main__".to_owned(), path, source.into());
    let error = super::check_registered_project(
        project.path(),
        &[],
        std::iter::once(RegisteredSource::new(&module, true, None)),
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("main.py:2:16") && error.contains("builtins.eval without a linked body"),
        "{error}"
    );
}

#[test]
fn a_builtin_namespace_lookup_only_requires_a_source_body_when_invoked() {
    let project = tempdir().unwrap();
    let path = project.path().join("main.py");
    let check = |body: &str| {
        fs::write(&path, format!("import builtins\n{body}\n")).unwrap();
        crate::check_file(&path, None)
    };
    let error = check(
        "def rows(f): yield f(input())\nfor row in rows(getattr(builtins, input())): print(row)",
    )
    .unwrap_err();
    assert!(
        error.contains("main.py:2:22") && error.contains("explicit table"),
        "{error}"
    );
    check("name=input()\nif name in ('len', 'str'): print(getattr(builtins,name)('abc'))").unwrap();
    check("print(getattr(builtins,input()).__name__)").unwrap();
    fs::write(
        project.path().join("inspect_callback.py"),
        "def name(callback):\n    if False: eval(input())\n    return callback.__name__\n",
    )
    .unwrap();
    check("from inspect_callback import name\nprint(name(getattr(builtins,input())))").unwrap();
}

#[test]
fn captured_generator_keeps_its_source_requirement_through_a_helper() {
    let project = tempdir().unwrap();
    let provider: crate::dynamic::ModuleSource<'_> = (
        "producer".to_owned(),
        project.path().join("producer.py"),
        "def rows():\n    yield eval(input())\n".into(),
    );
    fs::write(&provider.1, provider.2.as_bytes()).unwrap();
    let check = |tail: &str| {
        let entry: crate::dynamic::ModuleSource<'_> = (
            "__main__".to_owned(),
            project.path().join("main.py"),
            format!(
                "from producer import rows\ndef run():\n    for row in rows(): print(row)\n{tail}\nrun()\n"
            ).into(),
        );
        fs::write(&entry.1, entry.2.as_bytes()).unwrap();
        let mut main = RegisteredSource::new(&entry, true, None);
        main.initial = Some(crate::dynamic::EntryStart::Immediate);
        main.main_body = true;
        super::check_registered_project(
            project.path(),
            &[],
            [main, RegisteredSource::new(&provider, true, None)],
            None,
            None,
            None,
            None,
        )
    };
    let error = check("if False: globals()").unwrap_err();
    assert!(
        error.contains("producer.py:2:16") && error.contains("builtins.eval without a linked body"),
        "{error}"
    );
    check("def safe(): yield 7\nglobals()['rows'] = safe").unwrap();
}

#[test]
fn earlier_unlinked_source_precedes_a_later_initializer_argument() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    let check = |body: &str| {
        fs::write(&main, format!("def take(n: int): return n\n{body}\n")).unwrap();
        crate::check_file(&main, None).unwrap_err()
    };
    let first = check("exec(input(), globals())\nprint(take('x'))");
    assert!(
        first.contains("main.py:2:6") && first.contains("without a linked body"),
        "{first}"
    );
    let dormant = check("if False: exec(input(), globals())\nprint(take('x'))");
    assert!(dormant.contains("main.py:3:12"), "{dormant}");
    let rebound = check("def exec(*args): pass\nexec(input(), globals())\nprint(take('x'))");
    assert!(
        rebound.contains("main.py:3:15: Imre: unknown function or unsupported builtin arguments"),
        "{rebound}"
    );
    let input =
        check("def input(): return 'print(99)'\nexec(input(), globals())\nprint(take('x'))");
    assert!(input.contains("main.py:4:12"), "{input}");
    let code = check("def harmless(): pass\nexec(harmless.__code__, globals())\nprint(take('x'))");
    assert!(code.contains("main.py:4:12"), "{code}");
    let before = check("take('x')\nexec(input(), globals())\nprint(take('y'))");
    assert!(
        before.contains("main.py:2:6") && before.contains("expected int"),
        "{before}"
    );
}

#[test]
fn a_resolved_guarded_call_keeps_the_earlier_declaration_diagnostic() {
    let root = tempdir().unwrap();
    for (name, source) in [
        ("main.py", "import child\nimport opcode\n"),
        (
            "child.py",
            "from scalars import take\nheading = 'value'\nif 1 == 1: print(heading, end=take('wrong'))\n",
        ),
        (
            "scalars.py",
            "def declared() -> int: return 'separate'\ndef take(value: int): return value\n",
        ),
    ] {
        fs::write(root.path().join(name), source).unwrap();
    }
    let error = crate::check_file(&root.path().join("main.py"), None).unwrap_err();
    assert!(error.contains("scalars.py:1:31"), "{error}");
}

#[test]
fn the_ordinary_entry_keeps_an_operand_in_the_default_flow_region() {
    let directory = tempdir().unwrap();
    let entry = directory.path().join("app.py");
    fs::write(directory.path().join("observe.py"), "print('observed')\n").unwrap();
    fs::write(
        &entry,
        "import observe\nvalues: list[int] = [3]\nvalues.append(True)\n",
    )
    .unwrap();
    let mut graph = crate::modules::SourceGraph::load(&entry, Some(directory.path()), &[]).unwrap();
    let crate::CheckedProgram::Runtime(report) =
        crate::checked_lower_with(&mut graph, None, None).unwrap()
    else {
        panic!("expected the normal checker operand diagnostic")
    };
    let site = report.entry_operand().unwrap();
    assert!(report.function_site().is_none());
    assert!(
        graph
            .session
            .unconditional_module_operand(site.module, site.statement, site.operand)
    );
    assert!(
        report
            .to_string()
            .contains("app.py:3:15: Imre: expected int, got bool")
    );
}

#[test]
fn an_unavailable_provider_layout_preserves_the_callers_argument_contract() {
    let project = tempdir().unwrap();
    let installed = tempdir().unwrap();
    let package = installed.path().join("provider");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("model.py"), "class Result: pass\n").unwrap();
    fs::write(
        package.join("__init__.py"),
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from .model import Result\ntype Answer = Result\ndef get() -> Answer:\n    from .model import Result\n    return Result()\n",
    )
    .unwrap();
    let entry = project.path().join("app.py");
    let check = |argument: &str| {
        fs::write(
            &entry,
            format!("import provider\ndef take(n: int) -> int: return n\nprint(take({argument}))\nprint(provider.get())\n"),
        )
        .unwrap();
        crate::check_file_with_extensions(&entry, None, &[installed.path().to_owned()]).unwrap_err()
    };
    let error = check("'wrong'");
    assert!(error.contains("app.py:3:12"), "{error}");
    let error = check("1");
    assert!(error.contains("has no linked native layout"), "{error}");
}

#[test]
fn quoted_annotations_use_a_linked_class_reexported_for_type_checking() {
    let project = tempdir().unwrap();
    let root = project.path();
    fs::write(
        root.join("labels.py"),
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from model import Packet as Alias\n",
    )
    .unwrap();
    let main = root.join("main.py");
    fs::write(
        &main,
        "import labels\nfrom model import Packet\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from labels import Alias\ndef get() -> 'Alias':\n    return Packet()\ndef keep(value: 'Alias') -> 'Alias':\n    return value\nkeep(get())\n",
    )
    .unwrap();
    for model in ["class Packet: pass\n", "class Packet(ValueError): pass\n"] {
        fs::write(root.join("model.py"), model).unwrap();
        let mut graph = crate::modules::SourceGraph::load(&main, Some(root), &[]).unwrap();
        assert!(matches!(
            crate::checked_lower(&mut graph).unwrap(),
            crate::CheckedProgram::Native(_)
        ));
    }
}

#[test]
fn a_guarded_class_annotation_can_use_its_locally_imported_implementation() {
    let app = tempdir().unwrap();
    let site = tempdir().unwrap();
    let pkg = site.path().join("pkg");
    fs::create_dir(&pkg).unwrap();
    let entry = app.path().join("main.py");
    let entry_source = "import pkg\nprint(pkg.get().value())\n";
    fs::write(&entry, entry_source).unwrap();
    let check = || crate::check_file_with_extensions(&entry, None, &[site.path().to_owned()]);
    let factory = |argument: &str| {
        let source = format!(
            r#"from typing import TYPE_CHECKING, Optional
if TYPE_CHECKING:
    from .model import Item
_current: Optional['Item'] = None
def get() -> 'Item':
    global _current
    if _current is None:
        from .model import Item
        _current = Item({argument})
    return _current
"#
        );
        fs::write(pkg.join("__init__.py"), source).unwrap();
    };
    let model = pkg.join("model.py");
    fs::write(
        &model,
        concat!(
            "class Item:\n    def __init__(self, width: int = 1): pass\n",
            "    def value(self): return 42\n"
        ),
    )
    .unwrap();
    factory("");
    check().unwrap();

    factory("width='wrong'");
    let error = check().unwrap_err();
    assert!(
        error.contains("invalid-argument-type") && error.contains("__init__.py:9:25"),
        "{error}"
    );
    factory("");

    fs::write(
        &entry,
        format!("{entry_source}\ndef invalid() -> None:\n    status: int = True\n"),
    )
    .unwrap();
    let error = check().unwrap_err();
    assert!(
        error.contains("main.py:5:19: Imre: expected int, got bool"),
        "{error}"
    );
    fs::write(&entry, entry_source).unwrap();

    let init = pkg.join("__init__.py");
    let provider = fs::read_to_string(&init).unwrap();
    fs::write(&init, format!("{provider}cache: int = 'x'\n")).unwrap();
    let error = check().unwrap_err();
    assert!(
        error.contains("__init__.py:11:") && error.contains("int"),
        "{error}"
    );
    factory("");

    let stub = model.with_extension("pyi");
    fs::write(&stub, "class Item:\n    def value(self) -> int: ...\n").unwrap();
    let error = check().unwrap_err();
    assert!(
        error.contains("model.pyi:1:7 has no linked native layout"),
        "{error}"
    );

    fs::remove_file(stub).unwrap();
    fs::write(
        &model,
        "class Item:\n    def value(self): return eval(input())\n",
    )
    .unwrap();
    let error = check().unwrap_err();
    assert!(
        error.contains("model.py:1:7 has no linked native layout"),
        "{error}"
    );
}

#[test]
fn a_guarded_module_checks_arguments_after_its_callees_settle() {
    let project = tempdir().unwrap();
    let root = project.path();
    let pkg = root.join("pkg");
    fs::create_dir(&pkg).unwrap();
    fs::write(pkg.join("model.py"), "class Item: pass\n").unwrap();
    fs::write(
        pkg.join("__init__.py"),
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from .model import Item\ndef get() -> 'Item':\n    from .model import Item\n    return Item()\n",
    )
    .unwrap();
    let entry = root.join("main.py");
    let source = concat!(
        "import pkg\n",
        "def takes(a: int, b: int) -> None: pass\n",
        "def stop() -> int: raise ValueError()\n",
        "def indirect() -> int: return stop()\n",
        "try:\n    takes(ARGS)\nexcept ValueError:\n    pass\n",
    );
    fs::write(&entry, source.replace("ARGS", "indirect(), True")).unwrap();
    crate::check_file(&entry, Some(root)).unwrap();
    fs::write(&entry, source.replace("ARGS", "True, indirect()")).unwrap();
    let error = crate::check_file(&entry, Some(root)).unwrap_err();
    assert!(
        error.contains("main.py:6:11: Imre: expected int, got bool"),
        "{error}"
    );
}

#[test]
fn a_guarded_builtin_alias_uses_the_selected_stub() {
    let project = tempdir().unwrap();
    let root = project.path();
    let app = root.join("app.py");
    let stub = root.join("labels.pyi");
    fs::write(root.join("labels.py"), "Result = int\n").unwrap();
    fs::write(
        &app,
        "from typing import TYPE_CHECKING\n\
         if TYPE_CHECKING:\n    from labels import Result as int\n\
         def sample() -> int:\n    raise KeyError(4)\n    return 41\n\
         type Batch = list[int]\n\
         def first(items: Batch): return items[0]\n",
    )
    .unwrap();
    fs::write(&stub, "Result = int\n").unwrap();
    crate::check_file(&app, Some(root)).unwrap();
    fs::write(&stub, "Result = str\n").unwrap();
    let error = crate::check_file(&app, Some(root)).unwrap_err();
    assert!(
        error.contains("app.py:6:12: Imre: expected str, got int"),
        "{error}"
    );
}

#[test]
fn a_written_guarded_alias_requires_an_exported_member() {
    let project = tempdir().unwrap();
    let root = project.path();
    let app = root.join("app.py");
    let stub = root.join("labels.pyi");
    fs::write(root.join("model.py"), "class Packet(str): pass\n").unwrap();
    fs::write(
        root.join("labels.py"),
        "raise AssertionError('guarded provider ran')\n",
    )
    .unwrap();
    fs::write(&stub, "from model import Packet as Result\n").unwrap();
    fs::write(
        &app,
        r#"from model import Packet
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    from labels import Result as int
def sample(value: int) -> int:
    return value
print(sample(Packet("chosen")))
"#,
    )
    .unwrap();
    let named = crate::CompileOptions {
        module_entry: Some("app".into()),
        script_startup: Some((Default::default(), "imre".into())),
        extension_paths: vec![root.to_owned()],
        ..Default::default()
    };
    for error in [
        crate::check_file(&app, Some(root)).unwrap_err(),
        crate::compile_file_with_boxed_units(&app, Some(root), &named)
            .err()
            .unwrap(),
    ] {
        assert!(
            error.contains("app.py:4:24")
                && error.contains("Module `labels` has no member `Result`"),
            "{error}"
        );
    }
    fs::write(
        &stub,
        "from model import Packet as Result\n__all__ = ['Result']\n",
    )
    .unwrap();
    crate::check_file(&app, Some(root)).unwrap();
}

#[test]
fn a_guarded_reexport_resolves_to_its_linked_class_without_running_the_module() {
    let project = tempdir().unwrap();
    let root = project.path();
    let package = root.join("pkg");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("__init__.py"), "").unwrap();
    fs::write(package.join("model.py"), "class Packet: pass\n").unwrap();
    fs::write(package.join("other.py"), "class Packet: pass\n").unwrap();
    fs::write(
        package.join("labels.py"),
        "from .model import Packet\nprint('guarded initializer')\n",
    )
    .unwrap();
    let entry = root.join("main.py");
    let source = "from pkg.model import Packet\nfrom pkg.other import Packet as Other\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from pkg.labels import Packet as Alias\ndef get() -> 'Alias':\n    return Packet()\nget()\n";
    fs::write(&entry, source).unwrap();
    let mut graph = crate::modules::SourceGraph::load(&entry, Some(root), &[]).unwrap();
    assert!(graph.python_file("pkg.labels").is_none());
    assert!(matches!(
        crate::checked_lower(&mut graph).unwrap(),
        crate::CheckedProgram::Native(_)
    ));
    let returning_other = source.replace("return Packet()", "return Other()");
    fs::write(&entry, &returning_other).unwrap();
    let error = crate::check_file(&entry, Some(root)).unwrap_err();
    assert!(
        error.contains("7:12: Imre: expected pkg.model.Packet, got pkg.other.Packet"),
        "{error}"
    );

    let stub = package.join("labels.pyi");
    fs::write(&stub, "from .other import Packet as Packet\n").unwrap();
    let named = |root: &std::path::Path| crate::CompileOptions {
        module_entry: Some("main".into()),
        script_startup: Some((Default::default(), "imre".into())),
        extension_paths: vec![root.to_owned()],
        ..Default::default()
    };
    let check_named = |spelling: &std::path::Path| {
        let app = spelling.join("main.py");
        let options = named(spelling);
        fs::write(&entry, source).unwrap();
        let error = crate::compile_file_with_boxed_units(&app, Some(spelling), &options)
            .err()
            .expect("the guarded stub requires pkg.other.Packet");
        assert!(
            error.contains("main.py:7:12")
                && error.contains("expected `pkg.other.Packet`, found `pkg.model.Packet`"),
            "{error}"
        );
        fs::write(&entry, &returning_other).unwrap();
        let (compiled, _) =
            crate::compile_file_with_boxed_units(&app, Some(spelling), &options).unwrap();
        (error, compiled.source_inputs)
    };
    check_named(root);
    check_named(&package.join(".."));
    #[cfg(unix)]
    {
        let shadow = tempdir().unwrap();
        let other = shadow.path().join("pkg");
        fs::create_dir(&other).unwrap();
        for name in ["__init__.py", "model.py", "other.py", "labels.py"] {
            fs::copy(package.join(name), other.join(name)).unwrap();
        }
        fs::write(shadow.path().join("main.py"), source).unwrap();
        fs::write(
            other.join("labels.pyi"),
            "from .model import Packet as Packet\n",
        )
        .unwrap();
        let link = shadow.path().join("link");
        std::os::unix::fs::symlink(&package, &link).unwrap();
        let (error, inputs) = check_named(&link.join(".."));
        assert!(
            !error.contains(shadow.path().file_name().unwrap().to_str().unwrap()),
            "{error}"
        );
        assert!(
            inputs
                .declared_file_resolutions
                .contains(&(link.join(".."), Some(root.canonicalize().unwrap()),))
        );
        assert!(inputs.fresh());
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(!inputs.fresh());
    }

    let mut graph = crate::modules::SourceGraph::load(&entry, Some(root), &[]).unwrap();
    let alternate = tempdir().unwrap();
    fs::write(alternate.path().join("labels.pyi"), "class Packet: ...\n").unwrap();
    let mut observed = crate::SourceInputs::default();
    let mut fs = crate::watch::FileSystem::default();
    for (implementation, matches) in [
        (package.join("../pkg/labels.py"), true),
        (alternate.path().join("labels.py"), false),
    ] {
        assert_eq!(
            graph
                .session
                .guarded_stub(
                    &implementation,
                    &graph.inputs.declaration_files,
                    &mut observed,
                    &mut fs,
                )
                .is_some(),
            matches,
            "{}",
            implementation.display()
        );
    }
    assert!(observed.fresh());
    let (inputs, paths) = graph.disk_inputs(None);
    assert!(paths.contains(&stub.canonicalize().unwrap()));
    assert!(inputs.fresh());
    crate::check_file(&entry, Some(root)).unwrap();
    crate::compile_file_with_boxed_units(&entry, Some(root), &named(root)).unwrap();
    fs::write(&stub, "from .model import Packet as Packet\n").unwrap();
    assert!(!inputs.fresh());
    let error = crate::check_file(&entry, Some(root)).unwrap_err();
    assert!(
        error.contains("7:12: Imre: expected pkg.model.Packet, got pkg.other.Packet"),
        "{error}"
    );
}

#[test]
fn quoted_method_result_keeps_the_class_scope_binding() {
    use ruff_text_size::Ranged;

    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    fs::write(
        project.path().join("model.py"),
        "class Packet: pass\nclass Other: pass\n",
    )
    .unwrap();
    let source = r#"from model import Packet, Other
from model import Other as Alias
class Factory:
    Alias = Packet
    def get(self) -> "Alias":
        return Packet()
print(Factory().get() is None)
"#;
    fs::write(&main, source).unwrap();
    let mut graph = crate::modules::SourceGraph::load(&main, Some(project.path()), &[]).unwrap();
    let class = graph.modules[graph.entry].parsed.syntax().body[2]
        .as_class_def_stmt()
        .unwrap();
    let get = class.body[1].as_function_def_stmt().unwrap();
    let result = get.returns.as_deref().unwrap();
    assert!(
        graph
            .session
            .annotation_class(graph.entry, result)
            .is_some()
    );

    // Even the same literal at the same offsets inside decoded text has no lexical scope.
    let padding = " ".repeat(usize::from(result.start()) - "list[".len());
    let decoded = format!("list[{padding}\"Alias\"]");
    let parsed = ruff_python_parser::parse_expression(&decoded).unwrap();
    let detached = &parsed.expr().as_subscript_expr().unwrap().slice;
    assert_eq!(detached.range(), result.range());
    assert!(
        graph
            .session
            .annotation_class(graph.entry, detached)
            .is_none()
    );
    crate::checked_lower(&mut graph).unwrap();
    fs::write(&main, source.replace("return Packet()", "return Other()")).unwrap();
    let error = crate::check_file(&main, None).unwrap_err();
    assert!(
        error.contains("main.py:6:16: Imre: expected model.Packet, got model.Other"),
        "{error}"
    );
}

#[test]
fn run_path_can_enter_through_implicit_builtins() {
    let project = tempdir().unwrap();
    let root = project.path();
    let sources = [
        (
            "__main__".into(),
            root.join("app.py"),
            "run_path(input())\n".into(),
        ),
        (
            "__builtins__".into(),
            root.join("__builtins__.pyi"),
            "from runpy import run_path as run_path\n".into(),
        ),
    ];
    let error = super::check_registered_project(
        root,
        &[],
        sources.iter().map(|s| RegisteredSource::new(s, true, None)),
        Some(&|_| false),
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(error.contains("open executable filename"), "{error}");
}

#[test]
fn python_instances_keep_their_declared_class_in_fields_and_containers() {
    let project = tempdir().unwrap();
    let main = project.path().join("main.py");
    fs::write(
        project.path().join("model.py"),
        "class Item:
    def __init__(self, value): self.value = value
class Child(Item): pass
class Other:
    def __init__(self, value): self.value = value
class Box:
    def __init__(self):
        self.item: Item = Item(0)
        self.maybe: Item | None = None
    @property
    def current(self) -> list[Item]:
        return [self.item]
    @current.setter
    def current(self, replacement: Item):
        self.item = replacement
    def read(self):
        return self.item
    def write(self, replacement: Item):
        self.item = replacement
    from_functions = property(read, write)
type Items = list[Item]
",
    )
    .unwrap();
    let imports = "from model import Box, Child, Item, Items, Other\n";
    for (body, rule) in [
        ("box = Box()\nbox.item = Other(1)\n", "invalid-assignment"),
        (
            "box = Box()\nbox.from_functions = Other(1)\n",
            "invalid-assignment",
        ),
        (
            "box = Box()\nbox.current = Other(1)\n",
            "invalid-assignment",
        ),
        (
            "box = Box()\nbox.current.append(Other(1))\n",
            "invalid-argument-type",
        ),
        (
            "values: Items = [Item(0)]\nvalues.append(Other(1))\n",
            "invalid-argument-type",
        ),
        (
            "values: list[Item] = [Item(0)]\nvalues[0] = Other(1)\n",
            "invalid-assignment",
        ),
    ] {
        fs::write(&main, format!("{imports}{body}")).unwrap();
        let error = crate::check_file(&main, None).unwrap_err();
        assert!(error.contains(rule), "{error}");
    }
    fs::write(
        &main,
        format!("{imports}box = Box()\nbox.item = Child(1)\nbox.current = Child(1)\nbox.from_functions = Child(2)\nbox.current.append(Child(2))\nbox.maybe = Child(1)\nif box.maybe is not None: box.maybe.value = 2\nvalues: Items = [Item(0)]\nvalues.append(Child(2))\n"),
    )
    .unwrap();
    crate::check_file(&main, None).unwrap();
}

#[test]
fn a_live_any_argument_does_not_prove_a_scalar_layout() {
    use super::ArgumentContract;
    use crate::modules::{DefinitionId, SourceGraph};
    use ruff_python_ast::Stmt;
    use ruff_text_size::Ranged;

    let project = tempdir().unwrap();
    let path = project.path().join("app.py");
    fs::write(
        &path,
        "from typing import Any\ndef take(label: str) -> None: pass\ndef run(value: Any):\n    take(value)\n    take('label')\n    take(1)\n",
    )
    .unwrap();
    let graph = SourceGraph::load(&path, None, &[]).unwrap();
    let types = graph.session.operand_types(graph.entry);
    let parsed = types.parsed();
    let [_, Stmt::FunctionDef(take), Stmt::FunctionDef(run)] = parsed.suite().as_slice() else {
        panic!("expected take and run")
    };
    let target = DefinitionId {
        module: graph.entry,
        statement: 1,
    };
    let annotation = take.parameters.args[0].annotation().unwrap().range();
    let contract = |statement: &Stmt| {
        let call = statement
            .as_expr_stmt()
            .unwrap()
            .value
            .as_call_expr()
            .unwrap();
        types.argument_contract(&call.arguments.args[0], target, 0, annotation)
    };
    assert!(matches!(
        contract(&run.body[0]),
        Some(ArgumentContract::Dynamic)
    ));
    assert!(matches!(
        contract(&run.body[1]),
        Some(ArgumentContract::Checked)
    ));
    assert!(contract(&run.body[2]).is_none());
}

#[test]
fn only_the_builtin_inventory_supplies_a_string_tuple() {
    let source = "from sys import builtin_module_names as names\ndef inventory() -> tuple[str, ...]: return names\nprint(len(inventory()))\n";
    crate::check(source).unwrap();
    assert!(crate::check(&source.replace("tuple[str", "tuple[int")).is_err());
    assert!(crate::check(&source.replace("builtin_module_names", "argv")).is_err());
}
