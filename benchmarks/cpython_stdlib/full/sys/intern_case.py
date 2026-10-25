"""Drive the unchanged test_intern with one native string table for the whole method."""
import ast
import hashlib
import inspect
from pathlib import Path
import select
import subprocess
import textwrap
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import NativeCompileError, method_bytes, run_in_fixtures

CASE = "test.test_sys.SysModuleTest.test_intern"
HERE = Path(__file__).resolve().parent


def drive(module, command, subclass):
    events = []

    def execute(case, original):
        with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, encoding="utf-8") as proc:
            # Keep the host owners alive. Numeric handles identify objects in
            # the child; equal spellings never imply identical handles.
            handles, owners = {}, {}

            def request(*lines):
                if not all(isinstance(line, str) and "\n" not in line and "\r" not in line for line in lines):
                    raise ValueError("unexpected multiline input in original intern fixture")
                proc.stdin.write("\n".join(lines) + "\n")
                proc.stdin.flush()
                if not select.select([proc.stdout], [], [], 15)[0]:
                    raise TimeoutError("native string table did not reply")
                line = proc.stdout.readline()
                if not line.endswith("\n"):
                    raise AssertionError(("missing native reply", proc.poll()))
                return line[:-1]

            def invoke(query, *args):
                name = "_is_interned" if query else "intern"
                op = "query" if query else "intern"
                event = {"operation": name}
                events.append(event)
                if not args:
                    event["argument"] = "missing"
                    reply = request(op + "-empty")
                else:
                    value, = args
                    event["argument"] = type(value).__name__
                    if type(value) is bytes:
                        reply = request(op + "-bytes", value.decode("ascii"))
                    elif type(value) is str:
                        if id(value) not in handles:
                            handle = int(request("string", value))
                            handles[id(value)] = handle
                            owners[handle] = value
                        handle = handles[id(value)]
                        event["handle"] = handle
                        reply = request(op, str(handle))
                        if op == "intern":
                            result = int(reply)
                            event["returned_handle"] = result
                            return owners[result]
                    elif isinstance(value, str):
                        # Preserve the real upstream subclass and its __hash__.
                        # Sending its contents as an exact str would change the operation.
                        cls = textwrap.dedent(inspect.getsource(type(value)))
                        code = ("import sys\n" + cls + "\ntry:\n"
                                f"    print(repr(sys.{name}({type(value).__name__}({str(value)!r}))))\n"
                                "except TypeError as exc:\n    print('TypeError: ' + str(exc))\n")
                        event["source_sha256"] = hashlib.sha256(code.encode()).hexdigest()
                        reply = subclass(code)
                    else:
                        raise ValueError(type(value))
                if reply.startswith("TypeError: "):
                    message = reply.removeprefix("TypeError: ")
                    event["exception"] = message
                    raise TypeError(message)
                event["returned"] = ast.literal_eval(reply)
                return event["returned"]

            api = SimpleNamespace(intern=lambda *a: invoke(False, *a),
                                  _is_interned=lambda *a: invoke(True, *a))
            try:
                with patch.object(module, "sys", api):
                    original()
            finally:
                if proc.poll() is None:
                    proc.stdin.write("stop\n")
                    proc.stdin.flush()
                try:
                    status = proc.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
                    raise
                stderr = proc.stderr.read()
                if status or stderr:
                    raise AssertionError(("native string table failed", status, stderr))
        return True

    return execute, events


def observation(module, toolchain, directory, source):
    driver = directory / "driver"
    driver.mkdir()
    binary = toolchain.build((HERE / "intern_driver.py").read_bytes(), driver)

    def subclass(code):
        path = directory / "subclass"
        path.mkdir(exist_ok=True)
        program = toolchain.build(code.encode(), path)
        output = subprocess.run([program], check=True, text=True, capture_output=True, timeout=15)
        if output.stderr:
            raise AssertionError(output.stderr)
        return output.stdout.rstrip("\n")

    method, events = drive(module, [binary], subclass)

    def capture(case, original):
        try:
            method(case, original)
        except NativeCompileError as exc:
            diagnostic = str(exc).replace(str(directory / "subclass"), "[case]").strip()
            events[-1]["diagnostic"] = diagnostic
            return "blocked", diagnostic.splitlines()[-1]
        except AssertionError as exc:
            return "failure", str(exc).replace(str(directory), "[case]")
        return "pass", ""

    result, failures, skips = run_in_fixtures(load_suite("test.test_sys"), {CASE: capture})
    if failures or skips:
        raise AssertionError((failures, skips))
    (outcome, reason), = result[CASE]
    return {
        "outcome": outcome, "reason": reason,
        "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
        "evidence": {
            "operation": "one native string table for exact-string identity and original argument errors",
            "fixture": "original CPython SysModuleTest lifecycle, random spelling and str subclass",
            "assertions": "original CPython identity and exception assertions on native results",
            "method_sha256": hashlib.sha256(method_bytes(source, "SysModuleTest", "test_intern")).hexdigest(),
            "opt_level": toolchain.level, "attempts": events,
        },
    }
