"""Observe the two cat children launched by Imre under the original Linux method."""

# ruff: noqa: E402 -- the shared fixture runner lives two directories above this suite.

import argparse
import ast
import json
import os
from pathlib import Path
import re
import select
import signal
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from launch_errno_originals import digest
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

ID = "test.test_subprocess.POSIXProcessTestCase.test_restore_signals"
ACTOR = b"""import subprocess
import sys
args = sys.argv[1:]
with open("/dev/stdin") as requests, open("/dev/stdout", "w") as replies:
    while True:
        request = requests.readline()
        if not request:
            break
        if request != "0\\n" and request != "1\\n":
            raise ValueError("unexpected restore_signals request")
        result = subprocess.check_output(args, restore_signals=request == "1\\n")
        replies.write(repr(result) + "\\n")
        replies.flush()
"""


def measure(tc, root, binary, case, original):
    from test import test_subprocess as upstream

    with tempfile.TemporaryDirectory(dir=root) as temp:
        work = Path(temp)
        try:
            owner, preamble = fixture(case, work)
            execute = native_method(tc, work, SOURCE, owner, case.id(), preamble)
        except (RuntimeError, ValueError) as exc:
            body = dict(
                status="blocked", reason=str(exc).replace(temp, "<case>").strip()
            )
        else:
            body = dict(status="completed", **execute(case, original))

    actor = None
    calls = []

    def forward(args, *, restore_signals):
        nonlocal actor
        assert args == ["cat", "/proc/self/status"] and type(restore_signals) is bool
        if actor is None:
            actor = subprocess.Popen(
                [
                    "strace",
                    "-ff",
                    "-qq",
                    "-s",
                    "256",
                    "-e",
                    "signal=none",
                    "-e",
                    "trace=execve,clone,clone3,vfork,fork,wait4,rt_sigaction",
                    "-o",
                    root / "pid",
                    "--",
                    binary,
                    *args,
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
        actor.stdin.write(f"{int(restore_signals)}\n")
        actor.stdin.flush()
        assert select.select([actor.stdout], [], [], 15)[0], "native reply timed out"
        data = ast.literal_eval(actor.stdout.readline())
        assert type(data) is bytes
        fields = dict(line.split(b":", 1) for line in data.splitlines() if b":" in line)
        calls.append(
            (
                restore_signals,
                int(fields[b"Pid"]),
                int(fields[b"PPid"]),
                int(fields[b"SigIgn"], 16),
            )
        )
        return data

    try:
        with patch.object(
            upstream, "subprocess", SimpleNamespace(check_output=forward)
        ):
            original()
    finally:
        if actor is not None:
            actor.stdin.close()
            try:
                assert actor.wait(timeout=15) == 0, actor.stderr.read()
            finally:
                if actor.poll() is None:
                    actor.kill()
                    actor.wait()
                actor.stdout.close()
                actor.stderr.close()

    assert [restore for restore, *_ in calls] == [False, True]
    (_, pid1, parent, ignored), (_, pid2, second_parent, restored) = calls
    assert parent == second_parent and len({pid1, pid2, parent}) == 3
    bits = (1 << (signal.SIGPIPE - 1)) | (1 << (signal.SIGXFSZ - 1))
    assert (
        ignored & bits == bits and restored & bits == 0 and ignored ^ restored == bits
    )
    traces = {int(f.suffix[1:]): f.read_text().splitlines() for f in root.glob("pid.*")}
    assert any(
        line.startswith("execve(" + json.dumps(str(binary)) + ",")
        and line.endswith("= 0")
        for line in traces[parent]
    )
    for restore, child, _, _ in calls:
        assert any(
            re.match(r"(clone3?|fork|vfork)\(", line) and line.endswith(f"= {child}")
            for line in traces[parent]
        )
        assert (
            sum(
                line.startswith(f"wait4({child},") and line.endswith(f"= {child}")
                for line in traces[parent]
            )
            == 1
        )
        [executed] = [
            i
            for i, line in enumerate(traces[child])
            if line.startswith("execve(")
            and '["cat", "/proc/self/status"]' in line
            and line.endswith("= 0")
        ]
        before = traces[child][:executed]
        for sig in ["SIGPIPE", "SIGXFSZ"] if restore else ["SIGPIPE"]:
            disposition = "SIG_DFL" if restore else "SIG_IGN"
            assert any(
                line.startswith(f"rt_sigaction({sig}, {{sa_handler={disposition},")
                and line.endswith("= 0")
                for line in before
            ), (sig, before)
    return dict(
        body_attempt=body,
        roles=dict(
            body="cpython", assertions="cpython", operation="imre", fixture="cpython"
        ),
        argv=["cat", "/proc/self/status"],
        native_parent=parent,
        children=[
            dict(
                restore_signals=restore,
                pid=pid,
                ppid=ppid,
                sigign=hex(mask),
                successful_parent_reaps=1,
            )
            for restore, pid, ppid, mask in calls
        ],
        changed_signals=["SIGPIPE", "SIGXFSZ"],
    )


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with (
        tempfile.TemporaryDirectory() as temp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(temp)
        binary = tc.build(ACTOR, root)
        rows, failures, skips = run_in_fixtures(
            load_suite("test.test_subprocess"),
            {ID: lambda case, original: measure(tc, root, binary, case, original)},
        )
        assert list(rows) == [ID] and not failures and not skips, (failures, skips)
        raw = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            actor_sha256=digest(binary),
            rows=rows,
            toolchain={
                k: digest(getattr(args, k)) for k in ("imre", "runtime", "clang")
            },
        )
    args.result.write_text(json.dumps(raw, indent=2) + "\n")
