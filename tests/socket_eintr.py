"""Compare interrupted Linux TCP syscalls with CPython on one real connection."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile


FIXTURE = Path(__file__).resolve().parent / "fixtures/socket_eintr"
TRACES = {
    "io": ["connect:EINTR", "poll:EINTR", "SO_ERROR:EINTR", "connect:SO_ERROR", "accept:EINTR",
           "send:EINTR", "sendall:EINTR-after-progress", "recv:EINTR"],
    "nonblock": ["connect:EINTR"],
    "refused": ["connect:EINTR", "poll:EINTR", "SO_ERROR:EINTR", "connect:SO_ERROR"],
    "close": ["close:reused"],
    "close-reset": ["close:reused"],
}


def observe(command, shim):
    for mode, trace in TRACES.items():
        env = dict(os.environ, LD_PRELOAD=str(shim), IMRE_SOCKET_FAULT=mode)
        run = subprocess.run([*command, mode], env=env, timeout=15, text=True, capture_output=True)
        assert (run.returncode, run.stdout, run.stderr.splitlines()) == (0, "ok\n", trace), (mode, run)
    # Without the injected interruption, the very same close assertion must fail.
    control = subprocess.run([*command, "close"], timeout=15, text=True, capture_output=True)
    assert control.returncode != 0 and "AssertionError: close did not surface EINTR" in control.stderr, control


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    args = parser.parse_args()
    if not sys.platform.startswith("linux"):
        parser.error("the syscall interposer targets Linux; use socket_tcp.py on other POSIX systems")
    with tempfile.TemporaryDirectory(prefix="imre-socket-eintr-") as temp:
        work = Path(temp)
        shim = work / "fault.so"
        subprocess.run([args.clang, "-O2", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
                        FIXTURE / "interpose.c", "-ldl", "-o", shim], check=True, timeout=60)
        source = FIXTURE / "exercise.py"
        observe([args.python, "-E", source], shim)
        print("CPython: interrupted calls, partial writes and fd reuse passed", flush=True)
        for level in args.opt_levels:
            binary = work / "exercise"
            subprocess.run([args.compiler.resolve(), "build", source, "--no-cache", "--opt-level", level,
                            "--runtime", args.runtime.resolve(), "--clang", args.clang, "-o", binary],
                           check=True, timeout=180)
            observe([binary], shim)
            print(f"O{level}: five syscall traces matched CPython; clean close control rejected", flush=True)


if __name__ == "__main__":
    main()
