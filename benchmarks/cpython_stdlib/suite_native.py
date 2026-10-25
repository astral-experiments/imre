"""Build native programs and run complete pinned test methods under their fixtures."""

import ast
from collections import defaultdict
import dataclasses
from functools import wraps
import hashlib
from pathlib import Path
import subprocess
import sys
import unittest

from suite_evidence import build_report, occurrence_keys, reference_cases, roles
from suite_inventory import cases


class NativeCompileError(RuntimeError):
    """The compiler did not produce an executable for the requested native entry."""


def compile_blocker(exc):
    return {"outcome": "blocked", "reason": str(exc),
            "roles": roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved"),
            "evidence": {"stage": "compile"}}


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
        try:
            result = subprocess.run(command, cwd=directory, capture_output=True, text=True, timeout=180)
        except subprocess.TimeoutExpired as exc:
            raise NativeCompileError(f"180-second compiler timeout for {entry}") from exc
        if result.returncode:
            raise NativeCompileError(result.stderr or result.stdout or f"compiler exit {result.returncode}")
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


def _host_hooks(case):
    cls = type(case)
    module = sys.modules.get(cls.__module__)
    hooks = [name for name in ("setUpModule", "tearDownModule") if getattr(module, name, None)]
    for name in ("setUpClass", "tearDownClass", "setUp", "tearDown", "asyncSetUp", "asyncTearDown"):
        actual = getattr(case, name, None)
        default = getattr(unittest.TestCase, name, None)
        if getattr(actual, "__func__", actual) is not getattr(default, "__func__", default):
            hooks.append(name)
    if getattr(case, "_cleanups", None) or getattr(cls, "_class_cleanups", None):
        hooks.append("registered cleanup")
    return hooks


class _FixtureResult(unittest.TestResult):
    """Bind callbacks at startTest so even a repeated *object* keeps the right occurrence."""

    def __init__(self, selected):
        super().__init__()
        self.selected = selected
        self.index = 0
        self.active = None
        self.completed = {}
        self.entered = set()
        self.events = defaultdict(list)
        self.subtests = defaultdict(list)
        self.host_hooks = {}

    def startTest(self, case):
        super().startTest(case)
        key, expected, execute = self.selected[self.index]
        if case is not expected or self.active is not None:
            raise ValueError("nonstandard TestCase lifecycle requires an explicit fixture adapter")
        self.host_hooks[key] = _host_hooks(case)
        attr = case._testMethodName
        self.active = (key, attr, attr in vars(case), vars(case).get(attr))
        original = getattr(case, attr)
        @wraps(original)
        def invoke():
            self.entered.add(key)
            self.completed[key] = execute(case, original)
        setattr(case, attr, invoke)

    def stopTest(self, case):
        _, attr, existed, saved = self.active
        if existed:
            setattr(case, attr, saved)
        else:
            delattr(case, attr)
        self.active = None
        super().stopTest(case)

    def record(self, case, outcome, reason):
        if self.active is not None:
            self.events[self.active[0]].append((outcome, reason))
            return
        # Class/module setup can prevent startTest; teardown can fail after
        # addSuccess. Attribute the event to just this contiguous fixture group.
        label = case.id()
        phase, _, target = label.partition(" (")
        if phase not in {"setUpClass", "tearDownClass", "setUpModule", "tearDownModule"} or not target.endswith(")"):
            raise ValueError(f"cannot attribute fixture event {label!r}")
        is_class = phase.endswith("Class")
        def group(index):
            cls = type(self.selected[index][1])
            return f"{cls.__module__}.{cls.__qualname__}" if is_class else cls.__module__
        index = self.index if phase.startswith("setUp") else self.index - 1
        if not 0 <= index < len(self.selected) or group(index) != target[:-1]:
            raise ValueError(f"fixture event {label!r} does not match the selected cases")
        left = right = index
        while left and group(left - 1) == group(index):
            left -= 1
        while right + 1 < len(self.selected) and group(right + 1) == group(index):
            right += 1
        for key, _, _ in self.selected[left:right + 1]:
            self.events[key].append((outcome, reason))
            self.subtests[key].append({"fixture_event": label, "outcome": outcome, "reason": reason})

    def addError(self, case, err):
        super().addError(case, err)
        self.record(case, "error", self.errors[-1][1])

    def addFailure(self, case, err):
        super().addFailure(case, err)
        self.record(case, "failure", self.failures[-1][1])

    def addSkip(self, case, reason):
        super().addSkip(case, reason)
        if self.active and case is not self.selected[self.index][1]:
            self.subtests[self.active[0]].append({"id": case.id(), "outcome": "skip", "reason": reason})
        self.record(case, "skip", reason)

    def addUnexpectedSuccess(self, case):
        super().addUnexpectedSuccess(case)
        self.record(case, "unexpected-success", "upstream expected-failure method succeeded")

    def addExpectedFailure(self, case, err):
        super().addExpectedFailure(case, err)
        self.record(case, "expected-failure", self.expectedFailures[-1][1])

    def addSubTest(self, case, subtest, err):
        failures, errors = len(self.failures), len(self.errors)
        super().addSubTest(case, subtest, err)
        event = {"id": subtest.id(), "outcome": "pass"}
        if err is not None:
            if len(self.failures) > failures:
                event.update(outcome="failure", reason=self.failures[-1][1])
            elif len(self.errors) > errors:
                event.update(outcome="error", reason=self.errors[-1][1])
            else:
                raise ValueError(f"cannot attribute failing subtest {subtest.id()}")
            self.record(case, event["outcome"], event["reason"])
        self.subtests[self.active[0]].append(event)


class _FixtureSuite(unittest.TestSuite):
    """Leave fixture execution to unittest and mark the boundary of its current group."""

    def __init__(self, result):
        self.result = result
        super().__init__([case for _, case, _ in result.selected])

    def __iter__(self):
        for index, case in enumerate(super().__iter__()):
            self.result.index = index
            yield case
        self.result.index = len(self.result.selected)


def _fixture_cases(suite):
    if (type(suite).run is not unittest.TestSuite.run
            or type(suite).__call__ is not unittest.TestSuite.__call__):
        raise ValueError("custom TestSuite execution needs an explicit fixture adapter")
    for case in suite:
        if isinstance(case, unittest.BaseTestSuite):
            yield from _fixture_cases(case)
        else:
            yield case


def _run_selected(suite, executions, fixture_variant=None, reference=None):
    all_cases = list(cases(suite) if reference is None else _fixture_cases(suite))
    if fixture_variant is None:
        fixture_variant = lambda case: f"{type(case).__module__}.{type(case).__qualname__}"
    discovered = [(case.id(), fixture_variant(case)) for case in all_cases]
    keys = list(occurrence_keys(name for name, _ in discovered))
    if reference is not None and set(keys) != reference_cases(reference).keys():
        raise ValueError("discovery and reference occurrences differ")
    selected, used = [], set()
    for key, case in zip(keys, all_cases, strict=True):
        if key in executions and key[0] in executions:
            raise ValueError(f"both occurrence and ID adapters supplied for {key}")
        adapter = key if key in executions else key[0]
        if adapter in executions:
            used.add(adapter)
            selected.append((key, case, executions[adapter]))
    if used != set(executions):
        raise ValueError(f"adapters name undiscovered IDs/occurrences: {set(executions) - used}")
    result = _FixtureResult(selected)
    _FixtureSuite(result).run(result)
    return discovered, result


def run_in_fixtures(suite, executions):
    """Run selected callbacks under the upstream lifecycle; keep the original #508 interface."""
    _, result = _run_selected(suite, executions)
    evidence = {}
    for (name, _), value in result.completed.items():
        evidence.setdefault(name, []).append(value)
    if result.expectedFailures or result.unexpectedSuccesses:
        raise ValueError("expected-failure outcomes require an explicit reference comparison")
    failures = [(case.id(), message) for case, message in result.failures + result.errors]
    return evidence, failures, [(case.id(), reason) for case, reason in result.skipped]


def run_evidence(suite, executions, reference, level, *, preflight=None, exceptions=None, fixture_variant=None):
    """Collect v1 evidence for the full denominator, preserving fixture and subtest outcomes.

    IDs select all occurrences; (ID, 1-based occurrence) selects a single one.
    Preflight observations record compile blockers or reviewed exclusions before
    launching a case. Exceptions must use the exact (ID, occurrence, level) key.
    """
    if reference["platform"] != sys.platform or type(level) is not int or level not in (0, 3):
        raise ValueError("fixture platform and O0/O3 must match the native evidence run")
    discovered, result = _run_selected(suite, executions, fixture_variant, reference)
    observations = dict(preflight or {})
    for key, _, _ in result.selected:
        if key in observations:
            raise ValueError(f"both a preflight decision and an execution for {key}")
        returned = result.completed.get(key)
        observation = returned.get("observation", returned) if isinstance(returned, dict) else None
        if observation is not None and not {"roles", "evidence"} <= observation.keys():
            raise ValueError(f"{key}: declare roles and evidence; legacy labels are not provenance")
        if observation:
            observation = {field: observation[field] for field in ("roles", "evidence")}
            hooks = result.host_hooks.get(key, [])
            if hooks and observation["roles"]["fixture"] not in {"cpython", "mixed"}:
                raise ValueError(f"{key}: upstream host fixture hooks ran: {hooks}")
            if hooks:
                observation["evidence"] = {**observation["evidence"], "host_fixture_hooks": hooks}
        else:
            unknown = "unknown" if key in result.entered else "unobserved"
            fixture = "unknown" if key in result.entered else "cpython"
            observation = {"roles": roles(body=unknown, assertions=unknown, operation=unknown, fixture=fixture),
                           "evidence": {"fixture": "CPython lifecycle; no completed adapter observation"}}
        events = result.events.get(key, [])
        if not events and key not in result.completed:
            raise ValueError(f"selected occurrence did not produce a test result: {key}")
        if not events and returned is None:
            raise ValueError(f"{key}: passing callback did not return execution evidence")
        # A teardown or failed subtest takes precedence over an earlier skip.
        priority = {"error": 0, "failure": 1, "unexpected-success": 2, "expected-failure": 3, "skip": 4}
        outcome, reason = min(events, key=lambda e: priority[e[0]]) if events else ("pass", None)
        if reason is not None:
            observation["reason"] = reason
        if events or result.subtests.get(key):
            observation["evidence"] = {**observation["evidence"], "events": [
                {"outcome": status, "reason": text} for status, text in events], "details": result.subtests[key]}
        observations[key] = dict(observation, outcome=outcome)
    return build_report(reference, level, discovered, observations, exceptions=exceptions)


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


def native_method(toolchain, directory, path, owner, case_id, fixture=None, *, operation=None, assertions="imre"):
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
        record = {"execution": "native-body", "body": "Imre", "assertions": "Imre",
                  "fixture_execution": "CPython module/class/instance hooks; native instance in child",
                  "method_sha256": hashlib.sha256(body).hexdigest()}
        if operation is not None:
            location, description = operation
            record["observation"] = {"roles": roles(body="imre", assertions=assertions,
                                                    operation=location, fixture="mixed"),
                                     "evidence": {"operation": description,
                                                  "fixture": record["fixture_execution"],
                                                  "opt_level": toolchain.level,
                                                  "method_sha256": record["method_sha256"]}}
        return record
    return execute
