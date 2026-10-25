"""Run the unchanged CPython POSIX pipe2 methods inside their original fixtures."""
import argparse
import ast
from contextlib import ExitStack, chdir
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from host_descriptors import HERE, bindings, build_library
from method_preflight import methods
from native_descriptors import Toolchain, descriptor_suite, host_fixtures, verify_sources
from suite_evidence import roles
from suite_native import NativeCompileError, testcase_fixture


NAMES = ("test_pipe2", "test_pipe2_c_limits")


def native_body(case, original, module, tools, work):
    from test import support

    name = case._testMethodName
    path = Path(module.__file__)
    assert original.__func__.__code__.co_filename == str(path)
    original_body, first_line = methods(path)[type(case).__name__ + "." + name]
    digest = hashlib.sha256(original_body).hexdigest()
    # These release decorators use unittest.skipIf/skipUnless. The host ran
    # them at import and selected the unwrapped original method for this callback.
    inputs = {}
    if name == "test_pipe2":
        inputs["support"] = {"PIPE_MAX_SIZE": support.PIPE_MAX_SIZE}
    else:
        import _testcapi
        inputs["_testcapi"] = {"INT_MAX": _testcapi.INT_MAX, "UINT_MAX": _testcapi.UINT_MAX}
    attempts = {}
    for binding in (("public",) if name == "test_pipe2_c_limits" else ("public", "cleanup")):
        folder = work / (name + "-" + binding)
        folder.mkdir()
        for owner, values in inputs.items():
            (folder / (owner + ".py")).write_text("".join(f"{k} = {v}\n" for k, v in values.items()))
        imports = "import os\n"
        native_case = testcase_fixture()
        cleanup = ""
        if binding == "cleanup":
            # The only registered cleanups in the original body close its two fds.
            tree = ast.parse(original_body.decode().lstrip())
            registered = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
                          and isinstance(node.func, ast.Attribute) and node.func.attr == "addCleanup"]
            assert len(registered) == 2 and all(len(n.args) == 2 and ast.unparse(n.args[0]) == "os.close"
                                                for n in registered)
            (folder / "subject.py").write_text(
                "import os\nfrom os import pipe2, get_blocking, get_inheritable, O_CLOEXEC, O_NONBLOCK\n"
                "def close(fd: int) -> None:\n    os.close(fd)\n"
                "def read(fd: int, count: int) -> bytes:\n    return os.read(fd, count)\n"
                "def write(fd: int, data: bytes) -> int:\n    return os.write(fd, data)\n")
            imports = "from typing import Callable\nimport subject as os\n"
            native_case += """    cleanups: list[int]
    def __init__(self) -> None:
        super().__init__()
        self.cleanups = []
    def addCleanup(self, close: Callable[[int], None], fd: int) -> None:
        self.cleanups.append(fd)
    def clean(self) -> None:
        while self.cleanups:
            os.close(self.cleanups.pop())
"""
            cleanup = f"try:\n    case.{name}()\nfinally:\n    case.clean()\n"
        if "support" in inputs:
            imports += "import support\n"
        prefix = (imports + native_case + "\n").encode()
        source = prefix + original_body + b"\ncase = NativeCase()\n"
        source += (cleanup or f"case.{name}()\n").encode()
        source += f'print("completed {case.id()}")\n'.encode()
        try:
            binary = tools.build(source, folder)
        except NativeCompileError as error:
            diagnostic = str(error).replace(str(folder) + os.sep, "").strip()
            diagnostic = diagnostic.removeprefix(f"Compiling case.py (-O{tools.level})\n")
            attempts[binding] = {"diagnostic": diagnostic}
            found = re.search(r"case\.py:(\d+):(\d+):", diagnostic)
            if found:
                index = int(found[1]) - 1 - prefix.count(b"\n")
                if 0 <= index < len(original_body.splitlines()):
                    attempts[binding].update(upstream_line=first_line + index,
                                             upstream_statement=original_body.splitlines()[index].decode().strip())
            continue
        run = subprocess.run([binary], capture_output=True, text=True, timeout=30)
        if run.returncode or run.stderr or run.stdout != f"completed {case.id()}\n":
            raise AssertionError(f"native {case.id()}: {run.returncode} {run.stdout!r} {run.stderr!r}")
        return digest, inputs, attempts, binding
    return digest, inputs, attempts, None


def install_pipe2(view, invoke):
    def pipe2(*args):
        if len(args) == 1 and type(args[0]) is str:
            form = "pipe2_str"
        elif len(args) == 1 and type(args[0]) is int:
            form = "pipe2_int"
        elif len(args) == 2 and all(type(value) is int for value in args):
            form = "pipe2_two"
        else:
            raise AssertionError(f"new upstream pipe2 argument shape: {args!r}")
        data = invoke(form, *args).splitlines()
        if data[0] in (b"TypeError", b"OverflowError"):
            label, message = data
            raise {b"TypeError": TypeError, b"OverflowError": OverflowError}[label](message.decode())
        reader, writer = map(int, data)
        return reader, writer
    view.pipe2 = pipe2
    view.O_CLOEXEC = os.O_CLOEXEC
    view.O_NONBLOCK = os.O_NONBLOCK


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        ap.add_argument("--" + name, type=Path, required=True)
    ap.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    ap.add_argument("--cpython-source", type=Path)
    args = ap.parse_args()
    modules, commit = verify_sources(args.cpython_source)
    module = modules["test_posix"]
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory() as temp, ExitStack() as lifetime:
        work = Path(temp)
        def execute(case, original):
            digest, inputs, attempts, binding = native_body(case, original, module, tools, work)
            hooks = host_fixtures(case)
            evidence = {"opt_level": tools.level, "method_sha256": digest, "fixture": "; ".join(hooks),
                        "host_fixture_constants": inputs, "complete_native_attempts": attempts}
            if binding:
                evidence["operation"] = "os.pipe2"
                return {"status": "native_body_host_fixture", "method_sha256": digest, "binding": binding,
                        "host_fixtures": hooks,
                        "observation": {"roles": roles(body="imre", assertions="imre", operation="imre",
                                                       fixture="mixed"), "evidence": evidence}}
            if sys.platform != "linux":
                raise AssertionError("original method selected outside the supported pipe2 target")
            directory = work / (case._testMethodName + "-calls")
            directory.mkdir()
            library = build_library(directory, HERE / "pipe2_calls.py", tools.imre, tools.runtime,
                                    tools.clang, tools.level)
            view, calls = lifetime.enter_context(bindings(library, install_pipe2))
            host = []
            def observer(name):
                def observed(*args):
                    host.append("os." + name)
                    return getattr(os, name)(*args)
                return observed
            view.read, view.write = observer("read"), observer("write")
            trace, entries = [], []
            def collect_calls():
                entries.extend(calls)
                trace.extend("os." + ("pipe2" if call.startswith("pipe2_") else call) for call in calls)
            case.addCleanup(collect_calls)
            with patch.object(module, "os", view):
                original()
            evidence.update(operation="os.pipe2; descriptor checks", native_calls=trace,
                            native_driver_entries=entries, host_calls=host)
            return {"status": "mixed" if host else "host_driven_native", "method_sha256": digest,
                    "host_fixtures": hooks, "native_calls": trace, "host_calls": host,
                    "observation": {"roles": roles(body="cpython", assertions="cpython",
                        operation="mixed" if host else "imre", fixture="cpython"), "evidence": evidence}}
        with chdir(work):
            cases = descriptor_suite(module, {f"{module.__name__}.PosixTester.{name}": execute for name in NAMES})
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": args.opt_level,
                                      "cases": cases}, indent=2) + "\n")
    for name, row in cases.items():
        print(name, row.get("status", "reference_skip"), row.get("diagnostic", row.get("reference_skip", "")))
    return int(any(row.get("status") == "failed" for row in cases.values()))


if __name__ == "__main__":
    sys.exit(main())
