"""Exercise the original randomized deque reversal through a live native deque."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import random
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

CASE = "test.test_deque.TestBasic.test_reverse"
SCALE = 1 << 53  # random.random() returns an exact multiple of 2**-53.
SEED = 314005


class Session:
    """Each original deque has its own handle in the child. Only commands and observations cross the pipe."""

    def __init__(self, binary):
        self.child = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True)
        self.calls = Counter()
        self.last_inputs = []

    def exchange(self, operation, *lines):
        self.calls[operation] += 1
        self.child.stdin.write("".join(f"{line}\n" for line in (operation, *lines)))
        self.child.stdin.flush()
        if not select.select([self.child.stdout], [], [], 20)[0]:
            raise TimeoutError(f"no native response to {operation}")
        line = self.child.stdout.readline()
        if not line:
            raise AssertionError(f"native deque exited during {operation}: {self.child.stderr.read()}")
        return line.removesuffix("\n")

    def __call__(self, values):
        seed = [int(value * SCALE) for value in values]
        if any(type(value) is not float or not 0 <= value < 1 or word / SCALE != value
               for value, word in zip(values, seed, strict=True)):
            raise ValueError("unexpected random fixture value")
        self.last_inputs = seed
        return Handle(self, int(self.exchange("new", len(seed), *seed)))

    def close(self):
        try:
            out, err = self.child.communicate("quit\n", timeout=5)
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.communicate()
            raise
        if self.child.returncode or out or err:
            raise AssertionError(f"native deque exit {self.child.returncode}: {out!r} {err!r}")


class Handle:
    def __init__(self, session, key):
        self.session = session
        self.key = key

    def __len__(self):
        return int(self.session.exchange("length", self.key))

    def __iter__(self):
        words = json.loads(self.session.exchange("snapshot", self.key))
        return iter(word / SCALE for word in words)

    def reverse(self, *args):
        if not args:
            value = self.session.exchange("reverse", self.key)
            if value != "None":
                raise AssertionError(f"unexpected native return {value!r}")
            return None
        if len(args) != 1 or type(args[0]) is not int:
            raise ValueError("unexpected upstream call arguments")
        value = self.session.exchange("bad-arity", self.key, args[0])
        if not value.startswith("TypeError:"):
            raise AssertionError(f"native invalid call did not raise TypeError: {value}")
        raise TypeError(value.removeprefix("TypeError:"))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    p.add_argument("--compiler-revision", required=True)
    args = p.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("use a test-enabled CPython 3.14.5")
    from test import test_deque

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_deque.__file__).resolve().parents[2], manifest["files"])
    body = method_bytes(HERE / "upstream/Lib/test/test_deque.py", "TestBasic", "test_reverse")
    method = hashlib.sha256(body).hexdigest()
    toolchain = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        program = toolchain.build((HERE / "deque_reverse_driver.py").read_bytes(), work)

        def probe(binary):
            def execute(case, original):
                session = Session(binary)
                saved_state = random.getstate()
                try:
                    random.seed(SEED)
                    with patch.object(test_deque, "deque", session):
                        original()
                finally:
                    random.setstate(saved_state)
                    session.close()
                calls = dict(session.calls)
                if calls != {"new": 500, "reverse": 1000, "snapshot": 1000, "length": 1000, "bad-arity": 1}:
                    raise ValueError(f"incomplete native operations: {calls}")
                detail = {"operation": "Native deque construction, reversal, iteration, length, and arity errors.",
                          "fixture": "CPython runs the original body, random source, expected lists, assertions and fixture hooks.",
                          "native_calls": calls, "method_sha256": method, "random_seed": SEED,
                          "opt_level": toolchain.level,
                          "longest_input_count": len(session.last_inputs),
                          "longest_input_sha256": hashlib.sha256(json.dumps(session.last_inputs).encode()).hexdigest()}
                return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                        "fixture_execution": "CPython module/class/instance hooks", "method_sha256": method,
                        "observation": {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                                        "evidence": detail}}
            return run_in_fixtures(load_suite("test.test_deque"), {CASE: execute})

        evidence, failures, skips = probe(program)
        control = {"result": "unavailable", "reason": "the negative control uses GNU --wrap"}
        if not failures and not skips and sys.platform.startswith("linux"):
            noop = work / "noop.c"
            noop.write_text("void __wrap_imre_deque_reverse(void *deque) {(void)deque;}\n")
            negative = work / "noop-reverse"
            subprocess.run([toolchain.imre, "build", work / "case.py", "--no-cache", "--runtime", toolchain.runtime,
                            "--clang", toolchain.clang, "--opt-level", str(toolchain.level),
                            "-C", "link-arg=-Wl,--wrap=imre_deque_reverse", "-C", f"link-arg={noop}", "-o", negative],
                           capture_output=True, check=True, timeout=180)
            _, wrong, skipped = probe(negative)
            if (skipped or len(wrong) != 1 or wrong[0][0] != CASE or "line 417, in test_reverse" not in wrong[0][1]
                    or "AssertionError: Lists differ:" not in wrong[0][1]):
                raise AssertionError((wrong, skipped))
            control = {"result": "failed-as-expected", "upstream_line": 417, "wrapped": "imre_deque_reverse"}
    report = {"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
              "opt_level": args.opt_level, "tests": evidence, "failures": failures, "skips": skips, "negative_control": control}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(bool(failures or skips) or set(evidence) != {CASE})


if __name__ == "__main__":
    sys.exit(main())
