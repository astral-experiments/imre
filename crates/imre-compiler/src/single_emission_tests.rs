use super::*;
use std::cell::{Cell, RefCell};

enum Phase {
    Prepared(PathBuf, bool),
    Llvm,
}

thread_local! {
    static PHASES: RefCell<Option<Vec<Phase>>> = const { RefCell::new(None) };
}

fn observe_preparation<T>(run: impl FnOnce() -> T) -> (T, Vec<Phase>) {
    PHASES.set(Some(Vec::new()));
    let result = run();
    (result, PHASES.take().unwrap())
}

pub(crate) fn record_preparation(path: &Path, borrowed: bool) {
    PHASES.with_borrow_mut(|events| {
        if let Some(events) = events {
            events.push(Phase::Prepared(path.to_owned(), borrowed));
        }
    });
}

pub(crate) fn record_llvm_start() {
    PHASES.with_borrow_mut(|events| {
        if let Some(events) = events {
            events.push(Phase::Llvm);
        }
    });
}

#[derive(Clone)]
pub(crate) struct RegisteredOutputStarted;

impl Default for RegisteredOutputStarted {
    fn default() -> Self {
        record_llvm_start();
        Self
    }
}

fn assert_source_diagnostic(actual: &str, file: &Path, expected: &str) {
    let path = actual
        .strip_suffix(expected)
        .unwrap_or_else(|| panic!("expected {}{expected}, got {actual}", file.display()));
    assert_eq!(
        Path::new(path).canonicalize().unwrap(),
        file.canonicalize().unwrap()
    );
}

fn assert_refusal_before_llvm(
    entry: &Path,
    file: &Path,
    expected: &str,
    builds: &[CompileOptions],
) {
    let (diagnostic, events) = observe_preparation(|| check_file(entry, None).unwrap_err());
    assert_source_diagnostic(&diagnostic, file, expected);
    assert!(!events.iter().any(|event| matches!(event, Phase::Llvm)));
    for options in builds {
        let (diagnostic, events) =
            observe_preparation(|| compile_file(entry, None, options).err().unwrap());
        assert_source_diagnostic(&diagnostic, file, expected);
        assert!(!events.iter().any(|event| matches!(event, Phase::Llvm)));
    }
}

#[test]
fn imported_operand_refusal_precedes_llvm_emission() {
    let project = tempfile::tempdir().unwrap();
    let main = project.path().join("main.py");
    let source = project.path().join("source.py");
    let client = include_str!("../../../tests/fixtures/mix_template/main.py");
    let body = include_str!("../../../tests/fixtures/mix_template/source.py");
    std::fs::write(&main, client).unwrap();
    std::fs::write(&source, body).unwrap();
    let flags = CommandFlags {
        frozen_modules: true,
        ..Default::default()
    };
    let script = CompileOptions {
        script_startup: Some((flags.clone(), "imre".into())),
        ..Default::default()
    };
    let named = CompileOptions {
        module_entry: Some("main".into()),
        extension_paths: vec![project.path().to_owned()],
        script_startup: Some((flags, "imre".into())),
        ..Default::default()
    };
    assert_refusal_before_llvm(
        &main,
        &source,
        ":4:15: cannot compile this comprehension ahead of time (through 4:57)",
        &[CompileOptions::default(), script, named],
    );

    std::fs::write(&source, body.replace("await get(x)", "x")).unwrap();
    for build in [false, true] {
        let ((), events) = observe_preparation(|| {
            if build {
                compile_file(&main, None, &CompileOptions::default()).unwrap();
            } else {
                check_file(&main, None).unwrap();
            }
        });
        // Both physical files use their original parse exactly once, before any output.
        let prepared = events
            .iter()
            .position(|event| matches!(event, Phase::Llvm))
            .unwrap_or(events.len());
        assert!(
            events[prepared..]
                .iter()
                .all(|event| matches!(event, Phase::Llvm))
        );
        assert_eq!(prepared < events.len(), build);
        for file in [&main, &source] {
            let identity = file.canonicalize().unwrap();
            let observed: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Phase::Prepared(path, borrowed)
                        if path.canonicalize().ok().as_ref() == Some(&identity) =>
                    {
                        Some(*borrowed)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(observed, [true], "preparations of {}", file.display());
        }
    }
}

#[test]
fn conditional_operands_refuse_only_on_reachable_edges() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("subject.py");
    let class = include_str!("../../../tests/fixtures/class_boolean_refused.py");
    let assertion = include_str!("../../../tests/fixtures/assertion_message_refused.py");
    for (source, reached, dead, line, from, to) in [
        (class, "enabled and", "False and", 12, 33, 75),
        (assertion, "assert False,", "assert True,", 5, 22, 64),
    ] {
        std::fs::write(&entry, source).unwrap();
        assert_refusal_before_llvm(
            &entry,
            &entry,
            &format!(
                ":{line}:{from}: cannot compile this comprehension ahead of time (through {line}:{to})"
            ),
            &[CompileOptions::default()],
        );
        std::fs::write(&entry, source.replace(reached, dead)).unwrap();
        check_file(&entry, None).unwrap();
        compile_file(&entry, None, &CompileOptions::default()).unwrap();
    }
}

#[test]
fn indirect_literal_operand_refusal_precedes_llvm_emission() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("main.py");
    let body = include_str!("../../../tests/fixtures/mix_template/source.py");
    let caller = |child: &str| {
        format!(
            concat!(
                "import sys\nsys.path\n",
                "def run():\n    exec({}, globals())\n",
                "globals()['run']()\n",
                "print('build' in globals())\n",
            ),
            serde_json::to_string(child).unwrap(),
        )
    };
    let nested = format!("exec({}, globals())", serde_json::to_string(body).unwrap());
    for (child, prefix) in [(body, ""), (nested.as_str(), "<string>:1:6: Imre: ")] {
        std::fs::write(&entry, caller(child)).unwrap();
        assert_refusal_before_llvm(
            &entry,
            &entry,
            &format!(
                ":4:10: Imre: {prefix}<string>:4:15: cannot compile this comprehension ahead of time (through 4:57)"
            ),
            &[CompileOptions::default()],
        );
    }

    let caller = caller(&nested);
    let after_return = caller.replacen("    exec(", "    return\n    exec(", 1);
    std::fs::write(&entry, after_return).unwrap();
    check_file(&entry, None).unwrap();
    compile_file(&entry, None, &CompileOptions::default()).unwrap();
}

#[test]
fn imported_checked_function_retains_its_literal_call_site() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("entry.py");
    let rules = project.path().join("rules.py");
    std::fs::write(&entry, "import rules\nrules.invoke()\n").unwrap();
    std::fs::write(
        &rules,
        "def invoke() -> None:\n    exec(\"exec(input())\", {})\n",
    )
    .unwrap();
    assert_refusal_before_llvm(
        &entry,
        &rules,
        concat!(
            ":2:10: Imre: <string>:1:6: Imre: this call may select builtins.exec ",
            "without a linked body for its source. Define the handlers as Python functions ",
            "and select one by name, or declare the finite modules with --include-module."
        ),
        &[CompileOptions::default()],
    );

    std::fs::write(
        &rules,
        "def invoke() -> None:\n    exec(\"exec('result = 42')\", {})\n",
    )
    .unwrap();
    let compilation = compile_file(&entry, None, &CompileOptions::default()).unwrap();
    assert_eq!(
        compilation
            .llvm_ir
            .matches("call i64 @imre_dyn_exec_operand(")
            .count(),
        2,
    );
}

#[test]
fn imported_function_captures_its_defining_builtins() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("main.py");
    let rules = project.path().join("rules.py");
    std::fs::write(
        &entry,
        "import sys\nsys.path\nimport rules\nrules.consume()\n",
    )
    .unwrap();

    for (module, linked_calls) in [
        (
            concat!(
                "def consume():\n",
                "    scope = {}\n",
                "    exec('value = 41', scope)\n",
                "    return scope['value']\n",
                "__builtins__ = {'exec': print}\n",
            ),
            1,
        ),
        (
            "__builtins__ = {'exec': print}\ndef consume():\n    exec('bad !!!')\n",
            0,
        ),
    ] {
        std::fs::write(&rules, module).unwrap();
        let program = compile_file(&entry, None, &CompileOptions::default()).unwrap();
        assert_eq!(
            program
                .llvm_ir
                .matches("call i64 @imre_dyn_exec_operand(")
                .count(),
            linked_calls,
            "{module}",
        );
    }
}

#[test]
fn suspended_assignment_refusal_precedes_llvm_emission() {
    let project = tempfile::tempdir().unwrap();
    let main = project.path().join("subject.py");
    let augmented = include_str!("../../../tests/fixtures/registered_suspended_assignment.py");
    let assignment = augmented.replace("values +=", "values =");
    for (body, col, end) in [(augmented, 15, 54), (assignment.as_str(), 14, 53)] {
        std::fs::write(&main, body).unwrap();
        assert_refusal_before_llvm(
            &main,
            &main,
            &format!(
                ":12:{col}: cannot compile this comprehension ahead of time (through 12:{end})"
            ),
            &[CompileOptions::default()],
        );
    }
}

#[test]
fn writing_function_defaults_preserves_later_checked_errors() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("defaults.py");
    let source = r#"def answer(value: int = 9) -> int:
    return value

def run() -> None:
    alias = answer
    alias.__defaults__ = (17,)
    wrong: int = "late"
    print(answer())

run()
"#;
    std::fs::write(&path, source).unwrap();
    assert_refusal_before_llvm(
        &path,
        &path,
        ":7:18: Imre: expected int, got str; annotations are enforced contracts",
        &[CompileOptions::default()],
    );
    std::fs::write(&path, source.replace(r#""late""#, "17")).unwrap();
    compile_file(&path, None, &CompileOptions::default()).unwrap();
}

#[test]
fn bound_wrapper_emits_once_with_its_fixed_callable_layout() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("wrapper.py");
    let wrapper = include_str!("../../../tests/fixtures/callables/bound_methods.py");

    for source in [
        wrapper.to_owned(),
        format!("import sys\n{wrapper}\nprint(sys.version_info.major)\n"),
    ] {
        std::fs::write(&path, source).unwrap();
        let (compilation, phases) =
            observe_preparation(|| compile_file(&path, None, &CompileOptions::default()).unwrap());
        assert_eq!(
            phases
                .iter()
                .filter(|phase| matches!(phase, Phase::Llvm))
                .count(),
            1
        );
        assert!(!compilation.source_inputs.boxed_imports);
        assert!(
            compilation
                .llvm_ir
                .contains("call void @imre_function_set_wrapped(")
        );
        assert!(
            compilation
                .llvm_ir
                .contains("call ptr @imre_function_get_wrapped(")
        );
    }
}

#[test]
fn a_file_build_records_the_root_of_its_registered_package() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let package = root.join("parcel");
    std::fs::create_dir(&package).unwrap();
    let initializer = package.join("__init__.py");
    std::fs::write(&initializer, "def program(): return 112\n").unwrap();
    let entry = package.join("__main__.py");
    std::fs::write(
        &entry,
        "from parcel import program\nbuffer = bytearray()\nbuffer.append(program())\nprint(buffer)\n",
    )
    .unwrap();

    let compilation = compile_file(&entry, None, &CompileOptions::default()).unwrap();
    let inputs = compilation.source_inputs;
    assert!(inputs.boxed_imports);
    assert!(inputs.files.iter().any(|(path, _, _)| path == &initializer));
    assert!(inputs.fresh());
    std::fs::write(root.join("__init__.py"), "").unwrap();
    assert!(!inputs.fresh());
}

#[test]
fn fixed_file_selects_its_product_for_the_requested_startup() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("app.py");
    std::fs::write(
        &path,
        "def add(a: int, b: int) -> int:\n    return a + b\n\nprint(add(int(input()), 2))\n",
    )
    .unwrap();

    for sitecustomize in [false, true] {
        let options = CompileOptions {
            sitecustomize,
            ..Default::default()
        };
        let observed = Cell::new(false);
        object_fallback(
            CompilationMode::Check,
            ProgramEntry::File(&path, None),
            &options,
            Default::default(),
            false,
            |graph, call_error| {
                let product = checked_lower_with(graph, None, call_error)?;
                if sitecustomize {
                    assert!(matches!(product, CheckedProgram::Registered(None)));
                } else {
                    assert!(matches!(product, CheckedProgram::Native(_)));
                }
                observed.set(true);
                Ok(product)
            },
        )
        .unwrap();
        assert!(observed.get());
    }
}

#[test]
fn required_object_call_keeps_its_admission_reason() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("app.py");
    std::fs::write(
        &path,
        "def render(data: list[int] | None, spec: str) -> str:\n    return f'{data:{spec}}'\n\nprint(render([17, 19], input()))\n",
    )
    .unwrap();
    let mut graph = modules::SourceGraph::load(&path, None, &[]).unwrap();
    let reason = match checked_lower(&mut graph).unwrap() {
        CheckedProgram::Registered(Some(reason)) => reason,
        CheckedProgram::Runtime(reason) => panic!("unexpected lowering refusal: {reason}"),
        _ => panic!("the checked formatting operation requires a registered entry"),
    };
    let expected = ":2:14: Imre: this f-string field requires native Python object operations";
    assert_eq!(reason.kind, LoweringKind::Ordinary);
    assert_source_diagnostic(&reason.to_string(), &path, expected);
    assert_source_diagnostic(&check_file(&path, None).unwrap_err(), &path, expected);
}

#[test]
fn interleaved_import_preserves_another_modules_list_contract() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("app.py");
    let scores = project.path().join("scores.py");
    std::fs::write(&entry, "import library\nimport scores\n").unwrap();
    std::fs::write(
        project.path().join("library.py"),
        "print('loaded')\ndef later() -> int:\n    return 2\n",
    )
    .unwrap();
    std::fs::write(
        &scores,
        "def score() -> int:\n    values: list[int] = [1]\n    values.append(True)\n    return values[0]\n",
    )
    .unwrap();
    let expected = ":3:19: Imre: expected int, got bool; annotations are enforced contracts";
    assert_source_diagnostic(&check_file(&entry, None).unwrap_err(), &scores, expected);
    let options = CompileOptions {
        sitecustomize: true,
        ..Default::default()
    };
    assert_source_diagnostic(
        &compile_file(&entry, None, &options).err().unwrap(),
        &scores,
        expected,
    );
}

#[test]
fn eager_comprehension_exports_only_guaranteed_assignments() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("main.py");
    for (display, bound) in [
        ("[(alias := value) for _ in [1]]", true),
        ("[False for _ in [1] if (alias := value) is None]", true),
        ("[(alias := value) for _ in []]", false),
        ("[(alias := value) for _ in [1] if False]", false),
    ] {
        let source = format!(
            "def measure():\n    value = [2]\n    {display}\n    print(alias is value)\n\nmeasure()\n"
        );
        std::fs::write(&entry, source).unwrap();
        let result = check_file(&entry, None);
        if bound {
            result.unwrap();
        } else {
            assert_source_diagnostic(
                &result.unwrap_err(),
                &entry,
                ":4:11: Imre: local may be uninitialized on this path",
            );
        }
    }
}

const DORMANT_SOCKET_WORKER: &str = r#"import socket
import threading

import scores

def receive(stream):
    with stream as source:
        print(source.recv(2))

if False:
    reader, writer = socket.socketpair()
    writer.sendall(b"Rx")
    task = threading.Thread(target=receive, args=(reader,))
    task.start()
    task.join()
    writer.close()
scores.measure()
"#;

#[test]
fn dormant_thread_preserves_the_declared_call_diagnostic_across_edits() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("worker_unused.py");
    let scores = project.path().join("scores.py");
    std::fs::write(&entry, DORMANT_SOCKET_WORKER).unwrap();
    let source = "def take(value: int) -> int:\n    return value\n\ndef measure() -> int:\n    return take('x')\n";
    let expected = ":5:17: Imre: expected int, got str; annotations are enforced contracts";
    let mut compiler = IncrementalCompiler::default();
    std::fs::write(&scores, source).unwrap();
    assert_refusal_before_llvm(&entry, &scores, expected, &[CompileOptions::default()]);
    assert_source_diagnostic(
        &compiler.check_file(&entry, None).unwrap_err(),
        &scores,
        expected,
    );

    let mixed = format!("{source}\ndef other() -> int:\n    return 'unrelated'\n");
    std::fs::write(&scores, mixed).unwrap();
    let diagnostic = compiler.check_file(&entry, None).unwrap_err();
    assert_eq!(check_file(&entry, None).unwrap_err(), diagnostic);
    assert!(
        diagnostic.starts_with("error[invalid-return-type]"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("scores.py:8:12"), "{diagnostic}");
    assert!(!diagnostic.contains(expected), "{diagnostic}");

    std::fs::write(&scores, source.replace("take('x')", "take(3)")).unwrap();
    compiler.check_file(&entry, None).unwrap();
    check_file(&entry, None).unwrap();
}

#[test]
fn same_argument_span_retains_the_other_declared_parameter_error() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("worker_unused.py");
    std::fs::write(&entry, DORMANT_SOCKET_WORKER).unwrap();
    std::fs::write(
        project.path().join("api.py"),
        concat!(
            "def take_int(value: int) -> int:\n    return value\n",
            "\ndef take_bytes(value: bytes) -> int:\n    return 7\n",
        ),
    )
    .unwrap();
    std::fs::write(
        project.path().join("scores.py"),
        concat!(
            "from typing import TYPE_CHECKING\nfrom api import take_int as take\n",
            "if TYPE_CHECKING:\n    from api import take_bytes as take\n",
            "\ndef measure() -> int:\n    return take('x')\n",
        ),
    )
    .unwrap();

    let diagnostic = check_file(&entry, None).unwrap_err();
    assert_eq!(
        diagnostic.matches("scores.py:7:17").count(),
        2,
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("scores.py:7:17: Imre: expected int, got str"),
        "{diagnostic}"
    );
    assert_eq!(
        diagnostic.matches("error[invalid-argument-type]").count(),
        1,
        "{diagnostic}"
    );
    assert!(diagnostic.contains("Expected `bytes`"), "{diagnostic}");
    assert!(diagnostic.contains("api.py:4:5"), "{diagnostic}");
}

#[test]
fn dormant_thread_keeps_an_imported_modules_exact_types() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("main.py");
    let scores = project.path().join("scores.py");
    std::fs::write(
        &entry,
        concat!(
            "import socket\nimport threading\nimport scores\n",
            "def receive(stream):\n    with stream as source:\n        print(source.recv(2))\n",
            "if False:\n    reader, writer = socket.socketpair()\n",
            "    threading.Thread(target=receive, args=(reader,)).start()\n",
            "scores.measure()\n",
        ),
    )
    .unwrap();
    for (source, position) in [
        (
            "def take(value: int) -> int:\n    return value\n\ndef measure() -> int:\n    return take(True)\n",
            "5:17",
        ),
        (
            "def measure() -> int:\n    values: list[int] = [1]\n    values.append(True)\n    return values[0]\n",
            "3:19",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    values[0] = True\n    print(values)\n",
            "3:17",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    pair = (True, 2)\n    values[0], n = pair\n",
            "4:5",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    alias = values\n    pair = (True, 2)\n    alias[0], n = pair\n",
            "5:5",
        ),
        (
            concat!(
                "def value(seq: list[int]) -> bool:\n    return True\n",
                "def measure():\n    fresh = [1]\n    declared: list[int] = [2]\n",
                "    alias = fresh\n    alias[0] = value(alias := declared)\n",
            ),
            "7:16",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    values['a'] = True\n",
            "3:19",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    pair = ('a', 2)\n    values[True], n = pair\n",
            "4:12",
        ),
        (
            "def measure():\n    values: list[int] = [True]\n    print(values)\n",
            "2:26",
        ),
        (
            "def take(values: list[int]):\n    return values\n\ndef measure():\n    print(take([True]))\n",
            "5:17",
        ),
        (
            "def measure():\n    values: dict[int, str] = {1: 'one'}\n    values[True] = 'new'\n",
            "3:12",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    alias = values\n    alias.append(True)\n",
            "4:18",
        ),
        (
            "def measure():\n    values: list[int] = [1]\n    def mutate():\n        values.append(True)\n    mutate()\n",
            "4:23",
        ),
    ] {
        std::fs::write(&scores, source).unwrap();
        let error = format!(
            ":{position}: Imre: expected int, got bool; annotations are enforced contracts"
        );
        assert_refusal_before_llvm(&entry, &scores, &error, &[CompileOptions::default()]);
    }
    for source in [
        "def measure():\n    value = 1\n    value = 'ready'\n    print(value)\n",
        "def measure():\n    values = [1]\n    values.append(True)\n    print(values)\n",
        "def measure():\n    values = [1]\n    pair = (True, 2)\n    values[0], n = pair\n",
        concat!(
            "def measure():\n    values: list[int] = [1]\n    alias = values\n    alias = [2]\n",
            "    pair = (True, 2)\n    alias[0], n = pair\n",
        ),
        concat!(
            "def value(seq: list[int]) -> bool:\n    return True\n",
            "def measure():\n    declared: list[int] = [1]\n    fresh = [2]\n",
            "    alias = declared\n    alias[0] = value(alias := fresh)\n",
        ),
        concat!(
            "def measure():\n    typed: list[int] = [1]\n    other = [2]\n    alias = typed\n",
            "    for alias in [other]:\n        pass\n    alias.append(True)\n",
        ),
        concat!(
            "def measure():\n    typed: list[int] = [1]\n    other = [2]\n    alias = typed\n",
            "    try:\n        alias = other\n    finally:\n        alias.append(True)\n",
        ),
        concat!(
            "import sys\ndef measure():\n    declared: list[int] = [1]\n    inferred = [2]\n",
            "    if sys.argv[-1] == 'typed':\n        alias = declared\n",
            "    else:\n        alias = inferred\n    alias.append(True)\n",
        ),
        concat!(
            "def measure():\n    values: list[int] = [1]\n    other = [2]\n    alias = values\n",
            "    def change():\n        alias.append(True)\n    alias = other\n    change()\n",
        ),
    ] {
        std::fs::write(&scores, source).unwrap();
        check_file(&entry, None).unwrap();
    }
}

#[test]
fn retained_generator_propagates_its_nested_definition_refusal() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("main.py");
    let source = concat!(
        "async def get(x):\n",
        "    return x\n",
        "\n",
        "def values():\n",
        "    yield 0\n",
        "    async def build():\n",
        "        return t\"{[lambda v=await get(x): v for x in [1, 2]]}\"\n",
        "    yield build\n",
        "\n",
        "for item in values():\n",
        "    print(type(item).__name__)\n",
        "state: int\n",
    );
    std::fs::write(&entry, source).unwrap();
    let expected = ":7:19: cannot compile this comprehension ahead of time (through 7:61)";
    assert_refusal_before_llvm(&entry, &entry, expected, &[CompileOptions::default()]);
}
