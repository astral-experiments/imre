"""Run the original external-reap signal race with waitpid in the real native parent."""

import argparse
import hashlib
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
from attempt_bodies import fixture, SOURCE
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

NAMES = {"test_send_signal_race", "test_stopped"}
IDS = {"test.test_subprocess.POSIXProcessTestCase." + name for name in NAMES}
ACTOR = b"""import fixture_waitpid
import subprocess
import sys
p=subprocess.Popen(sys.argv[1:])
print(p.pid)
command=input()
while command!="quit":
    if command=="raw":
        result=fixture_waitpid.raw_wait(p.pid, int(input()))
        print(result)
    elif command=="signal":
        p.send_signal(int(input()))
        print("sent")
    elif command=="wait":
        print(p.wait())
    elif command=="status":
        code=p.returncode
        if code is None:
            print("None")
        else:
            print(code)
    command=input()
if p.poll() is None:
    p.kill()
p.wait()
"""


def hash_file(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def build(tc, root):
    site = root / "site"
    site.mkdir()
    (site / "Python.h").write_bytes(
        subprocess.check_output([tc.imre, "extension-header"])
    )
    (site / "fixture_waitpid.pyi").write_text(
        "def raw_wait(pid: int, flags: int, /) -> int: ...\n"
    )
    subprocess.run(
        [
            tc.clang,
            "-std=c11",
            "-O2",
            "-fPIC",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            site,
            "-c",
            HERE / "fixture_waitpid.c",
            "-o",
            root / "fixture.o",
        ],
        check=True,
    )
    subprocess.run(
        ["ar", "rcs", site / "fixture_waitpid.imre.a", root / "fixture.o"], check=True
    )
    (root / "case.py").write_bytes(ACTOR)
    binary = root / "case"
    subprocess.run(
        [
            tc.imre,
            "build",
            root / "case.py",
            "--extension-path",
            site,
            "--runtime",
            tc.runtime,
            "--clang",
            tc.clang,
            "--opt-level",
            str(tc.level),
            "--no-cache",
            "-o",
            binary,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return binary


class Parent:
    def __init__(self, binary, logs, args):
        self.logs = logs
        self.actor = subprocess.Popen(
            [
                "strace",
                "-ff",
                "-qq",
                "-s",
                "768",
                "-e",
                "signal=none",
                "-e",
                "trace=execve,clone,clone3,vfork,fork,wait4,kill,pidfd_send_signal,poll,ppoll,select,pselect6",
                "-o",
                logs / "pid",
                "--",
                binary,
                *args,
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
        )
        self.pid = int(self.read())
        self.wait_requests = []
        [self.parent] = map(
            int,
            Path(f"/proc/{self.actor.pid}/task/{self.actor.pid}/children")
            .read_text()
            .split(),
        )
        assert (
            Path(f"/proc/{self.parent}/exe").samefile(binary)
            and self.parent != self.pid
        )

    def read(self):
        if not select.select([self.actor.stdout], [], [], 30)[0]:
            raise AssertionError("native parent timed out")
        result = self.actor.stdout.readline()
        if not result:
            raise AssertionError(self.actor.stderr.read())
        return result.rstrip(b"\n")

    def request(self, *values):
        self.actor.stdin.write(
            b"".join(str(value).encode() + b"\n" for value in values)
        )
        return self.read()

    def external_wait(self, pid, flags):
        if self.pid != pid or flags not in (0, os.WNOHANG):
            raise AssertionError((pid, flags))
        packed = int(self.request("raw", flags))
        answer = packed >> 16, packed & 65535
        self.wait_requests.append((flags, *answer))
        return answer

    @property
    def returncode(self):
        value = self.request("status")
        return None if value == b"None" else int(value)

    def send_signal(self, sig):
        if self.request("signal", int(sig)) != b"sent":
            raise AssertionError("send_signal failed")

    def wait(self):
        return int(self.request("wait"))

    def close(self):
        self.actor.stdin.write(b"quit\n")
        self.actor.stdin.close()
        if self.actor.wait(timeout=10) or self.actor.stderr.read():
            raise AssertionError("native parent failed")
        self.actor.stdout.close()
        self.actor.stderr.close()

    def trace(self):
        lines = (self.logs / f"pid.{self.parent}").read_text().splitlines()
        if not any(
            re.match(r"(?:clone3?|vfork|fork)\(", s) and re.search(rf"= {self.pid}$", s)
            for s in lines
        ):
            raise AssertionError("child was not launched by the native parent")
        return lines


def measure(tc, binary, root, case, original):
    from test import test_subprocess as suite

    logs = root / case._testMethodName
    logs.mkdir()
    try:
        owner, pre = fixture(case, logs)
        native = native_method(tc, logs, SOURCE, owner, case.id(), pre)
    except (ValueError, RuntimeError) as exc:
        body = {
            "status": "blocked",
            "reason": str(exc).replace(str(logs), "<case>").strip().splitlines()[-1],
        }
    else:
        body = {"status": "completed", **native(case, original)}
    holder = []
    failure = None

    def launch(args):
        if holder or args != suite.ZERO_RETURN_CMD:
            raise AssertionError(("unexpected command", args))
        p = Parent(binary, logs, args)
        holder.append(p)
        return p

    def delegated_wait(pid, flags):
        [p] = holder
        return p.external_wait(pid, flags)

    class SupportOS:
        waitpid = staticmethod(delegated_wait)

        def __getattr__(self, k):
            return getattr(os, k)

    try:
        # Keep support.wait_process and its CPython assertions intact. Only the
        # raw waitpid must run in the kernel parent. Popen.send_signal and wait
        # always run through their real Imre implementations in that parent.
        with (
            patch.object(suite, "subprocess", SimpleNamespace(Popen=launch)),
            patch.object(suite.support, "os", SupportOS()),
        ):
            original()
    except AssertionError as exc:
        if case._testMethodName != "test_stopped" or not holder:
            raise
        failure = str(exc)
    finally:
        for p in holder:
            p.close()
    [p] = holder
    lines = p.trace()
    actual = [s for s in lines if s.startswith(f"wait4({p.pid},")]

    def index(pattern):
        return next(i for i, s in enumerate(actual) if re.search(pattern, s))

    completed = index(rf"WEXITSTATUS\(s\) == 0.*= {p.pid}$")
    echild = index(r"= -1 ECHILD")
    assert completed < echild
    kills = [s for s in lines if re.match(r"(kill|pidfd_send_signal)\(", s)]
    if kills or not any(
        pid == p.pid and status == 0 for _, pid, status in p.wait_requests
    ):
        raise AssertionError(
            (
                "native external reap and kernel signal observation",
                p.wait_requests,
                kills,
            )
        )
    if case._testMethodName == "test_stopped":
        if failure != "0 != -3":
            raise AssertionError(("unexpected stopped-status failure", failure))
    return dict(
        outcome="failure" if failure else "pass",
        failure=failure,
        body_attempt=body,
        roles=dict(
            body="cpython", assertions="cpython", operation="imre", fixture="mixed"
        ),
        external_wait_in_native_parent=True,
        original_support_wait_process=True,
        public_wait_observed_ECHILD=True,
        native_signals_sent=0,
        native_child_exit=0,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for n in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + n, required=True, type=Path)
    parser.add_argument("--opt-level", required=True, type=int, choices=(0, 3))
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        work = Path(tmp)
        binary = build(tc, work)
        positive = work / "positive"
        positive.mkdir()
        p = Parent(
            binary, positive, [sys.executable, "-c", "import time; time.sleep(30)"]
        )
        try:
            p.send_signal(signal.SIGTERM)
            if p.wait() != -signal.SIGTERM:
                raise AssertionError("missing actual signal status")
        finally:
            p.close()
        calls = p.trace()
        if not any(
            s.startswith(f"kill({p.pid}, SIGTERM)") and s.endswith("= 0") for s in calls
        ):
            raise AssertionError("missing positive control for native kill")

        def execute(case, original):
            return measure(tc, binary, work, case, original)

        rows, failed, skipped = run_in_fixtures(
            load_suite(manifest["module"]), dict.fromkeys(IDS, execute)
        )
        if failed or skipped or rows.keys() != IDS:
            raise AssertionError((failed, skipped))
        result = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            actor_sha256=hash_file(binary),
            fixture_waitpid_sha256=hash_file(HERE / "fixture_waitpid.c"),
            positive_live_child=dict(kernel_kill="SIGTERM", wait_status=-15),
            toolchain={
                n: hash_file(getattr(args, n)) for n in ("imre", "runtime", "clang")
            },
            rows=rows,
        )
    args.result.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
