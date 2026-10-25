"""Observe the original raw pipe read and the host's select call separately on Linux."""

# Shared runner modules live two directories above the vendored suite.
# ruff: noqa: E402

import argparse
from array import array
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import sys
import tempfile
import termios
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from host_popen import Harness, verify
from suite_inventory import load_suite
from suite_native import Toolchain, native_method, run_in_fixtures

ID = "test.test_subprocess.POSIXProcessTestCase.test_select_unbuffered"
HELD = "import os; os.write(1, b'apple'); os.read(0, 1)"


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


class ReadPipe:
    """Only the native stream reads. Host select watches the same kernel pipe."""

    def __init__(self, remote, actor_pid):
        self.stream = remote.stdout
        self.native_fd = remote.request("stdout.fileno")
        path = Path(f"/proc/{actor_pid}/fd/{self.native_fd}")
        self.fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
        opened = os.fstat(self.fd)
        assert stat.S_ISFIFO(opened.st_mode) and os.path.samestat(opened, path.stat())
        self.pipe = os.readlink(path)
        self.fileno_calls = 0
        self.reads = []

    def read(self, size):
        result = self.stream.read(size)
        queued = array("i", [0])
        fcntl.ioctl(self.fd, termios.FIONREAD, queued, True)
        self.reads.append((size, result.decode("ascii"), queued[0]))
        return result

    def fileno(self):
        self.fileno_calls += 1
        return self.fd

    def close(self):
        try:
            self.stream.close()
        finally:
            if self.fd >= 0:
                os.close(self.fd)
                self.fd = -1


class Probe:
    def __init__(self, harness, root, argv, kwargs):
        spawn = subprocess.Popen
        self.executable = None

        def trace(command, *args, **options):
            if "pass_fds" not in options:
                return spawn(command, *args, **options)
            assert self.executable is None
            self.executable = Path(command[0])
            return spawn(
                [
                    "strace",
                    "-ff",
                    "-qq",
                    "-yy",
                    "-s",
                    "256",
                    "-e",
                    "signal=none",
                    "-e",
                    "trace=execve,clone,clone3,fork,vfork,read,write,close,wait4,waitid",
                    "-o",
                    root / "pid",
                    "--",
                    *command,
                ],
                *args,
                **options,
            )

        self.root, self.argv = root, list(argv)
        root.mkdir()
        with patch.object(subprocess, "Popen", trace):
            self.remote = harness.Popen(argv, **kwargs)
        tracer = self.remote.actor.pid
        [self.pid] = map(
            int, Path(f"/proc/{tracer}/task/{tracer}/children").read_text().split()
        )
        assert self.pid != self.remote.pid and Path(f"/proc/{self.pid}/exe").samefile(
            self.executable
        )
        self.pipe = ReadPipe(self.remote, self.pid)
        self.remote.stdout = self.pipe
        self.selects = []
        self.alive_at_select = None

    def watch(self, real_select):
        def watch(r, w, x, timeout=None):
            result = real_select(r, w, x, timeout)
            if len(r) == 1 and r[0] is self.pipe:
                assert w == x == [] and timeout == 0.0
                self.selects.append(result[0] == [self.pipe])
            return result

        return watch

    def finish(self):
        try:
            if self.remote.stdin is not None:
                self.remote.stdin.close()
            if self.pipe.fd >= 0:
                self.pipe.close()
        finally:
            self.remote.finish()

    def observe(self, expected):
        queued = int(expected != "buffered-held")
        assert self.pipe.reads == [(4, "appl", queued)]
        assert self.pipe.fileno_calls == 1 and self.selects == [bool(queued)]
        assert self.pipe.fd == -1
        parent = (self.root / f"pid.{self.pid}").read_text().splitlines()
        child = (self.root / f"pid.{self.remote.pid}").read_text().splitlines()
        n = self.remote.pid
        assert any(
            re.match(r"(?:clone3?|vfork|fork)\(", s) and s.endswith(f"= {n}")
            for s in parent
        )
        assert any(
            s.startswith("execve(" + json.dumps(self.argv[0]) + ",")
            and "apple" in s
            and s.endswith("= 0")
            for s in child
        )
        assert any(
            re.match(rf"wait4\({n}, .*WEXITSTATUS\(s\) == 0.*= {n}$", s) for s in parent
        )
        prefix = f"read({self.pipe.native_fd}<{self.pipe.pipe}>, "
        drain = [s for s in parent if s.startswith(prefix)]
        assert len(drain) == 1, drain
        read = re.fullmatch(
            r'"(appl|apple)", (\d+)\)\s+= (\d+)', drain[0][len(prefix) :]
        )
        assert read, drain
        got, requested, count = read[1], int(read[2]), int(read[3])
        if expected == "buffered-held":
            assert got == "apple" and requested > 4 and count == 5
        else:
            assert (got, requested, count) == ("appl", 4, 4)
        assert any(
            s.startswith(f"close({self.pipe.native_fd}<{self.pipe.pipe}>)")
            and s.endswith("= 0")
            for s in parent
        )
        return dict(
            native_parent_sha256=sha(self.executable),
            data_child_argv=self.argv,
            native_read_request=requested,
            native_read_result=count,
            native_returned="appl",
            unread_kernel_bytes=queued,
            host_select_returned_original_stream=bool(queued),
            data_child_alive_at_select=self.alive_at_select,
            native_pipe_closes=1,
            native_reaps=1,
        )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = p.parse_args()
    manifest = verify()
    from test import test_subprocess as upstream

    tc = Toolchain(args.imre, args.runtime, args.clang, int(args.opt_level))
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(tmp)
        args.actors = root
        harness = Harness(args, root)
        actors = {}

        def measure(case, original):
            d = root / "body"
            d.mkdir()
            try:
                owner, pre = fixture(case, d)
                full = native_method(tc, d, SOURCE, owner, case.id(), pre)
            except (ValueError, RuntimeError) as exc:
                body = dict(
                    status="blocked",
                    reason=str(exc).replace(str(d), "<case>").strip().splitlines()[-1],
                )
            else:
                body = dict(status="completed", **full(case, original))

            def launch(argv, **kwargs):
                assert (
                    not actors
                    and kwargs == dict(stdout=subprocess.PIPE, bufsize=0)
                    and argv[0] == sys.executable
                )
                act = actors["original"] = Probe(
                    harness, root / "original", argv, kwargs
                )
                return act.remote

            real_select = select.select

            def watch(r, w, x, timeout=None):
                if "original" in actors:
                    return actors["original"].watch(real_select)(r, w, x, timeout)
                return real_select(r, w, x, timeout)

            with (
                patch.object(
                    upstream,
                    "subprocess",
                    SimpleNamespace(Popen=launch, PIPE=subprocess.PIPE),
                ),
                patch.object(select, "select", watch),
            ):
                original()
            return dict(
                body_attempt=body,
                roles=dict(
                    body="cpython",
                    assertions="cpython",
                    operation="mixed",
                    fixture="cpython",
                ),
            )

        try:
            rows, failures, skips = run_in_fixtures(
                load_suite(manifest["module"]), {ID: measure}
            )
            if failures or skips or rows.keys() != {ID}:
                raise AssertionError((failures, skips))
            original = actors["original"]
            assert original.pipe.fd == -1, (
                "upstream addCleanup must close the native pipe"
            )
        finally:
            if "original" in actors:
                actors["original"].finish()
        rows[ID][0]["observed"] = original.observe("original")
        controls = {}
        for name, size in (("raw-held", 0), ("buffered-held", -1)):
            act = Probe(
                harness,
                root / name,
                [sys.executable, "-c", HELD],
                dict(stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=size),
            )
            try:
                assert act.pipe.read(4) == b"appl"
                with patch.object(select, "select", act.watch(select.select)):
                    select.select([act.pipe], [], [], 0.0)
                act.alive_at_select = act.remote.poll() is None
                assert act.alive_at_select
                act.remote.stdin.close()
                assert act.remote.wait() == 0
            finally:
                act.finish()
            controls[name] = act.observe(name)
        report = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            toolchain={k: sha(getattr(args, k)) for k in ("imre", "runtime", "clang")},
            rows=rows,
            controls=controls,
        )
        args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
