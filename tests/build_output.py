#!/usr/bin/env python3
"""Check the real CLI's stderr on pipes and a pseudo-terminal."""

import argparse
import errno
import json
import os
from pathlib import Path
import pty
import re
import subprocess
import tempfile


STATUS = re.compile(r"^\s*(Compiling|Reusing|Linking|Finished|Cached|Running)\s+")
ANSI = re.compile(rb"\x1b\[[0-9;]*m")


def actions(lines):
    return [match[1] for line in lines if (match := STATUS.match(line))]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--output", type=Path, help="Save checked example transcripts as JSON")
    args = parser.parse_args()
    if not __debug__:
        parser.error("run with assertions enabled")
    binary = args.compiler.resolve()
    transcripts = {}
    with tempfile.TemporaryDirectory(prefix="relar-output-") as temp:
        root = Path(temp)
        env = dict(os.environ, RELAR_CACHE_DIR=str(root / "cache"), TERM="xterm-256color")
        env.pop("RELAR_CACHE_TRACE", None)
        env.pop("NO_COLOR", None)
        (root / "main.py").write_text(
            'from sys import argv\nfrom render import heading\nprint(heading())\n'
            'print(argv[1])\nassert argv[1] != "fail", "requested failure"\n'
        )
        helper = root / "render.py"

        def edit(value):
            helper.write_text(f'def heading() -> str:\n    return "{value}"\n')

        def invoke(name, *arguments, success=True):
            result = subprocess.run([binary, *arguments], cwd=root, env=env, capture_output=True)
            assert (result.returncode == 0) == success, (arguments, result.returncode, result.stderr)
            assert b"\x1b" not in result.stderr and b"\r" not in result.stderr, result.stderr
            lines = result.stderr.decode().splitlines()
            stdout = result.stdout.decode()
            transcripts[name] = dict(args=list(arguments), stdout=stdout, stderr=lines, code=result.returncode)
            return stdout, lines

        edit("alpha")
        out, cold = invoke("cold", "build", "main.py", "-o", "reporter")
        assert not out and actions(cold) == ["Compiling", "Linking", "Finished"], cold
        assert "(-O3)" in cold[0] and "objects compiled" in cold[1] and "reporter in " in cold[2], cold
        assert (root / "reporter").is_file()
        out, hit = invoke("hit", "run", "main.py", "--", "--quiet")
        assert out == "alpha\n--quiet\n" and actions(hit) == ["Cached", "Running"], (out, hit)
        assert hit[-1].strip() == "Running main.py", hit

        edit("bravo")
        out, normal = invoke("edit", "build", "main.py", "-o", "reporter")
        assert not out and actions(normal) == ["Compiling", "Linking", "Finished"], normal
        assert "compiled" in normal[1] and "reused" in normal[1], normal
        # Keep the edited literal on the same side of the program's other pooled literals.
        edit("cello")
        out, verbose = invoke("verbose", "run", "-v", "main.py", "--", "two")
        assert out == "cello\ntwo\n" and actions(verbose)[-3:] == ["Linking", "Finished", "Running"], verbose
        assert any("render" in l for l in verbose[1:] if l.strip().startswith("Compiling ")), verbose
        assert "Reusing" in actions(verbose), verbose
        out, quiet = invoke("quiet", "run", "main.py", "--quiet", "--", "three")
        assert out == "cello\nthree\n" and quiet == [], (out, quiet)
        out, quiet_build = invoke("quiet_build", "build", "-q", "main.py", "-o", "copy")
        assert not out and quiet_build == [] and (root / "copy").is_file()

        def terminal(overrides):
            master, slave = pty.openpty()
            try:
                process = subprocess.run([binary, "build", "main.py", "-o", "reporter"], cwd=root,
                                         env={**env, **overrides}, stdout=subprocess.PIPE, stderr=slave, check=True)
                assert not process.stdout
            finally:
                os.close(slave)
            output = bytearray()
            try:
                while True:
                    try:
                        chunk = os.read(master, 4096)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        break
                    if not chunk:
                        break
                    output.extend(chunk)
            finally:
                os.close(master)
            return bytes(output)

        for name, overrides, color in (("tty", {}, True), ("no_color", {"NO_COLOR": "1"}, False),
                                       ("dumb_terminal", {"TERM": "dumb"}, False)):
            observed = terminal(overrides)
            assert bool(ANSI.search(observed)) == color and b"Cached" in observed, (name, observed)
            transcripts[name] = {"color": color, "stderr": ANSI.sub(b"", observed).decode().splitlines()}

        out, child = invoke("child_failure", "run", "main.py", "--", "fail", success=False)
        quiet_out, child_error = invoke("quiet_child_failure", "run", "main.py", "-q", "--", "fail", success=False)
        assert actions(child) == ["Cached", "Running"] and child_error and not actions(child_error), child
        assert transcripts["child_failure"]["code"] == transcripts["quiet_child_failure"]["code"]
        assert out == quiet_out and child[2:] == child_error
        helper.write_text("def broken(:\n")
        out, failed = invoke("compile_failure", "run", "main.py", "--", "fail", success=False)
        quiet_out, quiet_error = invoke("quiet_compile_failure", "run", "-q", "main.py", "--", "fail", success=False)
        assert not out and not quiet_out and actions(failed) == ["Compiling"] and quiet_error
        assert not actions(quiet_error) and failed[1:] == quiet_error, (failed, quiet_error)

    if args.output:
        args.output.write_text(json.dumps(transcripts, indent=2) + "\n")
    print(f"{len(transcripts)} CLI output cases passed")


if __name__ == "__main__":
    main()
