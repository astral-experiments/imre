"""Run original size-rotation routines on their real fixture filenames."""

import argparse
import ast
from collections import Counter
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_evidence import roles
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
OWNER = "RotatingFileHandlerTest"
PREFIX = "test.test_logging." + OWNER + "."
NON_FILE = "test_should_not_rollover_non_file"
NATIVE = ("test_file_created", "test_should_not_rollover", "test_should_rollover", NON_FILE)
HOST = {
    "test_should_not_rollover_named_pipe": {"open": 1, "create": 1, "should": 1, "emit": 1,
                                            "close": 1, "message": 1},
    "test_max_bytes": {"open": 1, "from_message": 2, "should": 6, "emit": 2, "close": 1},
    "test_max_bytes_delay": {"open": 1, "from_message": 2, "should": 6, "emit": 2, "close": 1},
}

FIXTURE = """import logging.handlers
import os
import unittest

class NativeCase(unittest.TestCase):
    fn: str
    message_num: int
    rmfiles: list[str]

    def __init__(self) -> None:
        super().__init__()
        self.fn = FILENAME
        self.message_num = MESSAGE_NUM
        self.rmfiles = []
"""


def non_file_case(toolchain, directory, case, original, fixture):
    """Use the original unittest skip decision before running the definition on /dev/null."""
    source = SOURCE.read_bytes()
    cls, = [n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == OWNER]
    node, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == NON_FILE]
    guard, = node.decorator_list
    condition = ast.unparse(guard)
    if (condition != "unittest.skipIf(support.is_wasi, 'WASI does not have /dev/null.')"
            or original.__func__ is not getattr(type(case), NON_FILE)
            or original.__code__.co_firstlineno != guard.lineno
            or getattr(original, "__unittest_skip__", False)):
        raise ValueError(f"unexpected bound method or upstream guard on {case.id()}")
    body = b"".join(source.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
    entry = fixture.encode() + b"\n" + body + f"\nNativeCase().{NON_FILE}()\nprint('complete')\n".encode()
    binary = toolchain.build(entry, directory)
    run = subprocess.run([binary], capture_output=True, timeout=60)
    if (run.returncode, run.stdout, run.stderr) != (0, b"complete\n", b""):
        raise AssertionError((run.returncode, run.stdout, run.stderr))
    return {"roles": roles(body="imre", assertions="imre", fixture="mixed", operation="imre"),
            "evidence": {"method_sha256": hashlib.sha256(body).hexdigest(), "guard": condition,
                         "operation": "Native /dev/null handler, record construction, shouldRollover and close",
                         "fixture": "Original CPython hooks and unittest.skipIf decision; native child instance",
                         "opt_level": toolchain.level}}


def run_case(toolchain, directory, case, original):
    fixture = FIXTURE.replace("FILENAME", repr(case.fn)).replace("MESSAGE_NUM", str(case.message_num))
    method = case._testMethodName
    helpers = []
    if method not in {"test_should_not_rollover", "test_max_bytes", "test_max_bytes_delay"}:
        helpers.extend((("BaseTest", "next_message"), ("BaseFileTest", "next_rec")))
    if method not in {"test_should_not_rollover", "test_should_rollover", NON_FILE}:
        helpers.append(("BaseFileTest", "assertLogFile"))
    if method == "test_max_bytes_delay":
        helpers.append((OWNER, "test_max_bytes"))
    for owner, name in helpers:
        fixture += "\n" + method_bytes(SOURCE, owner, name).decode()
    path = Path(case.fn)
    # The original instance lives in the child. Pass its newly created backups to
    # the host's original tearDown; BaseFileTest already removes the active file.
    before = set(path.parent.glob(path.name + ".*"))
    try:
        if method == NON_FILE:
            return non_file_case(toolchain, directory, case, original, fixture)
        return native_method(toolchain, directory, SOURCE, OWNER, case.id(), fixture,
                             operation=("imre", "Native file handler, descriptor position, rotation and on-disk assertions"))(case, original)
    finally:
        case.rmfiles.extend(str(p) for p in sorted(set(path.parent.glob(path.name + ".*")) - before))


@contextmanager
def native_handlers(binary):
    calls = Counter()
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True, encoding="utf-8") as process:
        def send(command, *args):
            lines = [str(x) for x in (command, *args)]
            if any("\n" in line or "\r" in line for line in lines):
                raise ValueError("multiline input to the rotation fixture transport")
            process.stdin.write("\n".join(lines) + "\n")
            process.stdin.flush()
            if not select.select([process.stdout], [], [], 30)[0]:
                raise TimeoutError(f"native handler did not finish {command}")
            answer = process.stdout.readline()
            if not answer:
                raise RuntimeError(f"native handler exited: {process.stderr.read()}")
            calls[command] += 1
            return ast.literal_eval(answer)

        class Record:
            def __init__(self, index):
                self.index = index

            @property
            def msg(self):
                return send("message", self.index)

        def record(name, level, path, line, msg, args, exc_info, func):
            if (args, exc_info, func) != (None, None, None):
                raise ValueError("unexpected LogRecord arguments in the rotation fixture")
            return Record(send("create", name, level, path, line, msg))

        def from_mapping(fields):
            if fields.keys() != {"msg"}:
                raise ValueError(fields)
            return Record(send("from_message", fields["msg"]))

        class Handler:
            def __init__(self, filename, mode="a", maxBytes=0, backupCount=0,
                         encoding=None, delay=False):
                if encoding is None:
                    raise ValueError("rotation fixture must transport the original encoding")
                self.index = send("open", filename, mode, maxBytes, backupCount, encoding, int(delay))

            def shouldRollover(self, record):
                return send("should", self.index, -1 if record is None else record.index)

            def emit(self, record):
                return send("emit", self.index, record.index)

            def close(self):
                return send("close", self.index)

        try:
            yield SimpleNamespace(handlers=SimpleNamespace(RotatingFileHandler=Handler),
                                  LogRecord=record, makeLogRecord=from_mapping, DEBUG=send("debug")), calls
        finally:
            if process.poll() is None:
                process.stdin.write("stop\n")
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    raise
            stderr = process.stderr.read()
            if process.returncode or stderr:
                raise RuntimeError(f"native handler exit {process.returncode}: {stderr}")


def rotation_cases(toolchain, directory, names=(*NATIVE, *HOST)):
    from test import test_logging

    def execute(case, original):
        return run_case(toolchain, directory, case, original)
    result = {PREFIX + name: execute for name in names if name not in HOST}
    if not any(name in HOST for name in names):
        return result
    worker = directory / "rotation-worker"
    worker.mkdir()
    binary = toolchain.build((HERE / "rotation_driver.py").read_bytes(), worker)
    def host(case, original):
        with native_handlers(binary) as (view, calls), patch.object(test_logging, "logging", view):
            original()
        expected = {"debug": 1, **HOST[case._testMethodName]}
        if calls != expected:
            raise AssertionError((case.id(), calls, expected))
        return {"roles": roles(body="cpython", assertions="cpython", fixture="cpython", operation="imre"),
                "evidence": {"operation": "Native record construction, persistent rotating handler and actual file IO",
                             "fixture": "Original CPython hooks, file and thread lifetime, assertions and cleanup",
                             "native_calls": dict(calls), "opt_level": toolchain.level}}
    result.update({PREFIX + name: host for name in names if name in HOST})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--case", action="append", dest="methods")
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use a test-enabled CPython 3.14.5 with assertions enabled")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    toolchain = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    suite = load_suite(manifest["module"])
    denominator = len(list(cases(suite)))
    with tempfile.TemporaryDirectory() as tmp:
        callbacks = rotation_cases(toolchain, Path(tmp), args.methods or (*NATIVE, *HOST))
        observed, errors, skips = run_in_fixtures(suite, callbacks)
    args.result.write_text(json.dumps({"denominator": denominator, "opt_level": args.opt_level,
                                       "evidence": observed, "errors": errors, "skips": skips}, indent=2) + "\n")
    print(f"-O{args.opt_level}: {len(observed)} original routines, {len(errors)} errors, {len(skips)} skips")
    return int(bool(errors or skips or set(observed) != set(callbacks)))


if __name__ == "__main__":
    sys.exit(main())
