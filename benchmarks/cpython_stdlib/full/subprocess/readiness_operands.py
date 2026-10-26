"""Compare the original select operand with its integer fd on a held-open pipe."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from suite_native import NativeCompileError, Toolchain


PROGRAM = """import select
import subprocess
import sys

p = subprocess.Popen(sys.argv[1:], stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0)
f = p.stdout
print(f.read(4))
item = OPERAND
r, w, x = select.select([item], [], [], 0.0)
print(len(r) == 1 and r[0] COMPARISON item)
p.stdin.close()
f.close()
print(p.wait())
"""
CHILD = "import os; os.write(1, b'apple'); os.read(0, 1)"


def observe(tc, root, name):
    root.mkdir()
    stream = name == "stream"
    source = (
        PROGRAM.replace("OPERAND", "f" if stream else "f.fileno()")
        .replace("COMPARISON", "is" if stream else "==")
        .encode()
    )
    entry = root / "reference.py"
    entry.write_bytes(source)
    argv = [sys.executable, "-c", CHILD]
    oracle = subprocess.check_output([sys.executable, "-B", entry, *argv], timeout=15)
    assert oracle == b"b'appl'\nTrue\n0\n"
    result = {
        "source_sha256": hashlib.sha256(source).hexdigest(),
        "cpython": oracle.decode().splitlines(),
    }
    try:
        binary = tc.build(source, root)
    except NativeCompileError as exc:
        if not stream:
            raise
        result.update(
            native="compile-error",
            diagnostic=str(exc).replace(str(root), "<case>").strip().splitlines()[-1],
        )
        return result
    trace = root / "parent.trace"
    output = subprocess.check_output(
        [
            "strace",
            "-qq",
            "-yy",
            "-e",
            "trace=select,pselect6,poll,ppoll,wait4",
            "-o",
            trace,
            "--",
            binary,
            *argv,
        ],
        timeout=15,
    )
    assert output == oracle
    calls = trace.read_text().splitlines()
    ready = [
        line
        for line in calls
        if re.match(r"(?:select|pselect6|poll|ppoll)\(", line)
        and "pipe:[" in line
        and re.search(r"= 1\b", line)
    ]
    reaps = [
        line
        for line in calls
        if line.startswith("wait4(")
        and "WEXITSTATUS(s) == 0" in line
        and re.search(r"= [1-9]\d*$", line)
    ]
    assert len(ready) == len(reaps) == 1, calls
    result.update(
        native="pass",
        stdout=output.decode().splitlines(),
        native_readiness_syscalls=1,
        native_reaps=1,
    )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as tmp:
        result = {
            name: observe(tc, Path(tmp) / name, name) for name in ("stream", "integer")
        }
    args.result.write_text(
        json.dumps({"opt_level": tc.level, "operands": result}, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
