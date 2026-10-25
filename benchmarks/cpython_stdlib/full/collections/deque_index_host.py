"""Run CPython's randomized deque indexing and deletion methods against a live Imre deque."""

import argparse
from collections import Counter
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, method_bytes, run_in_fixtures


def driver(kind):
    if kind is str:
        construct = "values = deque(input())"
        read = "input()"
    else:
        construct = """if input() == "range":
        start = int(input())
        stop = int(input())
        step = int(input())
        values = deque(range(start, stop, step))
    else:
        values = deque()"""
        read = "int(input())"
    return f"""from collections import deque
import sys

def main() -> None:
    {construct}
    print("ready")
    sys.stdout.flush()
    while True:
        op = input()
        if op == "quit":
            return
        try:
            if op == "get":
                print(values[int(input())])
            elif op == "delete":
                del values[int(input())]
                print("ok")
            elif op == "append":
                values.append({read})
                print("ok")
            elif op == "popleft":
                print(values.popleft())
            elif op == "contains":
                print({read} in values)
            elif op == "length":
                print(len(values))
            else:
                raise ValueError("unknown deque operation")
        except IndexError as exc:
            print("IndexError:" + str(exc))
        sys.stdout.flush()

main()
""".encode()


class LiveDeque:
    """Transport positions and results; all deque state stays in the native child."""

    def __init__(self, program, kind, counters, arguments, cleanup):
        self.kind, self.counters = kind, counters
        self.child = subprocess.Popen([program], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True)
        cleanup.callback(self.close)
        self.send(*arguments)
        if self.read() != "ready":
            raise AssertionError("native deque constructor did not complete")
        counters["construct"] += 1

    def send(self, *lines):
        self.child.stdin.write("".join(f"{line}\n" for line in lines))
        self.child.stdin.flush()

    def read(self):
        if not select.select([self.child.stdout], [], [], 15)[0]:
            raise AssertionError("native deque did not reply")
        line = self.child.stdout.readline()
        if not line:
            raise AssertionError(f"native deque exited: {self.child.stderr.read()}")
        value = line.removesuffix("\n")
        if value.startswith("IndexError:"):
            raise IndexError(value.removeprefix("IndexError:"))
        return value

    def close(self):
        try:
            if self.child.poll() is None:
                self.send("quit")
            out, err = self.child.communicate(timeout=5)
        except (BrokenPipeError, subprocess.TimeoutExpired):
            self.child.kill()
            out, err = self.child.communicate()
        if self.child.returncode or out or err:
            raise AssertionError(f"native deque exit {self.child.returncode}: {out!r} {err!r}")

    def call(self, op, *args):
        self.counters[op] += 1
        self.send(op, *args)
        return self.read()

    def __len__(self):
        return int(self.call("length"))

    def __getitem__(self, at):
        return self.kind(self.call("get", at))

    def __delitem__(self, at):
        if self.call("delete", at) != "ok":
            raise AssertionError("native delete did not complete")

    def __contains__(self, value):
        answer = self.call("contains", value)
        if answer not in ("True", "False"):
            raise AssertionError(answer)
        return answer == "True"

    def append(self, value):
        if self.call("append", value) != "ok":
            raise AssertionError("native append did not complete")

    def popleft(self):
        return self.kind(self.call("popleft"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--compiler-revision")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use test-enabled CPython 3.14.5")
    from test import test_deque

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_deque.__file__).resolve().parents[2], manifest["files"])
    source = HERE / "upstream/Lib/test/test_deque.py"
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as work:
        binaries = {}
        for kind in (str, int):
            directory = Path(work) / kind.__name__
            directory.mkdir()
            binaries[kind] = tools.build(driver(kind), directory)

        def execute(case, original):
            calls = Counter()
            with ExitStack() as cleanup:
                def construct(*args):
                    if not args:
                        kind, inputs = int, ("empty",)
                    elif len(args) == 1 and type(args[0]) is range:
                        r = args[0]
                        kind, inputs = int, ("range", r.start, r.stop, r.step)
                    elif len(args) == 1 and type(args[0]) is str:
                        kind, inputs = str, args
                    else:
                        raise AssertionError(f"unsupported fixture arguments: {args!r}")
                    return LiveDeque(binaries[kind], kind, calls, inputs, cleanup)
                cleanup.callback(test_deque.random.setstate, test_deque.random.getstate())
                test_deque.random.seed(1)
                with patch.object(test_deque, "deque", construct):
                    original()
            proof = {"operation": "All deque operations in this method call a live Imre deque",
                     "fixture": "Original CPython unittest hooks, assertions and seeded random input",
                     "opt_level": args.opt_level, "native_calls": dict(calls),
                     "method_sha256": hashlib.sha256(method_bytes(source, "TestBasic", case._testMethodName)).hexdigest()}
            return {"observation": {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                                    "evidence": proof}}

        methods = {f"test.test_deque.TestBasic.{name}": execute for name in ("test_getitem", "test_delitem")}
        evidence, failures, skips = run_in_fixtures(load_suite("test.test_deque"), methods)
    report = {"cpython_commit": manifest["commit"], "opt_level": args.opt_level,
              "tests": evidence, "failures": failures, "skips": skips}
    if args.compiler_revision:
        report["compiler_revision"] = args.compiler_revision
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(bool(failures or skips) or evidence.keys() != methods.keys())


if __name__ == "__main__":
    sys.exit(main())
