"""Prove the runtime roles of all five upstream overload cases and one warning client."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, native_method, run_evidence, testcase_fixture
from module_fixture import check_modules, check_rejections
from registry_transport import HOST_TYPING, HOST_WARNING, Native, bind, worker_source

TYPING = "test.test_typing"
CLASS = "OverloadTests"
SOURCE = HERE / "upstream/Lib/test/test_typing.py"
WARNINGS = HERE.parent / "warnings"
WARN_SOURCE = WARNINGS / "upstream/Lib/test/test_warnings/__init__.py"


def source_digest(path, owner, name):
    data = path.read_bytes()
    cls, = (c for c in ast.parse(data).body if isinstance(c, ast.ClassDef) and c.name == owner)
    fn, = (f for f in cls.body if isinstance(f, ast.FunctionDef) and f.name == name)
    begin = min([fn.lineno, *(d.lineno for d in fn.decorator_list)])
    return hashlib.sha256(b"".join(data.splitlines(keepends=True)[begin - 1:fn.end_lineno])).hexdigest()


def build_body(lang, root, name):
    fixture = "from typing import overload, get_overloads\n" + testcase_fixture()
    if name == "test_overload_registry_repeated":
        # BaseTestCase's list equality compares the original functions by identity.
        # Keep the original helper unmodified; only the native assertion is adapted.
        fixture = """from typing import overload, get_overloads
class NativeCase:
    def assertEqual(self, first, expected) -> None:
        assert len(first) == len(expected)
        for index in range(len(first)):
            assert first[index] is expected[index]
"""
        fixture += "\n" + method_bytes(SOURCE, CLASS, "set_up_overloads").decode()
    directory = root / name
    directory.mkdir()
    case_id = f"{TYPING}.{CLASS}.{name}"
    execute = native_method(lang, directory, SOURCE, CLASS, case_id, fixture,
                            operation=("imre", "native overload registration, retrieval or singleton call"))
    return case_id, execute


def original_method(executable, module, owner, name, level, source, *, fault=False):
    import typing
    digest = source_digest(source, owner, name)

    def execute(case, original):
        native = Native(executable)
        native.fault = fault
        scope = typing._overload_registry
        try:
            with bind(module, native):
                original()  # Includes original @patch/@cpython_only decorators, if any.
            assert scope is typing._overload_registry, "upstream mock patch leaked"
            calls = native.counts()
            assert calls.get("register", 0) or calls.get("register-builtin", 0)
            assert calls.get("get", 0) or calls.get("get-builtin", 0)
            return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": {"operation": "persistent native overload registry, original function identity/order and metadata",
                                 "fixture": "CPython function objects, unittest assertions and decorators; native callback handles tied to original code sites",
                                 "method_sha256": digest, "opt_level": level, "native_calls": dict(sorted(calls.items()))}}
        finally:
            native.close()
    return execute


def product(lang, root, source):
    directory = root / source.stem
    directory.mkdir()
    executable = lang.build(source.read_bytes(), directory)
    def run(cmd):
        p = subprocess.run(cmd, capture_output=True, timeout=20)
        return p.returncode, p.stdout, p.stderr
    expected = run([sys.executable, source])
    actual = run([executable])
    if expected[0] or expected[2] or actual != expected:
        raise AssertionError((source.name, expected, actual))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--cpython-source", type=Path)
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        p.error("run with the pinned test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    from test import test_typing, test_warnings
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    warning_manifest = json.loads((WARNINGS / "UPSTREAM.json").read_text())
    if warning_manifest["commit"] != manifest["commit"]:
        raise ValueError("typing and its warnings client must use the same upstream revision")
    verify_files(WARNINGS / "upstream", warning_manifest["files"])
    verify_interpreter_tests(warning_manifest)
    reference = json.loads((HERE / "reference-linux.json").read_text())
    reports, clients = [], {}
    for level in (0, 3):
        lang = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
        with TemporaryDirectory() as temp:
            root = Path(temp)
            work = root / "worker"
            work.mkdir()
            worker = lang.build(worker_source(), work)
            for fixture in ("function_attributes.py", "module_overloads.py", "call_original.py", "call_noargs.py"):
                product(lang, root, HERE / fixture)
            check_modules(lang, root)
            if level == 0:
                check_rejections(lang, root)
            methods = dict(build_body(lang, root, name) for name in
                ("test_overload_fails", "test_overload_succeeds", "test_overload_registry_repeated"))
            for method in HOST_TYPING:
                methods[f"{TYPING}.{CLASS}.{method}"] = original_method(
                    worker, test_typing, CLASS, method, level, SOURCE)
            report = run_evidence(load_suite(TYPING), methods, reference, level)
            for row in report["records"]:
                if row["id"] in methods and row["outcome"] != "pass":
                    raise AssertionError(row)
            assert report["denominator"] == 712
            reports.append(report)
            cls = test_warnings.DeprecatedTests
            case = cls(HOST_WARNING)
            callback = original_method(worker, test_warnings, cls.__name__, HOST_WARNING, level, WARN_SOURCE)
            # run_in_fixtures preserves unittest's original complete lifecycle.
            from suite_native import run_in_fixtures
            observed, failures, skipped = run_in_fixtures(
                load_suite("test.test_warnings"), {case.id(): callback})
            if failures or skipped or len(observed[case.id()]) != 1:
                raise AssertionError((observed, failures, skipped))
            clients[str(level)] = observed[case.id()][0]
            # A broken native answer must break the real assertion. Reversing
            # a multi-overload result exposes both order and marker dependence.
            for mod, owner, method, src in ((test_typing, test_typing.OverloadTests, HOST_TYPING[0], SOURCE),
                                            (test_warnings, cls, HOST_WARNING, WARN_SOURCE)):
                target = owner(method)
                callback = original_method(worker, mod, owner.__name__, method, level, src, fault=True)
                _, failures, skipped = run_in_fixtures(load_suite(mod.__name__), {target.id(): callback})
                if not failures or skipped:
                    raise AssertionError(("incorrect native ordering failed to trip assertions", target.id()))
            print(f"-O{level}: {report['counts']}; original warning client and two fault witnesses passed", flush=True)
    write_reports(args.result, reports, reference)
    args.result.with_name("typing-warnings-client.json").write_text(json.dumps(clients, indent=2) + "\n")


if __name__ == "__main__":
    main()
