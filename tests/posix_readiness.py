"""Check native POSIX fd readiness and complete, byte-exact CPython method bodies."""
import argparse
import ast
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parents[1] / "benchmarks/cpython_stdlib/full/select"
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files
sys.path.insert(0, str(HERE))
import selector_methods

# Specialize only the assertion operands. The original method body, fd operation
# and asserted values are copied byte-for-byte from the unmodified test module.
IDENTITY = """    def assertIsNot(self, first: list[int], second: list[int]) -> None:
        if first is second:
            raise AssertionError("unexpectedly identical")
"""
ERRNO = """    def assertEqual(self, first: int | None, second: int) -> None:
        if first is None or first != second:
            raise AssertionError("different errno")
"""
CASES = [
    ("select", "SelectTestCase", "test_errno", ERRNO, "fp.close()", "pass", "exception not raised"),
    ("select", "SelectTestCase", "test_returned_list_identity", IDENTITY,
     "self.assertIsNot(r, w)", "self.assertIsNot(r, r)", "unexpectedly identical"),
    ("poll", "PollTests", "test_poll3", "",
     "ValueError, pollster.register, 0, -1", "ValueError, pollster.register, 0, 1", "ValueError not raised"),
]


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, timeout=kw.pop("timeout", 60), **kw)


def require(cmd, **kw):
    result = run(cmd, **kw)
    if result.returncode:
        raise AssertionError(f"{cmd}: {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def method_source(suite, owner, name, fixture):
    path = HERE / "upstream/Lib/test" / f"test_{suite}.py"
    data = path.read_bytes()
    cls, = (n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == owner)
    node, = (n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == name)
    # The original test_errno's FreeBSD-only decorator does not apply on Linux/macOS.
    if node.decorator_list:
        assert name == "test_errno" and sys.platform in ("linux", "darwin")
    body = b"".join(data.splitlines(keepends=True)[node.lineno-1:node.end_lineno])
    case = f"test.test_{suite}.{owner}.{name}"
    source = ("import errno\nimport select\nimport unittest\n"
              f"class {owner}(unittest.TestCase):\n{fixture}").encode() + body
    return case, body, source + f'\n{owner}().{name}()\nprint("passed {case}")\n'.encode()


@contextmanager
def pipe():
    r, w = os.pipe()
    try:
        yield r, w
    finally:
        os.close(r)
        os.close(w)


def probe(cmd, expected_state=None):
    state = require([*cmd, "state"])
    assert state.endswith("state passed\n"), state
    if expected_state is not None:
        assert state == expected_state, (cmd, state, expected_state)
    assert require([*cmd, "bounds"]) == "bounds passed\n"

    def events(fds):
        words = list(map(int, require([*cmd, "poll", "0",
                         *(str(n) for pair in fds.items() for n in pair)],
                         pass_fds=tuple(fds)).splitlines()))
        return dict(zip(words[::2], words[1::2], strict=True))

    with pipe() as (r, w), pipe() as (idle_r, _):
        interests = {idle_r: select.POLLIN, r: select.POLLIN, w: select.POLLOUT}
        assert events(interests) == {w: select.POLLOUT}
        os.set_blocking(w, False)
        while True:
            try:
                os.write(w, bytes(65536))
            except BlockingIOError:
                break
        assert events(interests) == {r: select.POLLIN}
        os.read(r, 65536)
        masks = events(interests)
        assert masks.get(w, 0) & select.POLLOUT and idle_r not in masks, masks
    with pipe() as (r, w):
        for mode in ("fractional", "select"):
            started = time.monotonic()
            assert require([*cmd, "wait", mode, str(r)], pass_fds=(r,), timeout=5) == "timeout\n"
            assert time.monotonic() - started >= 0.035
        for mode in ("none", "omitted", "negative"):
            with subprocess.Popen([*cmd, "wait", mode, str(r)], pass_fds=(r,),
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as child:
                try:
                    try:
                        child.wait(timeout=0.06)
                    except subprocess.TimeoutExpired:
                        pass
                    else:
                        raise AssertionError(f"{mode} returned while pipe was idle")
                    os.write(w, b"x")
                    out, err = child.communicate(timeout=5)
                    assert (child.returncode, out, err) == (0, "awake\n", "")
                    assert os.read(r, 1) == b"x"
                finally:
                    if child.poll() is None:
                        child.kill()
    return state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-levels", type=int, nargs="+", default=[0, 3])
    parser.add_argument("--host-original", action="store_true", help="requires CPython 3.14.5 with its test package")
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    if args.evidence_dir and (not args.host_original or args.opt_levels != [0, 3] or sys.platform != "linux"):
        parser.error("Linux O0/O3 evidence requires --host-original and both levels")
    if args.host_original and sys.version_info[:3] != (3, 14, 5):
        parser.error("the original host fixture requires CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    expected_state = probe([sys.executable, HERE / "operations.py"])
    reports = {"select": [], "poll": [], "selectors": []}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        def build(source, level):
            exe = work / "native"
            require([args.compiler, "build", source, "--no-cache", "--runtime", args.runtime,
                     "--opt-level", str(level), "--clang", args.clang, "-o", exe])
            return exe
        for level in args.opt_levels:
            observations = {}
            exe = build(HERE / "operations.py", level)
            probe([exe], expected_state)
            if args.host_original:
                for flags in ([], ["--controls"]):
                    print(require([sys.executable, "-B", HERE / "host_poll.py", exe, *flags]).strip())
                observations["test.test_poll.PollTests.test_poll1"] = (
                    {"body": "cpython", "assertions": "cpython", "operation": "imre", "fixture": "mixed"},
                    {"operation": "native poll(2) waits on inherited descriptors; registration state is replayed into each child",
                     "fixture": "CPython runs unchanged test_poll1, including random choices, pipe I/O, assertions and the original unittest lifecycle; dropping writable events fails it"})
                for name, operation, fixture in (
                    ("test.test_poll.PollTests.test_poll_blocks_with_negative_ms", "native poll with None, integer and float negative timeouts", "CPython owns the thread, pipe writes and timing assertions; forcing zero fails the original test"),
                    ("test.test_poll.PollTests.test_poll_c_limits", "native register, modify and poll validate actual C bounds", "CPython imports _testcapi, translates native exceptions and runs assertRaises; suppressing overflow fails the original test"),
                    ("test.test_select.SelectTestCase.test_select", "native select returns integer read fds for the subprocess pipe", "CPython makes the subprocess, converts file objects, reads and asserts; adding a spurious writer fails the original test"),
                ):
                    observations[name] = ({"body": "cpython", "assertions": "cpython", "operation": "imre", "fixture": "mixed"},
                                          {"operation": operation, "fixture": fixture})
                reuse = build(HERE / "selector_reuse.py", level)
                print(require([sys.executable, "-B", HERE / "host_selector_reuse.py", reuse]).strip())
                for owner in ("PollSelectorTestCase", "DefaultSelectorTestCase"):
                    observations[f"test.test_selectors.{owner}.test_unregister_after_fd_close_and_reuse"] = (
                        {"body": "cpython", "assertions": "cpython", "operation": "imre", "fixture": "mixed"},
                        {"operation": "one native selector registers fd numbers and unregisters them after native close and dup2",
                         "fixture": "CPython runs the unchanged method and its cleanups; both processes inherit the same two socketpairs. Duplicate unregister raises native KeyError; omitting dup2 gives native EBADF."})
            for suite, owner, method, fixture, before, after, failure in CASES:
                case, body, source = method_source(suite, owner, method, fixture)
                file = work / "method.py"
                file.write_bytes(source)
                exe = build(file, level)
                assert require([exe]) == f"passed {case}\n"
                assert require([sys.executable, file]) == f"passed {case}\n"
                assert source.count(before.encode()) == 1
                file.write_bytes(source.replace(before.encode(), after.encode()))
                negative = run([build(file, level)])
                assert negative.returncode and failure in negative.stderr, (case, negative)
                ref_negative = run([sys.executable, file])
                assert ref_negative.returncode and failure in ref_negative.stderr, (case, ref_negative)
                observations[case] = (
                    {"body": "imre", "assertions": "imre", "operation": "imre", "fixture": "mixed"},
                    {"operation": "original method body calls the native integer-fd readiness API",
                     "fixture": "host extracts the unchanged body and checks the applicable platform guard; native TestCase" +
                                (" with a typed assertion for the original operands" if fixture else " with callable assertRaises"),
                     "method_sha256": hashlib.sha256(body).hexdigest(), "negative_control": failure})
                print(f"O{level} {case}: passed; control detected", flush=True)
            source = HERE / "selector_operations.py"
            assert require([build(source, level)]) == require([sys.executable, source]) == "selector passed\n"
            sockets = HERE / "selector_sockets.py"
            assert require([build(sockets, level)]) == require([sys.executable, sockets]) == "independent socket readiness passed\n"
            observations.update(selector_methods.exercise(HERE, work, level, build, require, run))
            if args.evidence_dir:
                record_level(reports, observations, level)
    if args.evidence_dir:
        args.evidence_dir.mkdir(parents=True, exist_ok=True)
        for suite, pair in reports.items():
            (args.evidence_dir / f"evidence-{suite}-linux.json").write_text(json.dumps(pair, indent=2) + "\n")


def record_level(reports, observations, level):
    from suite_evidence import build_report
    from suite_inventory import cases, load_suite
    for suite in reports:
        reference = json.loads((HERE / f"reference-{suite}-linux.json").read_bytes())
        found = [(c.id(), "integer-fd readiness") for c in cases(load_suite(reference["module"]))]
        rows = {(name, 1): {"outcome": "pass", "roles": loc, "evidence": proof}
                for name, (loc, proof) in observations.items() if name in reference["reference"]}
        skips = {}
        for name, outcome in reference["reference"].items():
            if isinstance(outcome, dict) and "skipped" in outcome:
                skips[name, 1, level] = {"kind": "upstream-skip", "condition": outcome["skipped"],
                    "reason": outcome["skipped"], "source": f"Lib/test/test_{suite}.py:{name.split('.')[-1]}",
                    "configuration": "CPython regrtest default resources", "platform": "linux",
                    "fixture_variant": "integer-fd readiness"}
        reports[suite].append(build_report(reference, level, found, rows, exceptions=skips))


if __name__ == "__main__":
    main()
