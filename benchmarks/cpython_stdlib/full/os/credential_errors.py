"""Run original credential error methods with every kernel operation in a fresh child."""

from functools import partial
import hashlib
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

from method_preflight import methods
from native_descriptors import HERE, descriptor_suite, host_fixtures
from suite_native import NativeCompileError, testcase_fixture


SELECTIONS = {
    "test_os": ("PosixUidGidTests", ("setuid", "setgid", "seteuid", "setegid", "setreuid", "setregid")),
    "test_posix": ("PosixTester", ("setresuid_exception", "setresgid_exception")),
}
OPERATIONS = {"getuid", "getresuid", "getresgid", "setuid", "setgid", "seteuid", "setegid",
              "setreuid", "setregid", "setresuid", "setresgid"}


class NativeCredentials:
    def __init__(self, tools, root, module):
        self.tools, self.root, self.module = tools, root, module
        self.cache = {}
        self.calls = []

    def __getattr__(self, name):
        if name not in OPERATIONS:
            raise AttributeError(name)
        return partial(self.invoke, name)

    def invoke(self, name, *args):
        # A callable adapter preserves input type: the invalid original string
        # must reach the native converter instead of being normalized by Python.
        types = tuple(type(value) for value in args)
        if any(ty not in (int, str) for ty in types):
            raise TypeError((name, types))
        key = (name, types)
        if key not in self.cache:
            expressions = [f"int(sys.argv[{i}])" if ty is int else f"sys.argv[{i}]"
                           for i, ty in enumerate(types, 1)]
            call = f"{self.module}.{name}({', '.join(expressions)})"
            if name.startswith("getres"):
                result = f"a, b, c = {call}\n    print(a)\n    print(b)\n    print(c)"
            elif name.startswith("get"):
                result = f"print({call})"
            else:
                result = f"{call}\n    print('None')"
            source = f"import {self.module}\nimport sys\n\ntry:\n    {result}\n" + """except TypeError as e:
    print("TypeError")
    print(str(e))
except OverflowError as e:
    print("OverflowError")
    print(str(e))
except OSError as e:
    print("OSError")
    code = e.errno
    reason = e.strerror
    assert code is not None and reason is not None
    print(code)
    print(reason)
"""
            directory = self.root / (self.module + '-' + name + '-' + '-'.join(ty.__name__ for ty in types))
            directory.mkdir()
            self.cache[key] = self.tools.build(source.encode(), directory)
        self.calls.append(name)
        run = subprocess.run([self.cache[key], *map(str, args)], cwd=self.root, start_new_session=True,
                             capture_output=True, text=True, timeout=30, check=True)
        cells = run.stdout.splitlines()
        if run.stderr or not cells:
            raise AssertionError((name, run.stdout, run.stderr))
        if cells[0] in ("TypeError", "OverflowError") and len(cells) == 2:
            raise {"TypeError": TypeError, "OverflowError": OverflowError}[cells[0]](cells[1])
        if cells[0] == "OSError" and len(cells) == 3:
            raise OSError(int(cells[1]), cells[2])
        if name.startswith("getres") and len(cells) == 3:
            return tuple(map(int, cells))
        if name.startswith("get") and len(cells) == 1:
            return int(cells[0])
        if cells != ["None"]:
            raise AssertionError((name, cells))


def run_errors(modules, tools, root):
    rows = {}
    for suite, (cls, names) in SELECTIONS.items():
        module = modules[suite]
        binding = "os" if suite == "test_os" else "posix"
        source = methods(HERE / "upstream/Lib/test" / (suite + ".py"))
        native = NativeCredentials(tools, root, binding)

        def execute(case, original):
            method = case._testMethodName
            body, _ = source[f"{cls}.{method}"]
            directory = root / method
            directory.mkdir()
            imports, fixture = testcase_fixture().split("\n\n", 1)
            if suite == "test_os":
                # Keep the upstream platform probe and exact ID width constants.
                setup = f"HAVE_WHEEL_GROUP = {module.HAVE_WHEEL_GROUP!r}\n"
                fixture += ("    @property\n    def UID_OVERFLOW(self) -> int:\n"
                            f"        return {case.UID_OVERFLOW}\n\n"
                            "    @property\n    def GID_OVERFLOW(self) -> int:\n"
                            f"        return {case.GID_OVERFLOW}\n\n")
            else:
                setup = ""
            entry = (f"import {binding}\n{imports}\n\n" + setup + fixture).encode() + body
            entry += f"\nNativeCase().{method}()\n".encode()
            try:
                binary = tools.build(entry, directory)
            except NativeCompileError as exc:
                blocker = str(exc).replace(str(directory) + '/', '').strip().splitlines()[-1]
            else:
                subprocess.run([binary], cwd=directory, start_new_session=True, timeout=30, check=True)
                return {"status": "native_body_host_fixture", "assertions": "imre",
                        "method_sha256": hashlib.sha256(body).hexdigest(), "host_fixtures": host_fixtures(case)}
            native.calls.clear()
            with patch.object(module, binding, native):
                original()
            if not native.calls:
                raise AssertionError(f"{case.id()}: no native operations")
            return {"status": "host_driven_native", "native_calls": list(native.calls),
                    "native_body_blocker": blocker, "host_fixtures": host_fixtures(case) +
                    ["original callable assertion, with each real credential operation in an isolated native child"]}

        selections = {f"{module.__name__}.{cls}.test_{name}": execute for name in names}
        rows.update(descriptor_suite(module, selections))
    return rows
