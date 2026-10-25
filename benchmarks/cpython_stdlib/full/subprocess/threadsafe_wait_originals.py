"""Run both unchanged threadsafe_wait occurrences against one native Popen."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

IDS = {
    f"test.test_subprocess.{cls}.test_threadsafe_wait"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
}
ACTOR = b"""import fixture_threads
import subprocess
import sys
p=subprocess.Popen(sys.argv[2:])
print(p.pid)
print(p.returncode is None)
fixture_threads.start(int(sys.argv[1]))
command=input()
while command!="quit":
    if command=="status":
        code=p.returncode
        if code is None:
            print("None")
        else:
            print(code)
    elif command=="waittime":
        print(p.wait(timeout=float(input())))
    elif command=="wait":
        print(p.wait())
    elif command=="poll":
        code=p.poll()
        if code is None:
            print("None")
        else:
            print(code)
    elif command=="kill":
        p.kill()
        print(0)
    command=input()
fixture_threads.join()
if p.poll() is None:
    p.kill()
p.wait()
"""


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def build(tc, root):
    site = root / "site"
    site.mkdir()
    (site / "Python.h").write_bytes(
        subprocess.check_output([tc.imre, "extension-header"])
    )
    (site / "fixture_threads.pyi").write_text(
        "def start(fd: int, /) -> None: ...\ndef join() -> None: ...\n"
    )
    subprocess.run(
        [
            tc.clang,
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fPIC",
            "-pthread",
            "-I",
            site,
            "-c",
            HERE / "fixture_threads.c",
            "-o",
            root / "worker.o",
        ],
        check=True,
    )
    subprocess.run(
        ["ar", "rcs", site / "fixture_threads.imre.a", root / "worker.o"], check=True
    )
    (root / "case.py").write_bytes(ACTOR)
    binary = root / "case"
    ir = root / "case.ll"
    result = subprocess.run(
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
            "--emit-llvm",
            ir,
            "-C",
            "link-arg=-Wl,--wrap=imre_popen_control",
            "-o",
            binary,
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode:
        raise RuntimeError(result.stderr)
    ll = ir.read_text()
    for name in (
        "imre_popen_control",
        "imre_child_wait",
        "imre_child_status",
        "imre_child_signal",
    ):
        if not re.search(rf"\bcall (?:i64|ptr|void) @{name}\(", ll):
            raise AssertionError(("missing compiled public Popen operation", name))
    return binary, sha(ir)


def read(f):
    if not select.select([f], [], [], 20)[0]:
        raise TimeoutError("native Popen RPC timed out")
    s = f.readline()
    if not s:
        raise AssertionError("native Popen closed its channel early")
    return s.rstrip(b"\n")


class Parent:
    def __init__(self, binary, root, args):
        self.root = root
        self.owner = threading.get_ident()
        self.events = []
        sock, other = socket.socketpair()
        self.threadio = sock.makefile("rwb", buffering=0)
        sock.close()
        try:
            self.actor = subprocess.Popen(
                [
                    "strace",
                    "-ff",
                    "-qq",
                    "-ttt",
                    "-s",
                    "768",
                    "-e",
                    "signal=none",
                    "-e",
                    "trace=execve,clone,clone3,fork,vfork,wait4,waitid,kill,pidfd_send_signal,poll,ppoll,select,pselect6",
                    "-o",
                    root / "pid",
                    "--",
                    binary,
                    str(other.fileno()),
                    *args,
                ],
                pass_fds=(other.fileno(),),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                bufsize=0,
            )
        finally:
            other.close()
        self.pid = int(read(self.actor.stdout))
        assert read(self.actor.stdout) == b"True"
        self.tid = int(read(self.threadio))
        [self.parent] = map(
            int,
            Path(f"/proc/{self.actor.pid}/task/{self.actor.pid}/children")
            .read_text()
            .split(),
        )
        assert self.tid != self.parent and self.pid not in (self.parent, self.tid)
        assert Path(f"/proc/{self.parent}/exe").samefile(binary)
        assert f"Tgid:\t{self.parent}\n" in Path(f"/proc/{self.tid}/status").read_text()

    def rpc(self, operation, *args):
        main = threading.get_ident() == self.owner
        channel = self.actor.stdin if main else self.threadio
        start = time.monotonic()
        channel.write(b"".join(str(x).encode() + b"\n" for x in (operation, *args)))
        value = read(self.actor.stdout if main else self.threadio)
        result = None if value == b"None" else int(value)
        self.events.append(
            ("owner" if main else "timer", operation, result, start, time.monotonic())
        )
        return result

    @property
    def returncode(self):
        return self.rpc("status")

    def poll(self):
        return self.rpc("poll")

    def kill(self):
        self.rpc("kill")

    def wait(self, timeout=None):
        return self.rpc("wait") if timeout is None else self.rpc("waittime", timeout)

    def close(self):
        self.threadio.write(b"quit\n")
        self.threadio.close()
        self.actor.stdin.write(b"quit\n")
        self.actor.stdin.close()
        rc = self.actor.wait(timeout=15)
        errors = self.actor.stderr.read()
        self.actor.stdout.close()
        self.actor.stderr.close()
        if rc or errors:
            raise AssertionError((rc, errors))

    def verify(self):
        def trace(tid):
            return (self.root / f"pid.{tid}").read_text().splitlines()

        owner, worker, child = map(trace, (self.parent, self.tid, self.pid))
        assert any(
            "execve(" in s and "time.sleep(12)" in s and s.endswith("= 0")
            for s in child
        )
        assert any(
            "CLONE_THREAD" in s and re.search(rf"= {self.tid}$", s) for s in owner
        )
        assert any(
            re.search(rf"(?:fork|clone3?|vfork)\(.*= {self.pid}$", s)
            and "CLONE_THREAD" not in s
            for s in owner
        )

        def calls(lines, pattern):
            return [s for s in lines if re.search(pattern, s)]

        assert len(calls(worker, rf"kill\({self.pid}, SIGKILL\)\s+= 0$")) == 1
        assert not calls(owner, rf"kill\({self.pid},")
        assert calls(owner, rf"wait4\({self.pid}, .*WNOHANG.*= 0$")
        assert calls(worker, rf"wait4\({self.pid}, .*WNOHANG.*= 0$")
        reaps = calls(
            owner + worker,
            rf"wait4\({self.pid}, .*WTERMSIG\(s\) == SIGKILL.*= {self.pid}$",
        )
        assert len(reaps) == 1, (owner, worker)
        [main] = [e for e in self.events if e[:2] == ("owner", "waittime")]
        [killed] = [e for e in self.events if e[:2] == ("timer", "kill")]
        assert main[3] < killed[3] < main[4]
        worker_ops = [
            (op, value) for who, op, value, *_ in self.events if who == "timer"
        ]
        assert worker_ops == [
            ("poll", None),
            ("kill", 0),
            ("wait", -9),
            ("status", -9),
            ("wait", -9),
            ("status", -9),
        ], worker_ops
        assert main[2] == -9
        return dict(
            owner_timed_wait_overlaps_native_timer=True,
            timer_operations=worker_ops,
            owner_timed_wait=main[2],
            successful_reaps=len(reaps),
            signal="SIGKILL",
            child="CPython 3.14.5 original sleep(12) data program",
        )


def measure(tc, binary, root, case, original):
    from test import test_subprocess as upstream

    logs = root / type(case).__name__
    logs.mkdir()
    try:
        owner, pre = fixture(case, logs)
        compiled = native_method(tc, logs, SOURCE, owner, case.id(), pre)
    except (ValueError, RuntimeError) as exc:
        body = {
            "status": "blocked",
            "reason": str(exc).replace(str(logs), "<case>").strip().splitlines()[-1],
        }
    else:
        body = {"status": "completed", **compiled(case, original)}
    participants = []

    def launch(args):
        assert not participants and args == [
            sys.executable,
            "-c",
            "import time; time.sleep(12)",
        ]
        p = Parent(binary, logs, args)
        participants.append(p)
        return p

    try:
        with patch.object(upstream, "subprocess", SimpleNamespace(Popen=launch)):
            original()
    finally:
        for p in participants:
            p.close()
    [p] = participants
    return dict(
        body_attempt=body,
        roles=dict(
            body="cpython", assertions="cpython", operation="imre", fixture="mixed"
        ),
        observed=p.verify(),
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for n in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + n, type=Path, required=True)
    p.add_argument("--opt-level", type=int, required=True, choices=(0, 3))
    a = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(a.imre, a.runtime, a.clang, a.opt_level)
    with (
        tempfile.TemporaryDirectory() as d,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(d)
        binary, ir = build(tc, root)

        def execute(case, original):
            return measure(tc, binary, root, case, original)

        rows, fail, skip = run_in_fixtures(
            load_suite(manifest["module"]), dict.fromkeys(IDS, execute)
        )
        if fail or skip or rows.keys() != IDS:
            raise AssertionError((fail, skip, rows))
        result = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            actor_sha256=sha(binary),
            llvm_ir_sha256=ir,
            fixture_threads_sha256=sha(HERE / "fixture_threads.c"),
            toolchain={n: sha(getattr(a, n)) for n in ("imre", "runtime", "clang")},
            rows=rows,
        )
    a.result.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
