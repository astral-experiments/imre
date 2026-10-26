"""Observe a real ptrace stop in a compiled Popen parent without tracing the child from strace."""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

CHILD = """#include <signal.h>
#include <sys/ptrace.h>
int main(void) {
    if (ptrace(PTRACE_TRACEME, 0, 0, 0) == -1) return 111;
    raise(SIGQUIT);
    return 7;
}
"""
FIXTURE = """#include <Python.h>
#include <errno.h>
#include <sys/ptrace.h>
#include <sys/wait.h>
static PyObject *resume(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid;
    if (!PyArg_ParseTuple(args, "k", &pid)) return NULL;
    if (ptrace(PTRACE_CONT, (pid_t)pid, 0, 0) == -1) {
        PyErr_SetString(PyExc_RuntimeError, "could not continue traced child");
        return NULL;
    }
    int status;
    pid_t ended;
    do { ended = waitpid((pid_t)pid, &status, 0); } while (ended < 0 && errno == EINTR);
    if (ended != (pid_t)pid || !WIFEXITED(status)) {
        PyErr_SetString(PyExc_RuntimeError, "could not reap continued child");
        return NULL;
    }
    return PyLong_FromLong(WEXITSTATUS(status));
}
static PyMethodDef methods[]={{"resume", resume, METH_VARARGS, NULL},{NULL,NULL,0,NULL}};
static struct PyModuleDef module={PyModuleDef_HEAD_INIT, "fixture_trap", NULL, -1, methods,NULL,NULL,NULL,NULL};
PyMODINIT_FUNC PyInit_fixture_trap(void) { return PyModule_Create(&module); }
"""
ACTOR = """import fixture_trap
import subprocess
import sys
import time
p = subprocess.Popen([sys.argv[1]])
print(p.pid)
if sys.argv[2] == "poll":
    while p.poll() is None:
        time.sleep(0.001)
else:
    p.wait()
code = p.returncode
if code is None:
    print(999)
else:
    print(code)
print(p.wait())
print(fixture_trap.resume(p.pid))
"""
HOST_FIXTURE = """import ctypes, os
def resume(pid):
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.ptrace(7,pid,0,0) == -1:
        raise OSError(ctypes.get_errno(), "PTRACE_CONT")
    waited,status=os.waitpid(pid,0)
    assert waited==pid and os.WIFEXITED(status)
    return os.WEXITSTATUS(status)
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--opt-level", required=True, type=int, choices=(0, 3))
    p.add_argument(
        "--observe-only",
        action="store_true",
        help="write an actual pre-fix result even when it differs from CPython",
    )
    a = p.parse_args()
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        site = root / "site"
        site.mkdir()
        native = root / "parent"
        child = root / "child"
        (root / "child.c").write_text(CHILD)
        (root / "fixture.c").write_text(FIXTURE)
        (site / "Python.h").write_bytes(
            subprocess.check_output([a.imre, "extension-header"])
        )
        (site / "fixture_trap.pyi").write_text("def resume(pid: int, /) -> int: ...\n")
        (root / "parent.py").write_text(ACTOR)
        subprocess.run(
            [a.clang, "-Wall", "-Wextra", "-Werror", root / "child.c", "-o", child],
            check=True,
        )
        subprocess.run(
            [
                a.clang,
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fPIC",
                "-I",
                site,
                "-c",
                root / "fixture.c",
                "-o",
                root / "fixture.o",
            ],
            check=True,
        )
        subprocess.run(
            ["ar", "rcs", site / "fixture_trap.imre.a", root / "fixture.o"], check=True
        )
        subprocess.run(
            [
                a.imre,
                "build",
                root / "parent.py",
                "--extension-path",
                site,
                "--runtime",
                a.runtime,
                "--clang",
                a.clang,
                "--no-cache",
                "--opt-level",
                str(a.opt_level),
                "-o",
                native,
            ],
            check=True,
        )
        # The CPython reference owns its own real child and only uses this module to resume it.
        (root / "fixture_trap.py").write_text(HOST_FIXTURE)
        results = {}
        for mode in ("wait", "poll"):
            ref = subprocess.run(
                [sys.executable, "-E", root / "parent.py", child, mode],
                capture_output=True,
                check=False,
                timeout=10,
            )
            assert ref.returncode == 0 and not ref.stderr, (ref.stdout, ref.stderr)
            _, *expected = map(int, ref.stdout.splitlines())
            assert expected == [-3, -3, 7]
            log = root / (mode + ".trace")
            r = subprocess.run(
                [
                    "strace",
                    "-qq",
                    "-s",
                    "512",
                    "-e",
                    "signal=none",
                    "-e",
                    "trace=execve,clone,clone3,vfork,fork,waitid,wait4,ptrace",
                    "-o",
                    log,
                    "--",
                    native,
                    child,
                    mode,
                ],
                capture_output=True,
                check=True,
                timeout=12,
            )
            assert not r.stderr, r.stderr
            pid, *observed = map(int, r.stdout.splitlines())
            trace = log.read_text().splitlines()
            assert any(
                line.startswith("execve(" + json.dumps(str(native)) + ",")
                and line.endswith("= 0")
                for line in trace
            )
            assert any(
                re.match(r"(clone3?|vfork|fork)\(", line) and line.endswith(f"= {pid}")
                for line in trace
            )
            assert any(
                line.startswith(f"wait4({pid},")
                and "WSTOPSIG(s) == SIGQUIT" in line
                and line.endswith(f"= {pid}")
                for line in trace
            )
            assert any(
                line.startswith(f"ptrace(PTRACE_CONT, {pid},") and line.endswith("= 0")
                for line in trace
            )
            assert any(
                line.startswith(f"wait4({pid},")
                and "WEXITSTATUS(s) == 7" in line
                and line.endswith(f"= {pid}")
                for line in trace
            )
            results[mode] = dict(
                parent="imre",
                child="fixture C program: PTRACE_TRACEME and raise(SIGQUIT)",
                expected=expected,
                observed=observed,
                native_stop_signal=3,
                post_resume_reap=7,
            )
            if not a.observe_only and observed != expected:
                raise AssertionError((mode, observed, expected))
        a.result.write_text(
            json.dumps(
                dict(
                    opt_level=a.opt_level,
                    parent_sha256=hashlib.sha256(native.read_bytes()).hexdigest(),
                    modes=results,
                ),
                indent=2,
            )
            + "\n"
        )


if __name__ == "__main__":
    main()
