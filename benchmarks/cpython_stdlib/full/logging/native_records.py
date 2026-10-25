"""Exercise complete CPython record methods under the shared upstream fixture runner."""
import argparse
import ast
from contextlib import contextmanager
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures

REPR_FIXTURE = """import logging

class NativeCase:
    def assertStartsWith(self, text: str, prefix: str) -> None:
        if not text.startswith(prefix):
            raise AssertionError(f"{text!r} does not start with {prefix!r}")

    def assertEndsWith(self, text: str, suffix: str) -> None:
        if not text.endswith(suffix):
            raise AssertionError(f"{text!r} does not end with {suffix!r}")
"""


@contextmanager
def native_flags(binary):
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          text=True, encoding="utf-8") as child:
        called = []
        def invoke(command, *args):
            called.append(command)
            child.stdin.write("\n".join(map(str, (command, *args))) + "\n")
            child.stdin.flush()
            return ast.literal_eval(child.stdout.readline())

        class Record:
            def __init__(self, handle):
                self.handle = handle
            def __getattr__(self, name):
                return invoke("field", self.handle, name)

        class Logging:
            def __getattr__(self, name):
                return invoke("get", name)
            def __setattr__(self, name, value):
                invoke("set", name, value)
            def makeLogRecord(self, fields):
                if fields:
                    raise ValueError("metadata fixture requires an empty record mapping")
                return Record(invoke("new"))

        try:
            yield Logging(), called
        finally:
            if child.poll() is None:
                child.stdin.write("stop\n")
                child.stdin.close()
            if child.wait(timeout=10):
                raise RuntimeError(f"native record driver exited {child.returncode}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--result", type=Path)
    parser.add_argument("--cpython-source", type=Path)
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
    denom = len(list(cases(suite)))
    assert denom == 272
    from test import test_logging
    toolchain = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    source = STDLIB / "upstream/Lib/test/test_logging.py"
    prefix = "test.test_logging.LogRecordTest."
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        child = work / "driver"
        child.mkdir()
        binary = toolchain.build((HERE / "record_driver.py").read_bytes(), child)
        values = work / "values"
        values.mkdir()
        source_values = HERE / "record_values.py"
        program = toolchain.build(source_values.read_bytes(), values)
        def output(command):
            done = subprocess.run(command, capture_output=True, timeout=30)
            if done.returncode:
                raise AssertionError((command, done.returncode, done.stdout, done.stderr))
            return done.stdout, done.stderr
        if output([program]) != output([sys.executable, source_values]):
            raise AssertionError("record field ownership or formatting differs from CPython")
        def optional(case, method):
            with native_flags(binary) as (logging, operations), patch.object(test_logging, "logging", logging):
                method()
            if not operations:
                raise AssertionError("no native logging operation executed")
            return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                    "fixture_execution": "CPython module/class/instance hooks", "operations": operations}
        executions = {prefix + "test_optional": optional}
        executions[prefix + "test_str_rep"] = native_method(toolchain, work, source, "LogRecordTest",
                                                          prefix + "test_str_rep", REPR_FIXTURE)
        evidence, failures, skips = run_in_fixtures(suite, executions)
        if failures or skips or set(evidence) != set(executions):
            raise AssertionError((failures, skips, set(evidence)))
    report = {"optimization": args.opt_level, "reference_denominator": denom, "evidence": evidence}
    out = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.result:
        args.result.write_text(out)
    else:
        print(out, end="")


if __name__ == "__main__":
    main()
