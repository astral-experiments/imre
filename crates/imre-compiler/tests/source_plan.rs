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

#[test]
fn a_named_comprehension_checks_its_conditional_capture_before_the_filter() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("main.py"),
        r#"def accepts(value: int) -> bool:
    return bool(value)
def rows(items: list[int], flag: bool) -> list[str]:
    lookup: dict[int, int] = {item: item for item in items}
    if flag:
        word = "kept"
    return [word for item in items if accepts("bad")]
print(rows([], False))
"#,
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_imre"))
        .current_dir(project.path())
        .args(["-B", "-m", "main"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("expected int, got str; annotations are enforced contracts"),
        "{error}"
    );
    assert!(error.contains("main.py:7:47"), "{error}");
}

#[test]
fn eager_filters_use_the_checked_result_of_the_outer_call() {
    let source = r#"
def options() -> list[str]:
    return ["alpha", "beta"]
def text() -> str:
    return "ab"
def choose(prefix: str) -> list[str]:
    return [s for s in options() if s.startswith(prefix)]
def characters(keys: set[str]) -> set[str]:
    return {c for c in text() if c in keys}
"#;
    let llvm = compile(source, &CompileOptions::default()).unwrap();
    for kernel in ["imre_str_list_filter", "imre_filter_character_set"] {
        assert!(llvm.contains(&format!("call ptr @{kernel}(")), "{kernel}");
    }
}

#[test]
fn a_deferred_nullable_parameter_keeps_the_imported_call_and_source_contract() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("sink.py"),
        "def send(value):\n    print(value)\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("provider.py"),
        r#"from __future__ import annotations
from collections.abc import Sequence
import sys

def forward(argv: Sequence[str] | None = None) -> None:
    from sink import send
    send(argv or sys.argv[1:])
"#,
    )
    .unwrap();
    let main = root.path().join("main.py");
    std::fs::write(&main, "from provider import forward\nforward(('ok',))\n").unwrap();
    check_file(&main, Some(root.path())).unwrap();
    std::fs::write(&main, "from provider import forward\nforward(42)\n").unwrap();
    let error = check_file(&main, Some(root.path())).unwrap_err();
    assert!(error.contains("main.py:2:9"), "{error}");
    assert!(error.contains("Expected `Sequence[str] | None`"), "{error}");
}

#[test]
fn an_incoming_union_keeps_its_written_argument_contract() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.py");
    let header = "from collections.abc import Sequence\n\
                  def forward(value: Sequence[str] | int | None) -> None:\n    print(value)\n";
    std::fs::write(&main, format!("{header}forward(('ok',))\n")).unwrap();
    check_file(&main, Some(root.path())).unwrap();
    std::fs::write(&main, format!("{header}forward(1.5)\n")).unwrap();
    let error = check_file(&main, Some(root.path())).unwrap_err();
    assert!(error.contains("main.py:4:9"), "{error}");
    assert!(
        error.contains("Expected `Sequence[str] | int | None`"),
        "{error}"
    );
}

#[test]
fn a_bottom_return_keeps_the_python_call_and_the_written_contract() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.py");
    let child = root.path().join("child.py");
    std::fs::write(
        &main,
        "from child import stop\ndef consume(value: int) -> int: return value + 1\nprint(consume(stop()))\n",
    )
    .unwrap();
    let header = "import sys\nfrom typing import Never as End\ndef stop() -> End:\n";
    std::fs::write(&child, format!("{header}    return sys.exit(7)\n")).unwrap();
    check_file(&main, None).unwrap();
    compile_file(&main, None, &CompileOptions::default()).unwrap();

    for (body, location) in [("return 42", "4:12"), ("pass", "3:15")] {
        std::fs::write(&child, format!("{header}    {body}\n")).unwrap();
        let error = check_file(&main, None).unwrap_err();
        assert!(error.contains(&format!("child.py:{location}")), "{error}");
        assert!(error.contains("Never"), "{error}");
    }
}

#[test]
fn source_layout_requests_are_independent_of_unrelated_imports() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("app.py");
    let factory =
        "def factory(n):\n    def f(x): return n + x\n    return f\nprint(factory(2)(3))\n";
    for body in [
        factory,
        "def show(x): return x\nprint(show(1), show('a'))\n",
        "class A:\n    def show(self, x): return x\na = A()\nprint(a.show(1), a.show('a'))\n",
        "def choose(x): return 42 if x else 'other'\nprint(choose(True))\n",
        "def change():\n    x = 1\n    x = str(x)\n    return x\nprint(change())\n",
        "i = 1\nfor i in 's': print(i)\n",
        "i = 's'\nfor i in range(2): print(i)\n",
        "x = 1\npair = ('s', 2)\nx, y = pair\nprint(x)\n",
        "x = 1\nprint(x := 's')\n",
        "def f(x):\n    x = 's'\n    return x\nprint(f(1))\n",
        "x = 1\ndef read(): return x + '!'\ndef store(v):\n    global x\n    x = v\nstore('s')\nprint(read())\n",
        "def f(x=1): return x\nprint(f(), f('s'), f())\n",
        "def f(x=1): return x + '!'\nprint(f('s'))\n",
    ] {
        for imports in ["", "import unittest\n"] {
            std::fs::write(&file, format!("{imports}{body}")).unwrap();
            check_file(&file, None).unwrap();
        }
    }
    std::fs::write(&file, factory).unwrap();
    let options = CompileOptions::default();
    assert!(
        compile_file(&file, None, &options)
            .unwrap()
            .source_inputs
            .boxed_imports
    );
    std::fs::write(&file, "def inc(n): return n + 1\nprint(inc(2))\n").unwrap();
    assert!(
        !compile_file(&file, None, &options)
            .unwrap()
            .source_inputs
            .boxed_imports
    );
}

#[test]
fn an_inferred_frame_does_not_hide_declared_operands_or_unlinked_source() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("app.py");
    let body = "def factory(n):\n    def f(x): return n + x\n    return f\nprint(factory(2)(3))\n";
    for (invalid, diagnostic) in [
        (
            "def typed(x: int): return x\nprint(typed('bad'))\n",
            "expected int, got str",
        ),
        (
            "class A:\n    def show(self, x: int): return x\nprint(A().show('bad'))\n",
            "expected int, got str",
        ),
        (
            "def wrong() -> int: return 'bad'\n",
            "expected int, got str",
        ),
        (
            "def wrong():\n    x: int = 1\n    x = 'bad'\n",
            "expected int, got str",
        ),
        ("exec(input())\n", "without a linked body"),
    ] {
        std::fs::write(&file, format!("{body}{invalid}")).unwrap();
        let error = check_file(&file, None).unwrap_err();
        assert!(error.contains(diagnostic), "{error}");
    }
}

#[test]
fn materialized_pipelines_keep_the_callback_parameter_contract() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pipeline.py");
    for import in ["", "import unittest\n"] {
        for body in [
            "print(list(map(f, [True])))",
            "alias = f\nprint(list(filter(alias, (True,))))",
            "callbacks = (f,)\nprint(list(map(callbacks[0], [True])))",
            "from typing import Callable\ncb: Callable[[int], int] = f\nprint(list(map(cb, [True])))",
        ] {
            let source = format!("{import}def f(x: int) -> int: return x + 1\n{body}\n");
            check_callback_refusal(&path, &source, "True");
        }
    }
}

#[test]
fn pipeline_context_does_not_impose_an_inferred_or_unpulled_contract() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pipeline.py");
    for body in [
        "def run():\n    def map(cb, values): return values\n    return list(map(f, [True]))\nprint(run())",
        "def identity(x): return x\nprint(list(map(identity, [1])), list(map(identity, [True])))",
        "print(list(map(f, [0])) == [True])",
        "values = [True]\nlazy = map(f, values)\nvalues.clear()\nprint(list(lazy))",
        "print(list(zip([], map(f, [True]))))",
        "print(any(map(f, [1, True])))",
        "if False: print(list(map(f, [True])))",
    ] {
        for prefix in ["", "import unittest\n"] {
            let source = format!("{prefix}def f(x: int) -> int: return x + 1\n{body}\n");
            std::fs::write(&path, &source).unwrap();
            check_file(&path, None).unwrap_or_else(|error| panic!("{source}\n{error}"));
        }
    }
}

#[test]
fn unplaceable_dependencies_cannot_hide_checked_operands() {
    let project = tempfile::tempdir().unwrap();
    let app = project.path().join("main.py");
    std::fs::write(project.path().join("other.py"), "print(bytearray())\n").unwrap();
    let mut compiler = imre_compiler::IncrementalCompiler::default();
    for body in [
        "i: int\nfor i in [True]: print(i)\n",
        "def values() -> dict[str, list[int]]: return {'flag': [True]}\nprint(values())\n",
        "def value(flag) -> int: return True if flag else 1\nprint(value(True))\n",
    ] {
        for prefix in ["", "import unittest\n", "import other\n"] {
            let source = format!("{prefix}{body}");
            std::fs::write(&app, &source).unwrap();
            for result in [check_file(&app, None), compiler.check_file(&app, None)] {
                let error = result.expect_err(&source);
                assert!(
                    error.contains("expected int, got bool"),
                    "{source}\n{error}"
                );
                assert!(error.contains("main.py:"), "{error}");
            }
        }
    }
}

#[test]
fn source_checks_preserve_the_direct_integer_sign_convention() {
    let project = tempfile::tempdir().unwrap();
    let app = project.path().join("main.py");
    let declaration = "def positive(delta: float) -> bool: return delta > 0\n";
    for prefix in ["", "import unittest\n"] {
        std::fs::write(
            &app,
            format!("{prefix}{declaration}print(positive(10 ** 400))\n"),
        )
        .unwrap();
        check_file(&app, None).unwrap();
        for call in ["print(positive(True))", "saved = positive\nprint(saved(1))"] {
            std::fs::write(&app, format!("{prefix}{declaration}{call}\n")).unwrap();
            let error = check_file(&app, None).expect_err(call);
            assert!(error.contains("expected float, got"), "{call}\n{error}");
        }
    }
}

/// Check the same operand in a script and a named module, including its source position.
fn check_callback_refusal(path: &std::path::Path, source: &str, operand: &str) {
    std::fs::write(path, source).unwrap();
    let module = path.file_stem().unwrap().to_str().unwrap();
    let (line, column) = source
        .lines()
        .enumerate()
        .find_map(|(i, s)| s.find(operand).map(|j| (i + 1, j + 1)))
        .unwrap();
    let expected = format!(
        "{module}.py:{line}:{column}: Imre: expected int, got bool; annotations are enforced contracts"
    );
    let error = check_file(path, None).expect_err(source);
    assert!(error.ends_with(&expected), "{error}");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_imre"))
        .current_dir(path.parent().unwrap())
        .args(["-m", module])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{source}");
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.trim_end().ends_with(&expected), "{error}");
}

#[test]
fn bound_callbacks_keep_the_written_operand_contract() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("callbacks.py");
    for import in ["", "import unittest\n"] {
        for (body, operand) in [
            ("print(sorted([True, False], key=key))", "sorted"),
            ("def apply(fn): return fn(True)\nprint(apply(key))", "True"),
            (
                "from functools import partial\ng = partial(key, True)\nprint(g())",
                "True",
            ),
        ] {
            let source = format!("{import}def key(x: int): return x\n{body}\n");
            check_callback_refusal(&path, &source, operand);
        }
    }
    for (body, operand) in [
        (
            "callbacks = (key,)\nprint(sorted((True,), key=callbacks[0]))",
            "sorted",
        ),
        (
            "def call(n, fn): return fn(True)\nprint(call(fn=key, n=1))",
            "True",
        ),
        (
            "def call(fn): return fn(True)\ncb = key\nprint(call(cb))",
            "True",
        ),
        (
            "from functools import partial\ndef add(x: int, y: int): return x + y\ng = partial(add, 1, True)\nh = g\nprint(h())",
            "True",
        ),
    ] {
        let source = format!("import unittest\ndef key(x: int): return x\n{body}\n");
        check_callback_refusal(&path, &source, operand);
    }
}

#[test]
fn dynamic_and_deferred_callbacks_do_not_inherit_another_targets_contract() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("callbacks.py");
    for body in [
        "def sorted(xs, key): return xs\nprint(sorted([True], key=key))",
        "xs = [True]\nxs[:] = [1]\nprint(sorted(xs, key=key) == [True])",
        "keys = [key]\nkeys[0] = pred\nprint(sorted([True], key=keys[0]))",
        "def apply(fn, x): return fn(x)\nprint(apply(key, 1), apply(pred, True))",
        "def apply(fn, flag): return fn(True) if flag else fn(1)\nprint(apply(key, False), apply(pred, True))",
        "def apply(fn):\n    fn = pred\n    return fn(True)\nprint(apply(key))",
        "def apply(fn): return fn(True)\nprint([apply(key) for _ in range(0)])",
        "def apply(fn): yield fn(True)\ng = apply(key)",
        "g = partial(key, True)\nprint(g.args)",
        "g = partial(key, True)\ng.__setstate__((pred, (True,), {}, None))\nprint(g())",
        "g = partial(key, True)\nif False: print(g())",
    ] {
        let source = format!(
            "import unittest\nfrom functools import partial\ndef key(x: int): return x\ndef pred(x: bool): return x\n{body}\n"
        );
        std::fs::write(&path, &source).unwrap();
        check_file(&path, None).unwrap_or_else(|error| panic!("{source}\n{error}"));
    }
}

#[test]
fn inferred_stores_preserve_fixed_bindings_and_nested_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("app.py");
    for body in [
        "i: int = 1\nfor i in 's': print(i)\n",
        "x: int = 1\npair = ('s', 2)\nx, y = pair\n",
        "x: int = 1\nprint(x := 's')\n",
        "x: int = 1\ndef store():\n    global x\n    x = 's'\nstore()\n",
        "def f(x: int = 1): return x\nprint(f('s'))\n",
        "def f(x: int):\n    x = 's'\n    return x\nprint(f(1))\n",
        "def typed(n: int): return str(n)\nx = 1\nprint(x := typed('s'))\n",
    ] {
        std::fs::write(&file, body).unwrap();
        let error = check_file(&file, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }
    std::fs::write(
        &file,
        "class A:\n    def f(self):\n        self = 's'\n        return self\nA().f()\n",
    )
    .unwrap();
    let error = check_file(&file, None).unwrap_err();
    assert!(error.contains("expected app.A, got str"), "{error}");
}
