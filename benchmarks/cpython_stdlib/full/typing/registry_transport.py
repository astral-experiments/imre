"""Run original patch/deprecation fixtures around a persistent native registry."""
import ast
from collections import Counter, defaultdict
from contextlib import ExitStack, contextmanager
from pathlib import Path
import select
import subprocess
import sys
import typing
from unittest.mock import patch


HOST_TYPING = ("test_overload_registry", "test_overload_on_compiled_functions")
HOST_WARNING = "test_dunder_deprecated"


@contextmanager
def bind(module, api):
    # Rebind only the product operations referenced by the test. Its unittest
    # decorator still calls the real patch() on the actual typing module.
    with ExitStack() as stack:
        for name in ("typing", "overload", "get_overloads", "clear_overloads"):
            if name in vars(module):
                item = api if name == "typing" else getattr(api, name)
                stack.enter_context(patch.object(module, name, item))
        yield


class SourceSites:
    """Collect only fixture code locations; no reference result is fed to Imre."""
    def __init__(self):
        self.lines = set()

    def __getattr__(self, name):
        return getattr(typing, name)

    def note(self, function):
        if hasattr(function, "__code__"):
            self.lines.add(function.__code__.co_firstlineno)

    def overload(self, function):
        self.note(function)
        return typing.overload(function)

    def get_overloads(self, function):
        self.note(function)
        return typing.get_overloads(function)


def worker_source():
    from test import test_typing, test_warnings

    sites = SourceSites()
    for module, owner, names in (
        (test_typing, test_typing.OverloadTests, HOST_TYPING),
        (test_warnings, test_warnings.DeprecatedTests, [HOST_WARNING]),
    ):
        with patch.object(typing, "_overload_registry", defaultdict(lambda: defaultdict(dict))), bind(module, sites):
            for name in names:
                case = owner(name)
                outcome = case.run()
                if not outcome.wasSuccessful() or outcome.skipped:
                    raise AssertionError((outcome.failures, outcome.errors, outcome.skipped))
    template = Path(__file__).with_name("registry_worker.py").read_text()
    imports = [x for x in template.splitlines() if x.startswith(("import ", "from "))]
    lines = [*imports, "", "def make(line: int) -> Callable[[], None]:"]
    for line in sorted(sites.lines):
        # A fresh native object per invocation, a distinct code site at the real
        # firstlineno, and public module/name metadata from the upstream fixture.
        if len(lines) >= line:
            raise ValueError(f"overlapping fixture code locations near {line}")
        lines += [""] * (line - 1 - len(lines))
        lines.append(f"    def original_{line}() -> None: pass")
    for line in sorted(sites.lines):
        lines.append(f"    if line == {line}: return original_{line}")
    lines.append('    raise ValueError("unknown function source location")')
    return ("\n".join(lines) + "\n\n" + template[template.index("def serve()"):]).encode()


class Worker:
    def __init__(self, executable):
        self.process = subprocess.Popen([executable], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.functions = []
        self.calls = Counter()

    def call(self, operation, *args):
        self.calls[operation] += 1
        fields = (operation, *map(str, args))
        if any("\n" in s or "\r" in s for s in fields):
            raise ValueError("fixture metadata cannot contain newlines")
        self.process.stdin.write("\n".join(fields) + "\n")
        self.process.stdin.flush()
        if not select.select([self.process.stdout], [], [], 20)[0]:
            raise TimeoutError(f"native typing response: {operation}")
        output = self.process.stdout.readline()
        if not output:
            raise AssertionError(self.process.stderr.read())
        return ast.literal_eval(output)

    def handle(self, f):
        for i, original in enumerate(self.functions):
            if f is original:
                return i
        marker = getattr(f, "__deprecated__", None)
        values = (f.__code__.co_firstlineno, f.__module__, f.__qualname__, f.__name__,
                  "0" if marker is None else "1")
        handle = self.call("new", *values, *(() if marker is None else (marker,)))
        if handle != len(self.functions):
            raise ValueError("native function handles must be distinct and in insertion order")
        self.functions.append(f)
        return handle

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write("quit\n"); self.process.stdin.flush()
        output, errors = self.process.communicate(timeout=20)
        if self.process.returncode or output or errors:
            raise AssertionError((self.process.returncode, output, errors))


class RegistryView:
    """The selected private fixture only observes whether its patched map is empty."""
    def __init__(self, native):
        self.native = native

    def __eq__(self, other):
        if type(other) is not dict or other:
            return NotImplemented
        return self.native.call("size") == 0


class Native:
    def __init__(self, executable):
        self.executable = executable
        self.registries = []
        self.fault = False
        self.dummy = self.call_stub

    @property
    def native(self):
        owner = typing._overload_registry
        for fixture_map, worker in self.registries:
            if owner is fixture_map:
                return worker
        worker = Worker(self.executable)
        self.registries.append((owner, worker))
        return worker

    @property
    def _overload_registry(self):
        return RegistryView(self.native)

    def clear_overloads(self):
        return self.native.call("clear")

    def overload(self, f):
        if f is sum or f is print:
            same_dummy = self.native.call("register-builtin", f.__name__)
        else:
            same_dummy = self.native.call("register", self.native.handle(f))
        if same_dummy is not True:
            raise AssertionError("overload did not return the singleton native dummy")
        return self.dummy

    def call_stub(self, *args, **kwargs):
        raise NotImplementedError(self.native.call("stub"))

    def get_overloads(self, f):
        if f is sum or f is print:
            return self.native.call("get-builtin", f.__name__)
        worker = self.native
        indexes = worker.call("get", worker.handle(f))
        result = []
        for i in indexes:
            if type(i) is not int or not 0 <= i < len(worker.functions):
                raise AssertionError("native registry returned an invalid function handle")
            original = worker.functions[i]
            if worker.call("marker", i) != getattr(original, "__deprecated__", None):
                raise AssertionError("native function lost its deprecation metadata")
            result.append(original)
        return list(reversed(result)) if self.fault else result

    def close(self):
        for _, worker in self.registries:
            worker.close()

    def counts(self):
        return sum((worker.calls for _, worker in self.registries), Counter())
