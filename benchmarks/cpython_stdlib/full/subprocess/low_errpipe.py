"""Trace an unchanged Popen body after its native parent closed stdin and stdout."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from attempt_bodies import SOURCE, fixture
from host_popen import verify
from suite_inventory import load_suite
from suite_native import Toolchain, native_method, run_in_fixtures

NAME = "test.test_subprocess.POSIXProcessTestCase.test_small_errpipe_write_fd"
SYSCALLS = "execve,fcntl,close,close_range,dup,dup2,dup3,pipe,pipe2,socketpair,clone,clone3,vfork,wait4,waitid"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for n in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + n, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    p.add_argument("--logs", type=Path)
    args = p.parse_args()
    manifest = verify()
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)

    def attempt(case, original):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            logs = args.logs or directory
            logs.mkdir(exist_ok=True)
            owner, preamble = fixture(case, directory)

            class TracedToolchain:
                level = tc.level

                def build(self, source, work):
                    binary = tc.build(source, work)
                    wrapper = work / "traced"
                    command = [
                        "strace",
                        "-ff",
                        "-qq",
                        "-e",
                        "signal=none",
                        "-e",
                        "trace=" + SYSCALLS,
                        "-o",
                        str(logs / "log"),
                        "--",
                        str(binary),
                    ]
                    wrapper.write_text("#!/bin/sh\nexec " + shlex.join(command) + "\n")
                    wrapper.chmod(0o755)
                    return wrapper

            execute = native_method(
                TracedToolchain(), directory, SOURCE, owner, NAME, preamble
            )
            row = execute(case, original)
            traces = {
                int(p.suffix[1:]): p.read_text().splitlines()
                for p in logs.glob("log.*")
            }

            def find_exec(path):
                target = "execve(" + json.dumps(str(path)) + ","
                [pid] = [
                    pid
                    for pid, lines in traces.items()
                    if any(
                        line.startswith(target) and line.endswith("= 0")
                        for line in lines
                    )
                ]
                return pid

            parent, child = find_exec(directory / "case"), find_exec(sys.executable)
            assert parent != child
            lines = traces[parent]

            def first(pattern, start=0):
                for pos, line in enumerate(lines[start:], start):
                    if match := re.fullmatch(pattern, line):
                        return pos, match
                raise AssertionError(("missing native parent operation", pattern))

            saved = []
            cursor = 0
            for fd in (0, 1):
                cursor, match = first(
                    rf"fcntl\({fd}, F_DUPFD_CLOEXEC, 0\)\s+= (\d+)", cursor
                )
                saved.append(int(match[1]))
                cursor += 1
            assert len(set(saved)) == 2 and min(saved) > 2
            for fd in (0, 1):
                cursor, _ = first(rf"close\({fd}\)\s+= 0", cursor)
                cursor += 1
            cursor, _ = first(
                r"socketpair\(AF_UNIX, SOCK_SEQPACKET\|SOCK_CLOEXEC, 0, \[0, 1\]\) = 0",
                cursor,
            )
            cursor, _ = first(rf"(?:clone|clone3|vfork)\(.*\)\s+= {child}", cursor + 1)
            cursor, _ = first(
                rf"wait4\({child}, .*WEXITSTATUS\(s\) == 0.*\)\s+= {child}", cursor + 1
            )
            for fd, original_fd in enumerate(saved):
                cursor, _ = first(rf"dup2\({original_fd}, {fd}\)\s+= {fd}", cursor + 1)
            row["native_parent_sha256"] = hashlib.sha256(
                (directory / "case").read_bytes()
            ).hexdigest()
            row["native_closed_fds"] = [0, 1]
            row["launch_error_socket"] = {
                "type": "SOCK_SEQPACKET|SOCK_CLOEXEC",
                "fds": [0, 1],
            }
            row["data_child"] = {
                "engine": "cpython",
                "exec_observed": True,
                "wait_status": 0,
            }
            row["restored_with_dup2"] = [0, 1]
            return row

    with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
        rows, errors, skips = run_in_fixtures(
            load_suite(manifest["module"]), {NAME: attempt}
        )
    if set(rows) != {NAME} or errors or skips:
        raise AssertionError((errors, skips))
    args.result.write_text(
        json.dumps({"opt_level": args.opt_level, "rows": rows}, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
