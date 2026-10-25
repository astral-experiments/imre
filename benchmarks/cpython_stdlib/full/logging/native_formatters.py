"""Exercise untouched formatter tests without a host implementation of record or formatter operations."""
import argparse
import ast
from collections import Counter
from contextlib import contextmanager
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_evidence import roles
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
PRECISION = "test.test_logging.FormatterTest.test_issue_89047"


def buffering_cases(toolchain, work):
    fixture = """def NativeCase() -> BufferCase:
    case = BufferCase()
    case.setUp()
    return case


class BufferCase:
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        self.records = []

    def assertEqual(self, expected: str, actual: str) -> None:
        if expected != actual:
            raise AssertionError(f"{expected!r} != {actual!r}")
"""
    owner = "BufferingFormatterTest"
    fixture += "\n" + method_bytes(SOURCE, owner, "setUp").decode()
    callbacks = {}
    for name in ("test_default", "test_custom"):
        support = "import logging\n\n"
        if name == "test_custom":
            support += "class TestBufferingFormatter(logging.BufferingFormatter):\n"
            for helper in ("formatHeader", "formatFooter"):
                support += method_bytes(SOURCE, "TestBufferingFormatter", helper).decode() + "\n"
        dest = work / name
        dest.mkdir()
        case_id = f"test.test_logging.{owner}.{name}"
        callbacks[case_id] = native_method(
            toolchain, dest, SOURCE, owner, case_id, support + fixture,
            operation=("imre", "Native buffered record formatting; original record setup also runs in the child"))
    return callbacks


def optional(value):
    return (0,) if value is None else (1, value)


@contextmanager
def native_logging(binary):
    called = Counter()
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          text=True, encoding="utf-8") as process:
        def send(op, *args):
            lines = list(map(str, (op, *args)))
            if any("\n" in line or "\r" in line for line in lines):
                raise ValueError("unsupported multiline input to formatter test transport")
            called[op] += 1
            process.stdin.write("\n".join(lines) + "\n")
            process.stdin.flush()
            answer = process.stdout.readline()
            if not answer:
                raise RuntimeError(f"formatter process exited {process.poll()}")
            for error in (ValueError, TypeError):
                flag = "!" + error.__name__ + " "
                if answer.startswith(flag):
                    raise error(ast.literal_eval(answer[len(flag):]))
            return ast.literal_eval(answer)

        class Record:
            def __init__(self, fields):
                if fields.keys() == {"msg"}:
                    handle = send("record", "message", fields["msg"])
                else:
                    keys = {"name", "level", "pathname", "lineno", "exc_info", "func", "msg", "args"}
                    if fields.keys() not in (keys, keys | {"custom"}) or fields["exc_info"] is not None or fields["func"] is not None:
                        raise ValueError("unhandled record shape in formatter transport")
                    extra = (1, fields["custom"]) if "custom" in fields else (0,)
                    handle = send("record", "common", fields["name"], fields["level"], fields["pathname"],
                                  fields["lineno"], fields["msg"], *fields["args"], *extra)
                object.__setattr__(self, "handle", handle)
            def __setattr__(self, name, value):
                if name == "created":
                    send("created", self.handle, value)
                elif name == "msecs" and type(value) in (int, float):
                    send("msecs-" + type(value).__name__, self.handle, value)
                else:
                    raise AttributeError(name)
            @property
            def asctime(self):
                return send("asctime", self.handle)

        class Formatter:
            def __init__(self, fmt=None, datefmt=None, style="%", validate=True, *, defaults=None):
                mapping = (-1,) if defaults is None else (len(defaults), *(v for pair in defaults.items() for v in pair))
                self.handle = send("formatter", *optional(fmt), *optional(datefmt), style, int(validate), *mapping)
            @property
            def _fmt(self):
                return send("_fmt", self.handle)
            def usesTime(self):
                return send("usesTime", self.handle)
            def format(self, record):
                return send("format", self.handle, record.handle)
            def formatTime(self, record, datefmt=None):
                return send("formatTime", self.handle, record.handle, *optional(datefmt))
            def __setattr__(self, name, value):
                if name == "converter":
                    if value not in (time.gmtime, time.localtime):
                        raise ValueError("unhandled clock converter in transport")
                    send("converter", self.handle, value.__name__)
                else:
                    object.__setattr__(self, name, value)
        try:
            yield SimpleNamespace(makeLogRecord=Record, Formatter=Formatter), called
        finally:
            if process.poll() is None:
                process.stdin.write("stop\n")
                process.stdin.close()
            if process.wait(timeout=10):
                raise RuntimeError(f"formatter process exited {process.returncode}")


def host_formatter_cases(toolchain, binary, original):
    def execute(case, method):
        with native_logging(binary) as (logging, called), patch.object(original, "logging", logging):
            method()
        if not called:
            raise AssertionError("no native operations")
        fixture = "Original CPython hooks, test method and assertions"
        return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                "fixture_execution": fixture, "native_calls": dict(called),
                "observation": {"roles": roles(body="cpython", assertions="cpython", fixture="cpython", operation="imre"),
                                "evidence": {"operation": "Native Formatter and LogRecord operations", "fixture": fixture,
                                             "native_calls": dict(called), "opt_level": toolchain.level}}}
    names = ("percent", "dollars", "braces", "format_validate", "defaults_parameter", "invalid_style", "time", "issue_89047")
    return {"test.test_logging.FormatterTest.test_" + name: execute for name in names}


def precision_case(toolchain, work):
    fixture = """import logging
import time

class NativeCase:
    def assertNotIn(self, member: str, container: str) -> None:
        if member in container:
            raise AssertionError(f"{member!r} unexpectedly found in {container!r}")
"""
    return native_method(toolchain, work, SOURCE, "FormatterTest", PRECISION, fixture,
                         operation=("imre", "Native record timestamps, formatting and all 2,500 precision assertions"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("requires the pinned test-enabled CPython")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    suite = load_suite("test.test_logging")
    assert len(list(cases(suite))) == 272
    from test import test_logging
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as work:
        binary = tools.build((HERE / "formatter_driver.py").read_bytes(), Path(work))
        values = Path(work) / "values"
        values.mkdir()
        source_values = HERE / "formatter_values.py"
        program = tools.build(source_values.read_bytes(), values)
        def output(command):
            done = subprocess.run(command, capture_output=True, timeout=30)
            if done.returncode:
                raise AssertionError((command, done.returncode, done.stdout, done.stderr))
            return done.stdout, done.stderr
        actual = output([program])
        wanted = output([sys.executable, source_values])
        if actual != wanted:
            raise AssertionError((actual, wanted))
        executions = host_formatter_cases(tools, binary, test_logging)
        evidence, failures, skips = run_in_fixtures(suite, executions)
        if failures or skips or set(evidence) != set(executions):
            raise AssertionError((failures, skips, set(executions) - set(evidence)))
        body_dir = Path(work) / "precision"
        body_dir.mkdir()
        body = precision_case(tools, body_dir)
        native, failures, skips = run_in_fixtures(load_suite("test.test_logging"), {PRECISION: body})
        if failures or skips or set(native) != {PRECISION}:
            raise AssertionError((failures, skips))
        evidence[PRECISION].extend(native[PRECISION])
        buffering = buffering_cases(tools, Path(work))
        native, failures, skips = run_in_fixtures(load_suite("test.test_logging"), buffering)
        if failures or skips or set(native) != set(buffering):
            raise AssertionError((failures, skips))
        evidence.update(native)
    data = json.dumps({"optimization": args.opt_level, "reference_denominator": 272, "evidence": evidence},
                      indent=2, sort_keys=True) + "\n"
    if args.result:
        args.result.write_text(data)
    else:
        print(data, end="")


if __name__ == "__main__":
    main()
