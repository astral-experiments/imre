"""Observe original Popen finalizers, warning filters and fixture teardown in the real parent."""

import argparse
import errno
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
from unittest.mock import patch
import warnings

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

PREFIX = "test.test_subprocess.POSIXProcessTestCase."
IDS = {
    PREFIX + "test_zombie_fast_process_del",
    PREFIX + "test_leak_fast_process_del_killed",
}


def hash_file(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def build(tc, root):
    site = root / "site"
    site.mkdir()
    (site / "Python.h").write_bytes(
        subprocess.check_output([tc.imre, "extension-header"])
    )
    (site / "fixture_active_os.pyi").write_text(
        "def send_signal(pid: int, sig: int, /) -> int: ...\n"
        "def wait_status(pid: int, flags: int, /) -> int: ...\n"
    )
    obj = root / "active.o"
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
            HERE / "fixture_active_os.c",
            "-o",
            obj,
        ],
        check=True,
    )
    subprocess.run(["ar", "rcs", site / "fixture_active_os.imre.a", obj], check=True)
    # Isolate import resolution from the rest of the suite's host-only adapters.
    entry, binary = root / "parent.py", root / "parent"
    entry.write_bytes((HERE / "active_parent.py").read_bytes())
    subprocess.run(
        [
            tc.imre,
            "build",
            entry,
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
    )
    return binary


class Pipe:
    def __init__(self, parent, name):
        self.parent, self.name = parent, name

    def close(self):
        assert self.parent.request("close-" + self.name) == "closed"
        self.parent.closes.append(self.name)


class Owner:
    def __init__(self, parent):
        self.parent, self.pid = parent, parent.pid
        self.stdout, self.stderr = Pipe(parent, "out"), Pipe(parent, "err")

    def identity(self):
        ident = int(self.parent.request("identity"))
        self.parent.identities.append(ident)
        return ident

    def __del__(self):
        try:
            self.parent.drop(self)
        except Exception as error:
            # Python suppresses destructor exceptions; surface them in cleanup.
            self.parent.failure = error


class Active:
    def __init__(self, parent, identity, pid):
        self.parent, self.native_id, self.pid = parent, identity, pid

    def identity(self):
        return self.native_id  # compiled id() was evaluated while reading _active

    def wait(self):
        status = int(self.parent.request("wait-active", self.native_id))
        self.parent.teardown_waits.append(status)
        return status


class Parent:
    def __init__(self, binary, directory, argv, *, reference_entry=None):
        self.binary, self.directory, self.argv = binary, directory, argv
        self.reference_entry = reference_entry
        self.identities, self.memberships, self.closes = [], [], []
        self.signals, self.raw_waits, self.teardown_waits, self.missing = [], [], [], []
        self.warnings, self.cleanups = [], 0
        self.failure = None
        command = [str(binary)]
        if reference_entry is not None:
            command.extend(["-E", "-u", str(reference_entry)])
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
                "trace=execve,clone,clone3,vfork,fork,wait4,waitid,kill,close",
                "-o",
                directory / "pid",
                "--",
                *command,
                *argv,
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            start_new_session=True,
        )
        self.pid = int(self.read())
        self.fds = [int(self.read()), int(self.read())]
        [self.parent] = map(
            int,
            Path(f"/proc/{self.actor.pid}/task/{self.actor.pid}/children")
            .read_text()
            .split(),
        )
        assert self.parent != self.pid and Path(f"/proc/{self.parent}/exe").samefile(
            binary
        )

    def read(self, stderr=False):
        pipe = self.actor.stderr if stderr else self.actor.stdout
        if not select.select([pipe], [], [], 10)[0]:
            raise TimeoutError("active-child fixture did not respond")
        line = pipe.readline()
        if not line:
            raise RuntimeError("active-child fixture closed its protocol")
        return line.decode().rstrip("\n")

    def request(self, *words):
        self.actor.stdin.write(b"".join(str(word).encode() + b"\n" for word in words))
        return self.read()

    def drop(self, source):
        # The real warnings_helper.check_warnings context installs this filter.
        action, message, category, module, lineno = warnings.filters[0]
        assert (message, category, module, lineno) == (None, Warning, None, 0)
        assert action == "always"
        assert self.request("drop", action) == "dropped"
        diagnostic = self.read(stderr=True)
        found = re.search(
            r"ResourceWarning: (subprocess (\d+) is still running)$", diagnostic
        )
        assert found and int(found[2]) == self.pid, diagnostic
        self.warnings.append(action)
        # Host warning capture receives the category and text emitted by Imre.
        warnings.warn(found[1], ResourceWarning, source=source)

    def active(self):
        size = int(self.request("active"))
        entries = [
            Active(self, int(self.read()), int(self.read())) for _ in range(size)
        ]
        self.memberships.append([(entry.native_id, entry.pid) for entry in entries])
        return entries

    def signal(self, pid, sig):
        assert (pid, sig) == (self.pid, signal.SIGKILL)
        result = int(self.request("kill", int(sig)))
        assert result == 0
        self.signals.append(int(sig))

    def raw_wait(self, pid, flags):
        assert (pid, flags) == (self.pid, 0)
        result = int(self.request("waitpid", flags))
        self.raw_waits.append(result)
        if result < 0:
            raise OSError(-result, os.strerror(-result))
        return result >> 16, result & 65535

    def failed_launch(self, argv):
        [missing] = argv
        number = int(self.request("missing", missing))
        filename = self.read()
        assert number == errno.ENOENT and filename == missing
        self.missing.append(filename)
        raise OSError(number, os.strerror(number), filename)

    def cleanup(self):
        assert self.request("cleanup") == "cleaned"
        self.cleanups += 1

    def close(self):
        try:
            if self.failure:
                raise self.failure
            self.actor.stdin.write(b"quit\n")
            self.actor.stdin.close()
            assert self.actor.wait(timeout=10) == 0
            tail = self.actor.stderr.read()
            if self.reference_entry is None:
                assert tail == b"", tail
        finally:
            if self.actor.poll() is None:
                os.killpg(self.actor.pid, signal.SIGKILL)
                self.actor.wait()
            for stream in (self.actor.stdin, self.actor.stdout, self.actor.stderr):
                stream.close()

    def proof(self, killed):
        [identity] = self.identities
        retained = [(identity, self.pid)]
        assert self.memberships == (
            [retained, [], [], []] if killed else [retained, retained, []]
        )
        assert self.warnings == ["always"] and self.cleanups == 1
        assert self.closes == ["err", "out"]
        assert self.signals == ([signal.SIGKILL] if killed else [])
        assert self.raw_waits == ([-errno.ECHILD] if killed else [])
        assert self.teardown_waits == ([] if killed else [0])
        assert bool(self.missing) == killed
        lines = (self.directory / f"pid.{self.parent}").read_text().splitlines()
        child = (self.directory / f"pid.{self.pid}").read_text().splitlines()
        assert any(
            re.match(r"(clone3?|vfork|fork)\(", s) and s.endswith(f"= {self.pid}")
            for s in lines
        )
        assert any(
            s.startswith("execve(" + json.dumps(self.argv[0]) + ",")
            and json.dumps(self.argv[-1]) in s
            and s.endswith("= 0")
            for s in child
        )
        reaps = [
            i
            for i, s in enumerate(lines)
            if s.startswith(f"wait4({self.pid},") and s.endswith(f"= {self.pid}")
        ]
        assert len(reaps) == 1
        if killed:
            [sent] = [
                i
                for i, s in enumerate(lines)
                if re.fullmatch(rf"kill\({self.pid}, SIGKILL\)\s+= 0", s)
            ]
            [nochild] = [
                i
                for i, s in enumerate(lines)
                if s.startswith(f"wait4({self.pid},") and "ECHILD" in s and ", 0," in s
            ]
            [failed_pid] = [
                int(other.suffix[1:])
                for other in self.directory.glob("pid.*")
                if other.name not in {f"pid.{self.parent}", f"pid.{self.pid}"}
                and any(
                    line.startswith("execve(")
                    and self.missing[0] in line
                    and "ENOENT" in line
                    for line in other.read_text().splitlines()
                )
            ]
            [failed_spawn] = [
                i
                for i, line in enumerate(lines)
                if re.match(r"(clone3?|vfork|fork)\(", line)
                and line.endswith(f"= {failed_pid}")
            ]
            assert sent < reaps[0] < failed_spawn < nochild
            assert "SIGKILL" in lines[reaps[0]]
        else:
            assert "WEXITSTATUS(s) == 0" in lines[reaps[0]]
        assert len(set(self.fds)) == 2 and all(
            any(re.fullmatch(rf"close\({fd}\)\s+= 0", s) for s in lines)
            for fd in self.fds
        )
        return dict(
            parent_executable_verified=True,
            data_child_argv=["<pinned-cpython>", *self.argv[1:]],
            parent_warning_pid_matched=True,
            applied_host_filter="always",
            original_object_identity_retained=True,
            membership_lengths=list(map(len, self.memberships)),
            successful_kernel_reaps=1,
            kernel_status=-int(signal.SIGKILL) if killed else 0,
            fixture_waitpid_errno=errno.ECHILD if killed else None,
            failed_launch_errno=errno.ENOENT if killed else None,
            fixture_closed_parent_pipe_ends=len(self.closes),
            fixture_cleanup_calls=self.cleanups,
        )


def run_actors(tc, binary, work, reference_entry=None):
    from test import test_subprocess as upstream

    def execute(case, original):
        directory = work / case.id()
        directory.mkdir()
        attempt = None
        if tc is not None:
            body = directory / "body"
            body.mkdir()
            try:
                owner, preamble = fixture(case, body)
                full = native_method(tc, body, SOURCE, owner, case.id(), preamble)
            except (RuntimeError, ValueError) as exc:
                attempt = dict(
                    status="blocked",
                    reason=str(exc)
                    .replace(str(body), "<case>")
                    .strip()
                    .splitlines()[-1],
                )
            else:
                attempt = dict(status="completed", **full(case, original))
        parents, result = [], {}

        def parent():
            [actor] = parents
            return actor

        class PopenAPI:
            PIPE = subprocess.PIPE

            def Popen(self, argv, **options):
                assert options == dict(stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                if parents:
                    assert argv == upstream.NONEXISTING_CMD
                    return parent().failed_launch(argv)
                assert type(argv) is list and argv[:2] == [sys.executable, "-c"]
                parents.append(
                    Parent(binary, directory, argv, reference_entry=reference_entry)
                )
                return Owner(parent())

            @property
            def _active(self):
                return parent().active()

            def _cleanup(self):
                parent().cleanup()

        class OSFixture:
            def kill(self, pid, sig):
                parent().signal(pid, sig)

            def waitpid(self, pid, options):
                return parent().raw_wait(pid, options)

            def __getattr__(self, name):
                return getattr(os, name)

        def identity(owner):
            assert type(owner) in (Owner, Active)
            return owner.identity()

        # Keep these bindings through the original BaseTestCase.tearDown, which
        # waits on each active entry, calls _cleanup and runs the pipe cleanups.
        case.enterContext(patch.object(upstream, "subprocess", PopenAPI()))
        case.enterContext(patch.object(upstream, "os", OSFixture()))
        case.enterContext(patch.object(upstream, "id", identity, create=True))

        def finish():
            actor = parent()
            actor.close()
            result["observed"] = actor.proof(case._testMethodName.endswith("killed"))

        case.addCleanup(finish)
        original()
        result.update(body_attempt=attempt)
        if tc is None:
            result["execution"] = "reference-only"
        else:
            result["roles"] = dict(
                body="cpython", assertions="cpython", fixture="mixed", operation="imre"
            )
        return result

    rows, errors, skips = run_in_fixtures(
        load_suite("test.test_subprocess"), dict.fromkeys(IDS, execute)
    )
    if rows.keys() != IDS or errors or skips:
        raise AssertionError((errors, skips, rows.keys()))
    return rows


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    a = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(a.imre, a.runtime, a.clang, a.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(tmp)
        binary = build(tc, root)
        rows = run_actors(tc, binary, root)
        result = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            actor_sha256=hash_file(binary),
            parent_source_sha256=hash_file(HERE / "active_parent.py"),
            fixture_os_sha256=hash_file(HERE / "fixture_active_os.c"),
            toolchain={
                key: hash_file(getattr(a, key)) for key in ("imre", "runtime", "clang")
            },
            rows=rows,
        )
    a.result.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
