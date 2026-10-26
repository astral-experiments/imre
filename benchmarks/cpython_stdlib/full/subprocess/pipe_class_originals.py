"""Run the four unchanged Popen stream-class tests using native isinstance observations."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import select
import selectors
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

METHODS = ("test_io_buffered_by_default", "test_io_unbuffered_works")
IDS = {
    f"test.test_subprocess.{cls}.{m}"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
    for m in METHODS
}


def source(unbuffered):
    code = """import io
import subprocess
import sys
from typing import BinaryIO

def membership(f: BinaryIO) -> int:
    bits=0
    if isinstance(f,io.RawIOBase):
        bits+=1
    if isinstance(f,io.BufferedIOBase):
        bits+=2
    if isinstance(f,io.TextIOBase):
        bits+=4
    if isinstance(f,io.IOBase):
        bits+=8
    return bits
"""
    opt = ",bufsize=0" if unbuffered else ""
    code += f"p=subprocess.Popen(sys.argv[1:],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE{opt})\nprint(p.pid)\n"
    for field in ("stdin", "stdout", "stderr"):
        code += f"print(p.{field}.fileno())\n"
    code += 'op=input()\nwhile op!="quit":\n'
    for field in ("stdin", "stdout", "stderr"):
        code += f'    if op=="{field}.class":\n        print(membership(p.{field}))\n'
        code += f'    if op=="{field}.close":\n        p.{field}.close()\n        print(membership(p.{field}))\n'
    return (
        code + '    if op=="wait":\n        print(p.wait())\n    op=input()\np.wait()\n'
    ).encode()


class Stream:
    def __init__(self, owner, name):
        self.owner, self.name = owner, name

    @property
    def __class__(self):
        # The host assertion accepts only what the compiled predicate reported.
        # The requested bufsize is not consulted on this path.
        mask = self.owner.request(self.name + ".class")
        return {9: io.RawIOBase, 10: io.BufferedIOBase}[mask]

    def close(self):
        self.owner.request(self.name + ".close")


class Parent:
    def __init__(self, bin, root, args):
        self.bin = bin
        self.root = root
        self.events = []
        self.args = args
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
                "trace=execve,clone,clone3,fork,vfork,close,wait4,waitid,poll,ppoll,select,pselect6",
                "-o",
                root / "pid",
                "--",
                bin,
                *args,
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
        )
        self.pid = self.read()
        self.fds = [self.read() for _ in range(3)]
        [self.parent] = map(
            int,
            Path(f"/proc/{self.actor.pid}/task/{self.actor.pid}/children")
            .read_text()
            .split(),
        )
        assert (
            Path(f"/proc/{self.parent}/exe").samefile(bin) and self.parent != self.pid
        )
        for field in ("stdin", "stdout", "stderr"):
            setattr(self, field, Stream(self, field))

    def read(self):
        if not select.select([self.actor.stdout], [], [], 12)[0]:
            raise TimeoutError("native stream class call")
        line = self.actor.stdout.readline()
        if not line:
            raise AssertionError(self.actor.stderr.read())
        return int(line.rstrip(b"\n"))

    def request(self, query):
        self.actor.stdin.write(query.encode() + b"\n")
        value = self.read()
        self.events.append((query, value))
        return value

    def wait(self):
        return self.request("wait")

    def finish(self):
        self.actor.stdin.write(b"quit\n")
        self.actor.stdin.close()
        rc = self.actor.wait(timeout=10)
        err = self.actor.stderr.read()
        self.actor.stdout.close()
        self.actor.stderr.close()
        if rc or err:
            raise AssertionError((rc, err))

    def observations(self, unbuffered):
        parent = (self.root / f"pid.{self.parent}").read_text().splitlines()
        child = (self.root / f"pid.{self.pid}").read_text().splitlines()
        assert any(
            re.match(r"(clone3?|vfork|fork)\(", s) and re.search(rf"= {self.pid}$", s)
            for s in parent
        )
        assert any(
            s.startswith("execve(" + json.dumps(self.args[0]) + ",")
            and json.dumps(self.args[-1]) in s
            and s.endswith("= 0")
            for s in child
        )
        assert (
            sum(
                s.startswith(f"wait4({self.pid},")
                and "WEXITSTATUS(s) == 0" in s
                and s.endswith(f"= {self.pid}")
                for s in parent
            )
            == 1
        )
        assert not any(
            s.startswith(("poll(", "ppoll(", "select(", "pselect6(")) for s in parent
        )
        mask = 9 if unbuffered else 10
        expected = (
            [(f"{stream}.class", mask) for stream in ("stdin", "stdout", "stderr")]
            + [(f"{stream}.close", mask) for stream in ("stdin", "stdout", "stderr")]
            + [("wait", 0)]
        )
        assert self.events == expected, self.events
        assert len(set(self.fds)) == 3 and all(
            any(re.match(rf"close\({fd}\)\s+= 0$", s) for s in parent)
            for fd in self.fds
        )
        return dict(
            native_class="RawIOBase" if mask == 9 else "BufferedIOBase",
            native_io_base=True,
            class_unchanged_after_close=True,
            streams=["stdin", "stdout", "stderr"],
            closed_native_descriptors=3,
            successful_native_reaps=1,
            native_selector_syscalls=0,
            data_child_argv=list(self.args),
        )


def sha(p):
    with p.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for k in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + k, type=Path, required=True)
    p.add_argument("--opt-level", type=int, required=True, choices=(0, 3))
    a = p.parse_args()
    man = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", man["files"])
    verify_interpreter_tests(man)
    from test import test_subprocess as upstream

    tc = Toolchain(a.imre, a.runtime, a.clang, a.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):

        def measure(case, original):
            d = Path(tmp, case.id())
            d.mkdir()
            bodydir = d / "body"
            bodydir.mkdir()
            try:
                owner, pre = fixture(case, bodydir)
                full = native_method(tc, bodydir, SOURCE, owner, case.id(), pre)
            except (ValueError, RuntimeError) as exc:
                body = dict(
                    status="blocked",
                    reason=str(exc)
                    .replace(str(bodydir), "<case>")
                    .strip()
                    .splitlines()[-1],
                )
            else:
                body = dict(status="completed", **full(case, original))
            unbuffered = case._testMethodName == "test_io_unbuffered_works"
            expected = (
                selectors.SelectSelector
                if type(case).__name__.endswith("NoPoll")
                else selectors.PollSelector
            )
            assert upstream.subprocess._PopenSelector is expected
            binary = tc.build(source(unbuffered), d)
            instances = []

            def launch(args, **kw):
                options = dict.fromkeys(("stdin", "stdout", "stderr"), subprocess.PIPE)
                if unbuffered:
                    options["bufsize"] = 0
                assert (
                    not instances and args == upstream.ZERO_RETURN_CMD and kw == options
                )
                real = Parent(binary, d, args)
                instances.append(real)
                return real

            try:
                with patch.object(
                    upstream,
                    "subprocess",
                    SimpleNamespace(Popen=launch, PIPE=subprocess.PIPE),
                ):
                    original()
            finally:
                for real in instances:
                    real.finish()
            [real] = instances
            return dict(
                body_attempt=body,
                roles=dict(
                    body="cpython",
                    assertions="cpython",
                    operation="imre",
                    fixture="cpython",
                ),
                actor_sha256=sha(binary),
                fixture_selector=expected.__name__,
                observed=real.observations(unbuffered),
            )

        rows, fail, skips = run_in_fixtures(
            load_suite(man["module"]), dict.fromkeys(IDS, measure)
        )
        if fail or skips or rows.keys() != IDS:
            raise AssertionError((fail, skips))
    a.result.write_text(
        json.dumps(
            dict(
                cpython_commit=man["commit"],
                opt_level=tc.level,
                toolchain={k: sha(getattr(a, k)) for k in ("imre", "runtime", "clang")},
                rows=rows,
            ),
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
