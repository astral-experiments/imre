"""Build native programs and run complete pinned test methods under their fixtures."""

import ast
import dataclasses
from functools import wraps
import hashlib
from pathlib import Path
import subprocess
import unittest

from suite_inventory import cases


@dataclasses.dataclass(frozen=True)
class Toolchain:
    imre: Path
    runtime: Path
    clang: Path
    level: int

    def build(self, source, directory):
        entry = directory / "case.py"
        entry.write_bytes(source)
        binary = directory / "case"
        command = [self.imre.resolve(), "build", entry, "-o", binary, "--no-cache",
                   "--opt-level", str(self.level), "--runtime", self.runtime.resolve(),
                   "--clang", self.clang.resolve()]
        result = subprocess.run(command, cwd=directory, capture_output=True, text=True, timeout=180)
        if result.returncode:
            raise RuntimeError(result.stderr or result.stdout)
        return binary


def method_bytes(path, class_name, method):
    """Read the entire definition, preserving its original indentation and operands."""
    data = path.read_bytes()
    cls, = [node for node in ast.parse(data).body
            if isinstance(node, ast.ClassDef) and node.name == class_name]
    node, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method]
    if node.decorator_list:
        raise ValueError(f"{class_name}.{method} needs its upstream method decorator")
    return b"".join(data.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])


def run_in_fixtures(suite, executions):
    """Keep upstream module/class/instance fixtures and execute a callback for each selected ID.

    A callback receives the original case and bound method. A native-body adapter calls a
    compiled copy; a host-driven adapter calls the original method with native operations
    installed. Never infer the location of the tested operation from unittest success.
    """
    chosen = []
    evidence = {}
    for case in cases(suite):
        name = case.id()
        if name not in executions:
            continue
        method = getattr(case, case._testMethodName)
        def run(case=case, method=method, name=name):
            evidence.setdefault(name, []).append(executions[name](case, method))
        # The suite still sees upstream skip and expected-failure decorators.
        setattr(case, case._testMethodName, wraps(method)(run))
        chosen.append(case)
    missing = set(executions) - {case.id() for case in chosen}
    if missing:
        raise ValueError(f"IDs not discovered: {sorted(missing)}")
    result = unittest.TestResult()
    unittest.TestSuite(chosen).run(result)
    failures = [(case.id(), message) for case, message in result.failures + result.errors]
    skips = [(case.id(), reason) for case, reason in result.skipped]
    if result.expectedFailures or result.unexpectedSuccesses:
        raise ValueError("expected-failure outcomes require an explicit reference comparison")
    return evidence, failures, skips


def testcase_fixture(equality=None):
    """Use the native TestCase. Supply bool for a test whose equality operands are booleans."""
    if equality is None:
        return "import unittest\n\nclass NativeCase(unittest.TestCase):\n"
    if equality != "bool":
        raise ValueError(f"unsupported equality fixture: {equality}")
    return """import unittest

class NativeCase:
    def assertEqual(self, first: bool, second: bool, msg: str | None = None) -> None:
        if first != second:
            case = unittest.TestCase()
            raise AssertionError(case._formatMessage(msg, f"{first!r} != {second!r}"))
"""


def native_method(toolchain, directory, path, owner, case_id, fixture=None):
    """Build a complete method in its declared native fixture, then run it inside host fixtures."""
    method = case_id.rsplit(".", 1)[1]
    body = method_bytes(path, owner, method)
    entry = (fixture if fixture is not None else testcase_fixture()).encode() + b"\n" + body
    entry += f'\ncase = NativeCase()\ncase.{method}()\nprint("completed {case_id}")\n'.encode()
    binary = toolchain.build(entry, directory)

    def execute(case, original):
        result = subprocess.run([binary], capture_output=True, text=True, timeout=60)
        if result.returncode or result.stderr or result.stdout != f"completed {case_id}\n":
            raise AssertionError(f"native exit {result.returncode}: {result.stdout!r} {result.stderr!r}")
        return {"execution": "native-body", "body": "Imre", "assertions": "Imre",
                "fixture_execution": "CPython module/class/instance hooks; native instance in child",
                "method_sha256": hashlib.sha256(body).hexdigest()}
    return execute
