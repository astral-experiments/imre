"""Trace the native parent and child of both unchanged subprocess.call timeout methods."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent

IDS = {
    f"test.test_subprocess.{cls}.test_call_timeout"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
}
ASSERTION = "AssertionError: TimeoutExpired not raised by call"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Traced:
    """Let suite_native build the unchanged method; trace the resulting executable."""

    def __init__(self, toolchain):
        self.toolchain = toolchain
        self.level = toolchain.level

    def build(self, source, directory):
        self.binary = self.toolchain.build(source, directory)
        self.prefix = directory / "trace"
        command = [
            "strace",
            "-ff",
            "-qq",
            "-ttt",
            "-s",
            "2048",
            "-e",
            "trace=execve,kill,wait4",
            "-o",
            self.prefix,
            "--",
            self.binary,
        ]
        wrapper = directory / "observe"
        wrapper.write_text(
            "#!/bin/sh\nexec " + shlex.join(list(map(str, command))) + "\n"
        )
        wrapper.chmod(0o700)
        return wrapper

    def observe(self, executable, *, timed_out):
        # -ff keeps the observations of the native parent and the data child apart.
        traces = {}
        for path in self.prefix.parent.glob("trace.*"):
            traces[int(path.suffix[1:])] = path.read_text().splitlines()
            path.unlink()  # The negative control reuses exactly the same executable.
        (parent,) = (
            pid
            for pid, lines in traces.items()
            if any(f'execve("{self.binary}",' in line for line in lines)
        )
        (child,) = (
            pid
            for pid, lines in traces.items()
            if any(
                f'execve("{executable}",' in line
                and '"-c", "while True: pass"' in line
                and line.endswith("= 0")
                for line in lines
            )
        )
        assert parent != child
        lines = traces[parent]
        (parent_exec,) = (line for line in lines if f'execve("{self.binary}",' in line)
        (child_exec,) = (
            line for line in traces[child] if f'execve("{executable}",' in line
        )
        signals = [line for line in lines if f"kill({child}," in line]
        waits = [line for line in lines if f"wait4({child}," in line]
        (reap,) = (line for line in waits if line.endswith(f"= {child}"))
        if timed_out:
            (kill,) = signals
            assert f"kill({child}, SIGKILL)" in kill and kill.endswith("= 0")
            assert "WTERMSIG(s) == SIGKILL" in reap and lines.index(kill) < lines.index(
                reap
            )
            assert any("WNOHANG" in line and line.endswith("= 0") for line in waits)
            # The exec starts before the timed call, even if the first wait is delayed.
            assert float(kill.split()[0]) - float(parent_exec.split()[0]) >= 0.09
        else:
            assert signals == [] and "WEXITSTATUS(s) == 0" in reap
        return dict(
            parent=parent,
            child=child,
            child_exec=child_exec,
            signals=signals,
            reap=reap,
            pending_waits=sum(line.endswith("= 0") for line in waits),
        )


def main():
    sys.path[:0] = [str(HERE), str(HERE.parents[1])]
    from attempt_bodies import SOURCE, fixture
    from suite_inventory import load_suite, verify_files, verify_interpreter_tests
    from suite_native import Toolchain, native_method, run_in_fixtures

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
        parser.error("use pinned CPython 3.14.5 on Linux with strace")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(
        args.imre.resolve(),
        args.runtime.resolve(),
        args.clang.resolve(),
        args.opt_level,
    )
    rows = {}

    def check(case, original):
        name = case.id()
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            owner, preamble = fixture(case, directory)
            traced = Traced(tc)
            execute = native_method(traced, directory, SOURCE, owner, name, preamble)
            with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
                result = execute(case, original)
            positive = traced.observe(sys.executable, timed_out=True)
            with patch.dict(os.environ, IMRE_PYTHON="/bin/true"):
                try:
                    execute(case, original)
                except AssertionError as exc:
                    failure = str(exc)
                    assert ASSERTION in failure, failure
                else:
                    raise AssertionError(
                        "the original assertion accepted a child that exited"
                    )
            negative = traced.observe("/bin/true", timed_out=False)
            rows[name] = dict(
                method_sha256=result["method_sha256"],
                native_parent_sha256=digest(traced.binary),
                timed_out=positive,
                immediate_exit=negative,
                native_assertion_failure=failure,
            )
            return result

    _, failures, skips = run_in_fixtures(
        load_suite(manifest["module"]), dict.fromkeys(IDS, check)
    )
    if failures or skips or set(rows) != IDS:
        raise ValueError((failures, skips, rows.keys()))
    report = dict(
        cpython_commit=manifest["commit"],
        opt_level=tc.level,
        rows=rows,
        toolchain={k: digest(getattr(tc, k)) for k in ("imre", "runtime", "clang")},
    )
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
