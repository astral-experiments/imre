"""Build native programs and run complete pinned test methods under their fixtures."""

import ast
import builtins
from collections import defaultdict
import dataclasses
import dis
from functools import wraps
import hashlib
import importlib
from pathlib import Path
import re
import subprocess
import sys
from types import CodeType
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
    reachable_only: bool = False
    module_root: Path | None = None

    def build(self, source, directory):
        directory = directory.resolve()
        entry = directory / "case.py"
        entry.write_bytes(source)
        binary = directory / "case"
        command = [self.imre.resolve(), "build", entry, "-o", binary, "--no-cache",
                   "--opt-level", str(self.level), "--runtime", self.runtime.resolve(),
                   "--clang", self.clang.resolve()]
        if self.module_root is not None:
            command.extend(["--module-root", self.module_root.resolve()])
        if self.reachable_only:
            command.append("--reachable-only")
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


class NativeFixtureError(ValueError):
    """The original method needs fixture or module behavior the default entry cannot provide."""


def _code_globals(code):
    names = {op.argval for op in dis.get_instructions(code) if op.opname == "LOAD_GLOBAL"}
    for constant in code.co_consts:
        if isinstance(constant, CodeType):
            names.update(_code_globals(constant))
    return names


def _pinned_imports(tree, module, names):
    """Recreate only original, unconditional imports whose live binding still matches."""
    imports = {}
    for stmt in tree.body:
        if not isinstance(stmt, (ast.Import, ast.ImportFrom)):
            continue
        for alias in stmt.names:
            binding = alias.asname or alias.name.split(".")[0]
            if binding not in names:
                continue
            if binding in imports or isinstance(stmt, ast.ImportFrom) and stmt.level:
                raise NativeFixtureError(f"ambiguous or relative global import: {binding}")
            if isinstance(stmt, ast.Import):
                expected = importlib.import_module(alias.name if alias.asname else binding)
            else:
                expected = getattr(importlib.import_module(stmt.module), alias.name)
            if binding not in vars(module) or vars(module)[binding] is not expected:
                raise ValueError(f"upstream import was rebound: {binding}")
            single = type(stmt)(**{field: [alias] if field == "names" else getattr(stmt, field)
                                   for field in stmt._fields})
            imports[binding] = ast.unparse(single)
    # Keep missing fixture globals reproducible across Python processes.
    for name in sorted(names - imports.keys()):
        if name in vars(module) or not hasattr(builtins, name):
            raise NativeFixtureError(f"upstream module global needs its original definition: {name}")
    return list(imports.values())


def _default_source(case, path, blob):
    """Verify the source, the discovered method and the default fixtures before staging it."""
    data = path.read_bytes()
    if hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() != blob:
        raise ValueError(f"upstream source blob mismatch: {path}")
    cls = type(case)
    module = sys.modules[cls.__module__]
    name = case._testMethodName
    if Path(module.__file__).read_bytes() != data:
        raise ValueError(f"discovery imported different source for {case.id()}")
    if (type(cls) is not type or cls.__bases__ != (unittest.TestCase,) or cls.__qualname__ != cls.__name__
            or vars(module).get(cls.__name__) is not cls
            or case.id() != f"{cls.__module__}.{cls.__name__}.{name}"):
        raise NativeFixtureError(f"{case.id()}: requires a direct, named unittest.TestCase")
    hooks = _host_hooks(case)
    if hooks or unittest.case._module_cleanups:
        raise NativeFixtureError(f"{case.id()}: original fixture needs execution: {hooks or ['module cleanup']}")
    # Subclasses receive the two class cleanup fields from TestCase.__init_subclass__.
    metadata = {"__module__", "__doc__", "__qualname__", "__firstlineno__", "__static_attributes__",
                "__dict__", "__weakref__", "_classSetupFailed", "_class_cleanups"}
    shadow = (vars(cls).keys() - metadata) & set(dir(unittest.TestCase))
    if (shadow or getattr(cls, "__unittest_skip__", False)
            or getattr(cls, "__unittest_expecting_failure__", False) or cls._classSetupFailed):
        raise NativeFixtureError(f"{case.id()}: custom TestCase state or lifecycle: {sorted(shadow)}")
    if vars(case) != vars(cls(name)):
        raise NativeFixtureError(f"{case.id()}: discovered instance was configured by its fixture")
    tree = ast.parse(data)
    owner, = [node for node in tree.body if isinstance(node, ast.ClassDef) and node.name == cls.__name__]
    method, = [node for node in owner.body if isinstance(node, ast.FunctionDef) and node.name == name]
    func = vars(cls).get(name)
    if (owner.decorator_list or len(owner.bases) != 1 or owner.keywords or method.decorator_list
            or not hasattr(func, "__code__") or func.__closure__ or func.__dict__
            or func.__globals__ is not vars(module) or func.__defaults__ or func.__kwdefaults__):
        raise NativeFixtureError(f"{case.id()}: decorated or configured class/method needs its original fixture")
    compiled = compile(data, module.__file__, "exec", dont_inherit=True)
    original_class, = [c for c in compiled.co_consts if isinstance(c, CodeType) and c.co_name == cls.__name__]
    original_method, = [c for c in original_class.co_consts if isinstance(c, CodeType) and c.co_name == name]
    if original_method != func.__code__:
        raise ValueError(f"{case.id()}: discovered method differs from the pinned definition")
    args = method.args
    if (len(args.args) != 1 or args.posonlyargs or args.kwonlyargs or args.vararg or args.kwarg
            or args.args[0].annotation or method.returns):
        raise NativeFixtureError(f"{case.id()}: needs its original signature and annotation scope")
    receiver = args.args[0].arg
    parents = {child: parent for parent in ast.walk(method) for child in ast.iter_child_nodes(parent)}
    for node, parent in parents.items():
        if isinstance(node, ast.Name) and node.id == receiver:
            if (not isinstance(parent, ast.Attribute) or parent.value is not node or not isinstance(parent.ctx, ast.Load)
                    or not (parent.attr.startswith("assert") or parent.attr in {"fail", "skipTest"})
                    or not hasattr(unittest.TestCase, parent.attr)):
                raise NativeFixtureError(f"{case.id()}: depends on instance/class state outside the staged method")
        if (isinstance(node, ast.ImportFrom) and node.level) or isinstance(node, (ast.Global, ast.Nonlocal)):
            raise NativeFixtureError(f"{case.id()}: needs the original module or closure scope")
    names = _code_globals(original_method)
    if names & {"globals", "locals", "vars", "eval", "exec", "__import__", "__name__", "__file__", "super"}:
        raise NativeFixtureError(f"{case.id()}: depends on the original execution namespace")
    base = owner.bases[0]
    names.update(node.id for node in ast.walk(base) if isinstance(node, ast.Name))
    imports = _pinned_imports(tree, module, names)
    header = data.splitlines(keepends=True)[owner.lineno - 1]
    if header.decode().strip() != f"class {cls.__name__}({ast.unparse(base)}):":
        raise NativeFixtureError(f"{case.id()}: cannot preserve the original class header")
    body = method_bytes(path, cls.__name__, name)
    entry = ("\n".join(dict.fromkeys(["import unittest", *imports])) + "\n\n").encode()
    entry += header + body + b"\nif __name__ == '__main__':\n    unittest.main()\n"
    return entry, body


@dataclasses.dataclass(frozen=True)
class NativeDefaultMethod:
    """Run an unchanged discovered method with default fixtures inside native unittest.main()."""

    toolchain: Toolchain
    directory: Path
    source: Path
    blob: str
    operation: tuple[str, str]
    assertions: str = "imre"

    def run(self, case):
        location, description = self.operation
        execution_roles = roles(body="imre", assertions=self.assertions, operation=location, fixture="imre")
        try:
            entry, body = _default_source(case, self.source, self.blob)
        except NativeFixtureError as exc:
            return {"outcome": "blocked", "reason": str(exc), "roles": roles(
                    body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved"),
                    "evidence": {"stage": "original fixture", "source_blob": self.blob}}
        try:
            binary = self.toolchain.build(entry, self.directory)
        except NativeCompileError as exc:
            observation = compile_blocker(exc)
            observation["reason"] = observation["reason"].replace(str(self.directory) + "/", "<case>/")
            observation["evidence"].update(source_blob=self.blob, method_sha256=hashlib.sha256(body).hexdigest())
            return observation
        try:
            result = subprocess.run([binary, "-v"], cwd=self.directory, capture_output=True, text=True, timeout=60)
        except subprocess.TimeoutExpired:
            return {"outcome": "error", "reason": f"native runner timed out after 60 seconds for {case.id()}",
                    "roles": roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved"),
                    "evidence": {"stage": "native runner", "source_blob": self.blob}}
        name = case._testMethodName
        staged_id = f"__main__.{type(case).__name__}.{name}"
        stderr = re.sub(r"(?m)^(Ran 1 test in )\d+\.\d+s$", r"\1<elapsed>s", result.stderr)
        label = f"{name} ({staged_id})"
        if short := case.shortDescription():
            label += "\n" + short
        expected = f"{label} ... ok\n\n{'-' * 70}\nRan 1 test in <elapsed>s\n\nOK\n"
        evidence = {"operation": description, "fixture": "default TestCase lifecycle in native unittest.main",
                    "source_blob": self.blob, "method_sha256": hashlib.sha256(body).hexdigest(),
                    "entry_sha256": hashlib.sha256(entry).hexdigest(), "staged_id": staged_id,
                    "stdout": result.stdout, "stderr": stderr, "status": result.returncode,
                    "reachable_only": self.toolchain.reachable_only}
        if (result.returncode, result.stdout, stderr) != (0, "", expected):
            return {"outcome": "error", "reason": f"native runner did not report one passing {staged_id} (exit {result.returncode}); see stderr/stdout",
                    "roles": roles(body="unobserved", assertions="unobserved", operation="unobserved",
                                   fixture="imre" if f"{name} ({staged_id}) ..." in stderr else "unobserved"),
                    "evidence": evidence}
        return {"outcome": "pass", "roles": execution_roles, "evidence": evidence}


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


def run_evidence(suite, executions, reference, level, *, preflight=None, exceptions=None,
                 fixture_variant=None, native_defaults=None):
    """Collect v1 evidence for the full denominator, preserving fixture and subtest outcomes.

    IDs select all occurrences; (ID, 1-based occurrence) selects a single one.
    Preflight observations record compile blockers or reviewed exclusions before
    launching a case. native_defaults maps exact (ID, occurrence) keys to native
    default-fixture entries; their lifecycle runs solely in the native child.
    Exceptions must use the exact (ID, occurrence, level) key.
    """
    if reference["platform"] != sys.platform or type(level) is not int or level not in (0, 3):
        raise ValueError("fixture platform and O0/O3 must match the native evidence run")
    observations = dict(preflight or {})
    if native_defaults:
        leaves = list(_fixture_cases(suite))
        discovered_cases = dict(zip(occurrence_keys(case.id() for case in leaves), leaves, strict=True))
        if discovered_cases.keys() != reference_cases(reference).keys():
            raise ValueError("discovery and reference occurrences differ")
        for key, entry in native_defaults.items():
            if key not in discovered_cases or key in observations or key in executions or key[0] in executions:
                raise ValueError(f"unknown or duplicate native default-fixture occurrence: {key}")
            if entry.toolchain.level != level:
                raise ValueError(f"{key}: native toolchain opt level does not match the report")
            observations[key] = entry.run(discovered_cases[key])
    discovered, result = _run_selected(suite, executions, fixture_variant, reference)
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
    """Use the native TestCase, including for the former bool-specific fixture."""
    if equality not in (None, "bool"):
        raise ValueError(f"unsupported equality fixture: {equality}")
    return "import unittest\n\nclass NativeCase(unittest.TestCase):\n"


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
