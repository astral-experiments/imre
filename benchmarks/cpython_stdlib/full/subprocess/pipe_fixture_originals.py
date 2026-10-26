"""Run the original EINTR and unbuffered pipe cases against their real kernel endpoints."""

import argparse
import array
import ast
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import subprocess
import sys
import tempfile
import termios
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from attempt_bodies import SOURCE, fixture
from selector_fixture import verify_binding
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

HERE = Path(__file__).resolve().parent
EINTR = {
    f"test.test_subprocess.{cls}.test_communicate_eintr"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
}
RAW = "test.test_subprocess.POSIXProcessTestCase.test_select_unbuffered"
IDS = EINTR | {RAW}
HEAD = "import fixture_pipes\nimport subprocess\nimport sys\n"
SINGLE = """with subprocess.Popen(sys.argv[2:], STREAM=subprocess.PIPE) as p:
    print(p.pid)
    if input()!="communicate":
        raise AssertionError("expected communicate")
    out,err=p.communicate()
    print(repr(out))
    print(repr(err))
    if input()!="exit":
        raise AssertionError("expected context exit")
print("closed")
"""


def eintr_source(select_only):
    from textwrap import indent

    selector = (
        "subprocess._PopenSelector = selectors.SelectSelector\n" if select_only else ""
    )
    text = HEAD + ("import selectors\n" if select_only else "")
    for stream in ("stdout", "stderr"):
        text += f"def drive_{stream}() -> None:\n" + indent(
            SINGLE.replace("STREAM", stream), "    "
        )
    return (
        text
        + selector
        + 'fixture_pipes.arm(int(sys.argv[1]))\nmode=input()\nwhile mode!="quit":\n    if mode=="stdout":\n        drive_stdout()\n    else:\n        drive_stderr()\n    mode=input()\nprint(fixture_pipes.disarm())\n'
    ).encode()


def raw_source(bufsize):
    return (
        HEAD
        + f"p=subprocess.Popen(sys.argv[2:],stdout=subprocess.PIPE,bufsize={bufsize})\n"
        + """f=p.stdout
print(p.pid)
print(f.fileno())
fixture_pipes.share_fd(int(sys.argv[1]), f.fileno())
op=input()
while op!="quit":
    if op=="read":
        print(repr(f.read(int(input()))))
    elif op=="wait":
        print(p.wait())
    elif op=="close":
        f.close()
        print(f.closed)
    op=input()
if not f.closed:
    f.close()
p.wait()
"""
    ).encode()


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def site(tc, root):
    s = root / "site"
    s.mkdir()
    (s / "Python.h").write_bytes(subprocess.check_output([tc.imre, "extension-header"]))
    (s / "fixture_pipes.pyi").write_text(
        "def arm(channel: int, /) -> None: ...\ndef disarm() -> int: ...\ndef share_fd(channel: int, fd: int, /) -> None: ...\n"
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
            "-I",
            s,
            "-c",
            HERE / "fixture_pipes.c",
            "-o",
            root / "fixture.o",
        ],
        check=True,
    )
    subprocess.run(
        ["ar", "rcs", s / "fixture_pipes.imre.a", root / "fixture.o"], check=True
    )
    return s


def build(tc, root, site, source):
    top = root / "actor"
    top.mkdir()
    src = top / "case.py"
    src.write_bytes(source)
    binary = top / "case"
    result = subprocess.run(
        [
            tc.imre,
            "build",
            src,
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
        capture_output=True,
        text=True,
    )
    if result.returncode:
        raise RuntimeError(result.stderr)
    return binary


class Actor:
    def __init__(self, binary, root, args):
        self.root = root
        self.binary = binary
        self.channel, other = socket.socketpair()
        self.channel.settimeout(15)
        try:
            self.sp = subprocess.Popen(
                [
                    "strace",
                    "-ff",
                    "-qq",
                    "-s",
                    "768",
                    "-e",
                    "signal=SIGUSR1",
                    "-e",
                    "trace=execve,clone,clone3,fork,vfork,wait4,waitid,kill,rt_sigaction,rt_sigreturn,poll,ppoll,select,pselect6,read",
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

    def send(self, *values):
        self.sp.stdin.write(b"".join(str(x).encode() + b"\n" for x in values))

    def read(self):
        if not select.select([self.sp.stdout], [], [], 15)[0]:
            raise TimeoutError("native pipe parent timed out")
        s = self.sp.stdout.readline()
        if not s:
            raise AssertionError(self.sp.stderr.read())
        return s.rstrip(b"\n")

    def parent(self):
        [pid] = map(
            int,
            Path(f"/proc/{self.sp.pid}/task/{self.sp.pid}/children")
            .read_text()
            .split(),
        )
        assert Path(f"/proc/{pid}/exe").samefile(self.binary)
        return pid

    def done(self):
        self.sp.stdin.close()
        rc = self.sp.wait(timeout=15)
        err = self.sp.stderr.read()
        self.sp.stdout.close()
        self.sp.stderr.close()
        self.channel.close()
        if rc or err:
            raise AssertionError((rc, err))

    def trace(self, pid):
        return (self.root / f"pid.{pid}").read_text().splitlines()

    def verify_child(self, owner, pid, program):
        lines = self.trace(owner)
        child = self.trace(pid)
        assert any(
            re.match(r"(clone3?|vfork|fork)\(", s) and re.search(rf"= {pid}$", s)
            for s in lines
        )
        assert any(
            s.startswith("execve(") and program in s and s.endswith("= 0")
            for s in child
        )
        assert any(
            s.startswith(f"wait4({pid},")
            and re.search(rf"WEXITSTATUS\(s\) == 0.*= {pid}$", s)
            for s in lines
        )
        return lines, child


class Context:
    def __init__(self, a, pid, stream):
        self.a = a
        self.pid = pid
        self.stream = stream

    def __enter__(self):
        return self

    def communicate(self):
        self.a.send("communicate")
        assert self.a.channel.recv(1) == bytes([signal.SIGUSR1])
        signal.raise_signal(signal.SIGUSR1)
        result = tuple(ast.literal_eval(self.a.read().decode()) for _ in range(2))
        assert result == ((b"", None) if self.stream == "stdout" else (None, b""))
        return result

    def __exit__(self, *exc):
        self.a.send("exit")
        assert self.a.read() == b"closed"


def eintr(tc, site, root, case, orig, upstream):
    no_poll = type(case).__name__.endswith("NoPoll")
    verify_binding(upstream.subprocess, no_poll)
    binary = build(tc, root, site, eintr_source(no_poll))
    actor = None
    children = []
    host_callbacks = []
    previous_profile = sys.getprofile()

    def observe(frame, event, arg):
        if (
            event == "call"
            and frame.f_code.co_name == "handler"
            and frame.f_code.co_filename == upstream.__file__
        ):
            host_callbacks.append(frame.f_locals["signum"])
        if previous_profile is not None:
            previous_profile(frame, event, arg)

    def launch(args, **kw):
        nonlocal actor
        assert args == [
            sys.executable,
            "-c",
            "import os, signal;os.kill(os.getppid(), signal.SIGUSR1)",
        ]
        [stream] = kw
        assert stream in ("stdout", "stderr") and kw[stream] == subprocess.PIPE
        handler = signal.getsignal(signal.SIGUSR1)
        assert (
            callable(handler)
            and handler.__name__ == "handler"
            and handler.__module__ == upstream.__name__
        )
        if actor is None:
            actor = Actor(binary, root, args)
        actor.send(stream)
        pid = int(actor.read())
        children.append((stream, pid))
        return Context(actor, pid, stream)

    try:
        sys.setprofile(observe)
        with patch.object(
            upstream, "subprocess", SimpleNamespace(Popen=launch, PIPE=subprocess.PIPE)
        ):
            orig()
    finally:
        sys.setprofile(previous_profile)
        if actor:
            owner = actor.parent()
            actor.send("quit")
            deliveries = int(actor.read())
            actor.done()
    assert deliveries == 2 and [s for s, p in children] == ["stdout", "stderr"]
    assert host_callbacks == [signal.SIGUSR1, signal.SIGUSR1]
    calls = []
    for stream, pid in children:
        lines, child = actor.verify_child(
            owner, pid, "os.kill(os.getppid(), signal.SIGUSR1)"
        )
        assert any(
            s.startswith(f"kill({owner}, SIGUSR1)") and s.endswith("= 0") for s in child
        )
        [i] = [
            i
            for i, s in enumerate(lines)
            if s.startswith("--- SIGUSR1") and f"si_pid={pid}," in s
        ]
        # One pipe and no timeout uses communicate's direct read path in both
        # fixture variants. A selector call here would be the wrong operation.
        interrupted = lines[i - 1]
        found = re.match(r"read\((\d+), .*ERESTART", interrupted)
        if not found or int(found[1]) < 3:
            raise AssertionError(
                (
                    stream,
                    "signal did not interrupt the native pipe read",
                    lines[max(0, i - 4) : i + 5],
                )
            )
        assert lines[i + 1].startswith("rt_sigreturn(") and "EINTR" in lines[i + 1]
        restored = next(s for s in lines[i + 2 :] if s.startswith(f"read({found[1]},"))
        assert re.search(r', "", [0-9]+\)\s+= 0$', restored), restored
        assert not any(
            s.startswith(("poll(", "ppoll(", "select(", "pselect6(")) for s in lines
        )
        calls.append(
            dict(
                stream=stream,
                interrupted="read",
                signal_return="EINTR",
                restarted="read on the same pipe fd",
                restarted_result="EOF",
                native_selector_syscalls=0,
                sender="original data child",
                successful_reaps=1,
            )
        )
    return dict(
        actor_sha256=sha(binary),
        selector="SelectSelector" if no_poll else "PollSelector",
        native_deliveries=deliveries,
        original_handler_calls=len(host_callbacks),
        launches=calls,
    )


def queued(fd):
    n = array.array("i", [0])
    fcntl.ioctl(fd, termios.FIONREAD, n, True)
    return n[0]


class RawStream:
    def __init__(self, actor):
        self.a = actor
        self.pid = int(actor.read())
        self.remote = int(actor.read())
        self.owner = actor.parent()
        size = array.array("i").itemsize
        data, anc, flags, _ = actor.channel.recvmsg(1, socket.CMSG_SPACE(size))
        assert data == b"F" and not flags and len(anc) == 1
        lev, typ, raw = anc[0]
        assert (lev, typ) == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
        fds = array.array("i")
        fds.frombytes(raw)
        [self.fd] = fds
        assert (
            os.fstat(self.fd).st_ino
            == os.stat(f"/proc/{self.owner}/fd/{self.remote}").st_ino
        )
        self.remaining = None
        self.reads = []

    def read(self, size):
        self.a.send("read", size)
        result = ast.literal_eval(self.a.read().decode())
        self.remaining = queued(self.fd)
        self.reads.append((size, result))
        return result

    def fileno(self):
        return self.fd

    def close(self):
        if self.fd is None:
            return
        self.a.send("close")
        assert self.a.read() == b"True"
        os.close(self.fd)
        self.fd = None


class RawParent:
    def __init__(self, actor):
        self.stdout = RawStream(actor)
        self.a = actor
        self.waits = []

    def wait(self):
        self.a.send("wait")
        rc = int(self.a.read())
        self.waits.append(rc)
        return rc


def unbuffered(tc, site, root, case, orig, upstream):
    binary = build(tc, root, site, raw_source(0))
    host = []
    observed = {}
    readiness = []
    real_select = select.select

    def watch(readers, writers, exceptional, timeout=None):
        result = real_select(readers, writers, exceptional, timeout)
        if host and readers == [host[0].stdout]:
            stream = host[0].stdout
            assert (writers, exceptional, timeout) == ([], [], 0.0)
            assert len(result[0]) == 1 and result[0][0] is stream
            readiness.append(queued(stream.fd))
        return result

    def launch(args, **kw):
        assert (
            not host
            and args == [sys.executable, "-c", 'import sys;sys.stdout.write("apple")']
            and kw == dict(stdout=subprocess.PIPE, bufsize=0)
        )
        p = RawParent(Actor(binary, root, args))
        host.append(p)

        def finish_after_original_close():
            f = p.stdout
            closed_by_original = f.fd is None
            if not closed_by_original:
                f.close()
            p.a.send("quit")
            p.a.done()
            lines, _ = p.a.verify_child(f.owner, f.pid, "sys.stdout.write")
            assert (
                p.waits == [0]
                and f.reads == [(4, b"appl")]
                and f.remaining == 1
                and readiness == [1]
                and closed_by_original
            )
            assert any(
                re.match(rf'read\({f.remote}, "appl", 4\)\s+= 4$', s) for s in lines
            )
            observed.update(
                actor_sha256=sha(binary),
                read_bytes=4,
                remaining_kernel_bytes=f.remaining,
                same_open_file_description="SCM_RIGHTS",
                host_select_returned_original_stream=True,
                native_stream_closed=True,
                native_reaps=1,
            )

        # Register first, so the original later-registered f.close runs first.
        case.addCleanup(finish_after_original_close)
        return p

    with (
        patch.object(
            upstream, "subprocess", SimpleNamespace(Popen=launch, PIPE=subprocess.PIPE)
        ),
        patch.object(select, "select", watch),
    ):
        orig()
    return observed


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--opt-level", type=int, required=True, choices=(0, 3))
    p.add_argument(
        "--test-id",
        action="append",
        choices=sorted(IDS),
        help="run only the named original occurrence (repeatable)",
    )
    a = p.parse_args()
    selected = set(a.test_id) if a.test_id else IDS
    man = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", man["files"])
    verify_interpreter_tests(man)
    from test import test_subprocess as upstream

    tc = Toolchain(a.imre, a.runtime, a.clang, a.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(tmp)
        s = site(tc, root)

        def measure(case, orig):
            d = root / case.id()
            d.mkdir()
            try:
                owner, prep = fixture(case, d)
                native = native_method(tc, d, SOURCE, owner, case.id(), prep)
            except (ValueError, RuntimeError) as exc:
                body = dict(
                    status="blocked",
                    reason=str(exc).replace(str(d), "<case>").strip().splitlines()[-1],
                )
            else:
                body = dict(status="completed", **native(case, orig))
            op = (eintr if case.id() in EINTR else unbuffered)(
                tc, s, d, case, orig, upstream
            )
            return dict(
                body_attempt=body,
                roles=dict(
                    body="cpython",
                    assertions="cpython",
                    operation="imre" if case.id() in EINTR else "mixed",
                    fixture="mixed",
                ),
                observed=op,
            )

        rows, fail, skips = run_in_fixtures(
            load_suite(man["module"]), dict.fromkeys(selected, measure)
        )
        if fail or skips or rows.keys() != selected:
            raise AssertionError((fail, skips))
        data = dict(
            cpython_commit=man["commit"],
            opt_level=tc.level,
            fixture_sha256=sha(HERE / "fixture_pipes.c"),
            toolchain={k: sha(getattr(a, k)) for k in ("imre", "runtime", "clang")},
            rows=rows,
        )
    a.result.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    main()
