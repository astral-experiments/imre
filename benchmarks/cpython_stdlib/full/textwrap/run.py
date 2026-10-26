"""Run CPython's unmodified textwrap methods and record where each part executed."""
import argparse
import ast
from collections import Counter
from contextlib import ExitStack
import hashlib
import importlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes
from wrapper_cases import shorten_case, wrapper_cases

TESTS = HERE / "upstream/Lib/test/test_textwrap.py"
OWNERS = ("DedentTestCase", "IndentTestCase")


def native_source(observed):
    data = TESTS.read_bytes()
    lines = data.splitlines(keepends=True)
    classes = {node.name: node for node in ast.parse(data).body if isinstance(node, ast.ClassDef)}
    source = b"import unittest\nfrom textwrap import dedent, indent\n"
    for name in OWNERS:
        cls = classes[name]
        first_method = next(node for node in cls.body if isinstance(node, ast.FunctionDef))
        # Compile the original class header and fixtures, including the tuple concatenation.
        source += b"\n" + b"".join(lines[cls.lineno - 1:first_method.lineno - 1])
        if name == "DedentTestCase":
            source += method_bytes(TESTS, name, "assertUnchanged")
        for method in observed[name]:
            source += b"\n" + method_bytes(TESTS, name, method)
    return source + b"\nif __name__ == '__main__':\n    unittest.main()\n"


def run_native_cases(tools, directory, module, methods):
    source = native_source(methods)
    binary = tools.build(source, directory)
    expected = {f"{method} (__main__.{cls}.{method}) ... ok" for cls in methods for method in methods[cls]}
    def run(binary):
        return subprocess.run([binary, "-v"], text=True, capture_output=True, timeout=60)
    result = run(binary)
    lines = result.stderr.splitlines()
    if (result.returncode or result.stdout or {s for s in lines if " ... " in s} != expected
            or sum(" ... " in s for s in lines) != len(expected) or lines[-1:] != ["OK"]):
        raise AssertionError((result.returncode, result.stdout, result.stderr))
    # Prove that the original assertions reject incorrect product results, in both classes.
    bad = source.replace(b"from textwrap import dedent, indent", b"""def dedent(text: str) -> str:
    return "wrong"
def indent(text: str, prefix: str, predicate: None = None) -> str:
    return "wrong" """)
    wrong = run(tools.build(bad, directory))
    if (wrong.returncode != 1 or wrong.stdout
            or {s for s in wrong.stderr.splitlines() if s.endswith(" ... FAIL")}
            != {s.removesuffix("ok") + "FAIL" for s in expected}):
        raise AssertionError(("wrong product controls", wrong.returncode, wrong.stdout, wrong.stderr))
    results = {}
    for cls, names in methods.items():
        for name in names:
            fixture = "Original assertUnchanged helper and native unittest.TestCase/main"
            operation = "textwrap.dedent"
            if cls == "IndentTestCase":
                fixture = "Original class tuple declarations and native unittest.TestCase/main"
                operation = "textwrap.indent and textwrap.dedent" if name.startswith("test_roundtrip_") else "textwrap.indent"
            results[(f"{module.__name__}.{cls}.{name}", 1)] = {
                "outcome": "pass",
                "roles": roles(body="imre", assertions="imre", operation="imre",
                               fixture="imre"),
                "evidence": {"operation": operation,
                             "fixture": fixture, "opt_level": tools.level,
                             "method_sha256": hashlib.sha256(method_bytes(TESTS, cls, name)).hexdigest()},
            }
    return results


def callback_worker(lambdas):
    # Compile the exact expressions from the pinned methods into the public call boundary.
    src = """import sys
from textwrap import indent, dedent

def main() -> None:
    tag = sys.argv[1]
    try:
        if tag == "int":
            print(repr(dedent(int(sys.argv[2]))))
        elif tag == "bytes":
            print(repr(dedent(bytes([int(n) for n in sys.argv[2:]]))))
"""
    for line, expr in lambdas.items():
        src += f"        elif tag == '{line}':\n            print(repr(indent(sys.argv[2], sys.argv[3], {expr})))\n"
    return (src + """        else:
            raise ValueError("unknown callback")
    except TypeError as exc:
        print("!TypeError: " + str(exc))
main()
""").encode()


def run_host_cases(module, binary, reference, native, lambdas, level):
    suite = load_suite(module.__name__)
    discovered = [(c.id(), type(c).__module__ + "." + type(c).__qualname__) for c in cases(suite)]
    counts = Counter()
    class Results(unittest.TestResult):
        current = None
        def startTest(self, test):
            super().startTest(test)
            self.current = test.id()
        def stopTest(self, test):
            self.current = None
            super().stopTest(test)
    results = Results()
    def invoke(*args):
        res = subprocess.run([binary, *args], capture_output=True, text=True, check=True, timeout=30)
        if res.stderr:
            raise AssertionError(res.stderr)
        counts[results.current] += 1
        return res.stdout.removesuffix("\n")
    def indent(text, prefix, predicate=None):
        if predicate is None or (results.current, 1) in native:
            return textwrap.indent(text, prefix, predicate)
        line = predicate.__code__.co_firstlineno
        if Path(predicate.__code__.co_filename).resolve() != Path(module.__file__).resolve() or line not in lambdas:
            raise AssertionError("predicate did not come from the pinned original test")
        actual = ast.literal_eval(invoke(str(line), text, prefix))
        if type(actual) is not str or actual != textwrap.indent(text, prefix, predicate):
            raise AssertionError((text, prefix, line, actual))
        return actual
    def dedent(text):
        if type(text) is str:
            return textwrap.dedent(text)
        args = (str(text),) if type(text) is int else tuple(map(str, text))
        actual = invoke(type(text).__name__, *args)
        try:
            textwrap.dedent(text)
        except TypeError as exc:
            if actual != "!TypeError: " + str(exc):
                raise AssertionError((actual, str(exc))) from exc
            raise TypeError(actual.partition(": ")[2]) from None
        raise AssertionError(("unexpected reference success", text))
    with ExitStack() as stack:
        for name, fn in (("indent", indent), ("dedent", dedent)):
            stack.enter_context(patch.object(module, name, fn))
        suite.run(results)
    if results.testsRun != reference["denominator"] or results.skipped or not results.wasSuccessful():
        raise AssertionError((results.testsRun, results.skipped, results.errors, results.failures))
    for name, count in counts.items():
        assert (name, 1) not in native
        native[(name, 1)] = {
            "outcome": "pass", "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
            "evidence": {"operation": "textwrap.dedent" if name.endswith("test_type_error") else "textwrap.indent",
                         "fixture": "Unmodified CPython method and class; arguments sent to native public function",
                         "native_calls": count, "opt_level": level},
        }
    return build_report(reference, level, discovered, native)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    if sys.implementation.name != "cpython" or manifest["tag"] != "v" + ".".join(map(str, sys.version_info[:3])):
        raise ValueError("use the pinned CPython")
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(textwrap.__file__).parent, {"textwrap.py": manifest["files"]["Lib/textwrap.py"]})
    verify_interpreter_tests(manifest)
    module = importlib.import_module(manifest["module"])
    ref = json.loads((HERE / "reference-linux.json").read_bytes())
    source = TESTS.read_text()
    methods = {}
    lambdas = {}
    for cls in ast.parse(source).body:
        if not isinstance(cls, ast.ClassDef) or cls.name not in OWNERS:
            continue
        methods[cls.name] = []
        for method in cls.body:
            if not isinstance(method, ast.FunctionDef) or not method.name.startswith("test_"):
                continue
            callbacks = [n for n in ast.walk(method) if isinstance(n, ast.Lambda)]
            if cls.name == "IndentTestCase" and callbacks:
                lambdas.update((node.lineno, ast.get_source_segment(source, node)) for node in callbacks)
            elif (cls.name, method.name) != ("DedentTestCase", "test_type_error"):
                methods[cls.name].append(method.name)
    reports = []
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            native = run_native_cases(tools, directory, module, methods)
            wrapped = wrapper_cases(module, tools, directory, ref["denominator"])
            if native.keys() & wrapped.keys():
                raise AssertionError("duplicate execution evidence")
            key, complete = shorten_case(module, tools, directory, TESTS)
            if key not in wrapped:
                raise AssertionError("the original placeholder case did not consume native operations")
            wrapped[key] = complete
            native.update(wrapped)
            worker = tools.build(callback_worker(lambdas), directory)
            report = run_host_cases(module, worker, ref, native, lambdas, level)
            edges = HERE / "edge_cases.py"
            expected = subprocess.check_output([sys.executable, "-I", edges], timeout=30)
            actual = subprocess.run([tools.build(edges.read_bytes(), directory)], check=True,
                                    capture_output=True, timeout=30)
            if actual.stderr or actual.stdout != expected:
                raise AssertionError((level, expected, actual.stdout, actual.stderr))
            print(f"O{level}: {report['denominator']} original methods: {report['counts']}", flush=True)
            reports.append(report)
    write_reports(args.result, reports, ref)


if __name__ == "__main__":
    main()
