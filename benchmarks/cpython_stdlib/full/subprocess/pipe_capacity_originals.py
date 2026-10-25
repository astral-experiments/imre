"""Measure the kernel pipe capacity inspected by each original Popen test."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shlex
import stat
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from host_popen import Harness, Stream, source, verify
from suite_inventory import load_suite
from suite_native import Toolchain, native_method, run_in_fixtures

STREAMS = ("stdin", "stdout", "stderr")
IDS = {
    f"test.test_subprocess.{cls}.{name}"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
    for name in ("test_pipesizes", "test_pipesize_default")
}
TRACE = "execve,clone,clone3,fork,vfork,pipe,pipe2,fcntl,write,close,kill,wait4,waitid,poll,ppoll,select,pselect6"


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


class TracedHarness(Harness):
    def actor(self, keywords, layout):
        self.binary = self.args.build(source(keywords, layout).encode(), self.work)
        wrapper = self.work / "traced"
        command = [
            "strace",
            "-ff",
            "-qq",
            "-yy",
            "-s",
            "768",
            "-e",
            "signal=none",
            "-e",
            "trace=" + TRACE,
            "-o",
            str(self.work / "pid"),
            "--",
            str(self.binary),
        ]
        wrapper.write_text("#!/bin/sh\nexec " + shlex.join(command) + ' "$@"\n')
        wrapper.chmod(0o700)
        return wrapper


class Pipe(Stream):
    def fileno(self):
        fd = self.parent.request(self.name + ".fileno")
        path = Path(f"/proc/{self.owner}/fd/{fd}")
        # Capacity belongs to the pipe inode. This opens a separate description
        # just for F_GETPIPE_SZ; no read, write or stream close goes through it.
        flags = os.O_WRONLY if self.name == "stdin" else os.O_RDONLY
        alias = os.open(path, flags | os.O_NONBLOCK | os.O_CLOEXEC)
        st = os.fstat(alias)
        assert stat.S_ISFIFO(st.st_mode) and st.st_ino == path.stat().st_ino
        self.aliases[alias] = (self.name, fd, st.st_ino)
        return alias


def measure(tc, root, case, orig, upstream):
    d = root / case.id()
    d.mkdir()
    try:
        cls, prep = fixture(case, d)
        native = native_method(tc, d, SOURCE, cls, case.id(), prep)
    except (ValueError, RuntimeError) as exc:
        body = dict(status="blocked", reason=str(exc).strip().splitlines()[-1])
    else:
        body = dict(status="completed", **native(case, orig))
    expected = (
        selectors.SelectSelector
        if type(case).__name__.endswith("NoPoll")
        else selectors.PollSelector
    )
    assert upstream.subprocess._PopenSelector is expected
    h = TracedHarness(tc, d)
    aliases, pipes, baseline = {}, {}, []

    def launch(argv, **kw):
        nonlocal requested, owner
        assert not h.parents and argv[:2] == [sys.executable, "-c"] and len(argv) == 3
        assert set(kw) == {*STREAMS, "pipesize"} and all(
            kw[s] == h.PIPE for s in STREAMS
        )
        requested = kw["pipesize"]
        p = h.Popen(argv, **kw)
        [owner] = map(
            int,
            Path(f"/proc/{p.actor.pid}/task/{p.actor.pid}/children")
            .read_text()
            .split(),
        )
        assert Path(f"/proc/{owner}/exe").samefile(h.binary)
        for name in STREAMS:
            f = Pipe(p, name)
            f.owner, f.aliases = owner, aliases
            setattr(p, name, f)
        return p

    def get(fd, cmd):
        assert cmd == fcntl.F_GETPIPE_SZ
        if fd not in aliases:
            size = fcntl.fcntl(fd, cmd)
            baseline.append(size)
            return size
        name, remote, inode = aliases.pop(fd)
        try:
            assert name not in pipes
            size = fcntl.fcntl(fd, cmd)
            pipes[name] = (remote, inode, size)
            return size
        finally:
            os.close(fd)

    requested = owner = None
    try:
        with (
            patch.object(
                upstream, "subprocess", SimpleNamespace(Popen=launch, PIPE=h.PIPE)
            ),
            patch.object(
                upstream,
                "fcntl",
                SimpleNamespace(fcntl=get, F_GETPIPE_SZ=fcntl.F_GETPIPE_SZ),
            ),
        ):
            orig()
        [p] = h.parents
        assert not aliases and list(pipes) == list(STREAMS)
        assert len({inode for _, inode, _ in pipes.values()}) == 3
        for fd, _, _ in pipes.values():
            assert not Path(f"/proc/{owner}/fd/{fd}").exists()
    finally:
        for fd in aliases:
            os.close(fd)
        for p in h.parents:
            p.finish()

    parent = (d / f"pid.{owner}").read_text().splitlines()
    child = (d / f"pid.{p.pid}").read_text().splitlines()
    assert any(
        line.startswith("execve(" + json.dumps(str(h.binary)) + ",")
        and line.endswith("= 0")
        for line in parent
    )
    assert any(
        line.startswith("execve(" + json.dumps(sys.executable) + ",")
        and "sys.stdin.read()" in line
        and line.endswith("= 0")
        for line in child
    )
    [spawn] = [
        i
        for i, s in enumerate(parent)
        if re.match(r"(clone3?|vfork|fork)\(", s) and s.endswith(f"= {p.pid}")
    ]
    sets = [
        (
            i,
            re.match(
                r"fcntl\(\d+<pipe:\[(\d+)\]>, F_SETPIPE_SZ, (\d+)\)\s+= (\d+)$", s
            ),
        )
        for i, s in enumerate(parent)
        if "F_SETPIPE_SZ," in s
    ]
    assert not any(
        s.startswith(("poll(", "ppoll(", "select(", "pselect6(")) for s in parent
    )
    [reap] = [
        s
        for s in parent
        if s.startswith(f"wait4({p.pid},") and s.endswith(f"= {p.pid}")
    ]
    sent = any(
        s.startswith(f"kill({p.pid}, SIGKILL)") and s.endswith("= 0") for s in parent
    )
    # The original closes all three streams before kill() in the positive-size
    # case. If the data child exits first, Popen.kill() may reap it while polling.
    assert "kill" in h.calls and (
        sent or (requested > 0 and "WIFEXITED" in reap and "WNOHANG" in reap)
    )
    sizes = {k: v[2] for k, v in pipes.items()}
    if requested == -1:
        assert not sets and len(baseline) == 2 and set(sizes.values()) == set(baseline)
        assert "exit" in h.calls
    else:
        assert (
            len(baseline) == 1
            and requested == baseline[0] // 2
            and set(sizes.values()) == {requested}
        )
        assert len(sets) == 3 and all(
            i < spawn and m and int(m[2]) == int(m[3]) == requested for i, m in sets
        )
        assert {int(m[1]) for _, m in sets} == {v[1] for v in pipes.values()}
        stdin = pipes["stdin"]
        assert any(
            re.match(rf'write\({stdin[0]}<pipe:\[{stdin[1]}\]>, "pear", 4\)\s+= 4$', s)
            for s in parent
        )
        assert all(
            x in h.calls
            for x in (
                "stdin.write",
                "stdin.close",
                "stdout.close",
                "stderr.close",
                "wait",
            )
        )
    return dict(
        body_attempt=body,
        roles=dict(
            body="cpython", assertions="cpython", operation="imre", fixture="mixed"
        ),
        observed=dict(
            actor_sha256=sha(h.binary),
            selector_fixture=expected.__name__,
            native_parent_selector_calls=0,
            requested=requested,
            reference_pipe_capacities=baseline,
            native_pipe_capacities=sizes,
            distinct_pipe_inodes=3,
            set_size_before_spawn=len(sets),
            wrote_pear=requested > 0,
            all_native_streams_closed=True,
            original_child="pinned CPython running only the original stdio payload",
            kill_observation="SIGKILL delivered"
            if sent
            else "already exited and reaped by native poll",
            successful_reaps=1,
            native_calls=h.calls,
        ),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    man = verify()
    from test import test_subprocess as upstream

    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        rows, errors, skips = run_in_fixtures(
            load_suite(man["module"]),
            dict.fromkeys(IDS, lambda c, o: measure(tc, Path(tmp), c, o, upstream)),
        )
        if errors or skips or rows.keys() != IDS:
            raise AssertionError((errors, skips, IDS - rows.keys()))
    args.result.write_text(
        json.dumps(
            dict(
                cpython_commit=man["commit"],
                opt_level=tc.level,
                toolchain={
                    n: sha(getattr(args, n)) for n in ("imre", "runtime", "clang")
                },
                rows=rows,
            ),
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
