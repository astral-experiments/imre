"""Run the complete upstream display bodies with their real CPython contexts."""
from contextlib import ExitStack
from hashlib import sha256
from io import StringIO
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

from suite_evidence import roles
from suite_native import method_bytes

HERE = Path(__file__).resolve().parent
PACKAGE = HERE / "upstream/Lib/test/test_warnings/__init__.py"
METHODS = ("test_formatwarning", "test_showwarning")


def operation_cases(tools, root):
    """Compare live cache edits, default/file/stdout/stderr, and write failures."""
    work = root / "display-operations"
    work.mkdir()
    source = HERE / "display_operations.py"
    executable = tools.build(source.read_bytes(), work)
    filename = work / "observed-source.py"
    destination = work / "output"
    filename.write_bytes("    café     \r\n\tsecond line  \r\n".encode())

    def run(command):
        destination.unlink(missing_ok=True)
        p = subprocess.run([*command, filename, destination], capture_output=True, timeout=30)
        return p.returncode, p.stdout, p.stderr, destination.read_bytes() if destination.exists() else None

    expected = run([sys.executable, "-E", source])
    actual = run([executable])
    if expected[0] or not expected[3] or b"changed in cache" not in expected[2] or actual != expected:
        raise AssertionError((expected, actual))


def observe(executable, method, level, *, fault=False):
    digest = sha256(method_bytes(PACKAGE, "WarningsDisplayTests", method)).hexdigest()

    def execute(case, original):
        calls = {"format": 0, "show": 0}

        def command(mode, message, category, filename, lineno, line):
            assert category is Warning, "upstream display fixture changed its category"
            result = subprocess.run([executable, mode, message, filename, str(lineno),
                                     "omitted" if line is None else "supplied", line or ""],
                                    capture_output=True, timeout=20)
            if result.returncode or result.stderr:
                raise AssertionError((result.returncode, result.stderr))
            calls[mode] += 1
            value = result.stdout.decode()
            if fault and (mode == "show" or method == "test_formatwarning"):
                value += "incorrect native result"
            return value

        def formatwarning(message, category, filename, lineno, line=None):
            return command("format", message, category, filename, lineno, line)

        def showwarning(message, category, filename, lineno, file=None, line=None):
            # Upstream creates a fresh host StringIO and reads it after each call.
            # Transfer the output produced by the native write to that exact stream.
            if type(file) is not StringIO or file.getvalue() != "" or file.tell() != 0:
                raise AssertionError("upstream display fixture no longer passes an empty StringIO")
            file.write(command("show", message, category, filename, lineno, line))

        with ExitStack() as stack:
            stack.enter_context(patch.object(case.module, "formatwarning", formatwarning))
            stack.enter_context(patch.object(case.module, "showwarning", showwarning))
            original()
        expected = {"format": 2, "show": 2 if method == "test_showwarning" else 0}
        if calls != expected:
            raise AssertionError((method, calls))
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"method_sha256": digest, "opt_level": level, "native_calls": calls,
                             "operation": "native default warning formatting and StringIO write",
                             "fixture": "CPython runs the original method, its warning context, source fixture, assertEqual and target observation"}}

    return execute
