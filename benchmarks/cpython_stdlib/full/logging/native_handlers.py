"""Run the unchanged base Handler naming test with one persistent native object."""
import argparse
import ast
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_evidence import roles
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, run_in_fixtures

ID = "test.test_logging.HandlerTest.test_name"


def handler_cases(toolchain, directory, original):
    binary = toolchain.build((HERE / "handler_driver.py").read_bytes(), directory)

    def drive(case, method):
        calls = []
        with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, encoding="utf-8") as child:
            def request(op, *args):
                calls.append(op)
                for line in (op, *args):
                    if "\n" in line or "\r" in line:
                        raise ValueError("handler driver expects a single line")
                    child.stdin.write(line + "\n")
                child.stdin.flush()
                reply = child.stdout.readline()
                if not reply:
                    raise AssertionError("native Handler exited before acknowledging " + op)
                return reply.removesuffix("\n")

            class Handler:
                def __init__(self):
                    assert request("new") == "ok"

                @property
                def name(self):
                    return ast.literal_eval(request("get"))

                @name.setter
                def name(self, value):
                    assert request("set", value) == "ok"

                def emit(self, record):
                    assert record is None
                    reply = request("emit")
                    if reply.startswith("NotImplementedError:"):
                        raise NotImplementedError(reply.split(":", 1)[1])
                    if reply != "ok":
                        raise AssertionError(reply)

            try:
                with patch.object(original, "logging", SimpleNamespace(Handler=Handler)):
                    method()
            finally:
                if child.poll() is None:
                    child.stdin.write("stop\n")
                child.stdin.close()
                if child.wait(timeout=15) or child.stderr.read():
                    raise AssertionError("native Handler driver failed")
        assert calls == ["new", "set", "get", "set", "get", "emit"], calls
        return {
            "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
            "evidence": {"operation": "Native Handler construction, name writes and reads, and emit",
                         "fixture": "Original BaseTest setup, teardown and cleanups",
                         "opt_level": toolchain.level, "native_calls": calls},
        }
    return {ID: drive}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use a test-enabled CPython 3.14.5 with assertions enabled")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    from test import test_logging
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        tool = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
        sample = HERE / "handler_values.py"
        binary = tool.build(sample.read_bytes(), work)
        check = lambda command: subprocess.run(command, cwd=work, capture_output=True,
                                                check=True, timeout=30)
        expected, actual = check([sys.executable, "-I", "-B", sample]), check([binary])
        assert (expected.stdout, expected.stderr) == (actual.stdout, actual.stderr)
        callbacks = handler_cases(tool, work, test_logging)
        suite = load_suite(manifest["module"])
        denominator = len(list(cases(suite)))
        observed, failures, skips = run_in_fixtures(suite, callbacks)
        if failures or skips or set(observed) != {ID}:
            raise AssertionError((failures, skips, observed))
    report = {"opt_level": args.opt_level, "denominator": denominator, "evidence": observed}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print(f"-O{args.opt_level}: original body and assertions on CPython, six native Handler operations")


if __name__ == "__main__":
    main()
