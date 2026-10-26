"""Trace the original shell-output method and distinguish its native parent and shell."""

# ruff: noqa: E402
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from host_popen import verify
from suite_inventory import load_suite
from suite_native import Toolchain, native_method, run_in_fixtures

ID = "test.test_subprocess.MiscTests.test_getoutput"
ACTOR = b"""import subprocess
import sys
if sys.argv[1] == "getoutput":
    print(repr(subprocess.getoutput(sys.argv[2])))
else:
    print(repr(subprocess.getstatusoutput(sys.argv[2])))
"""


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def trace(binary, api, command, directory):
    """Identify the shell and its actual pipe writes, reads, end of file and reap."""
    directory.mkdir()
    result = subprocess.run(
        [
            "strace",
            "-ff",
            "-qq",
            "-yy",
            "-s",
            "512",
            "-e",
            "signal=none",
            "-e",
            "trace=execve,clone,clone3,fork,vfork,dup2,read,write,wait4",
            "-o",
            directory / "trace",
            "--",
            binary,
            api,
            command,
        ],
        cwd=directory,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    assert not result.stderr
    processes = {
        int(p.suffix[1:]): p.read_text().splitlines() for p in directory.glob("trace.*")
    }

    def exec_pid(pattern):
        [pid] = [
            pid
            for pid, lines in processes.items()
            if any(re.match(pattern, s) and s.endswith("= 0") for s in lines)
        ]
        return pid

    def forked(pid, child):
        return any(
            re.match(r"(clone3?|fork|vfork)\(", s) and s.endswith(f"= {child}")
            for s in processes[pid]
        )

    parent = exec_pid(re.escape("execve(" + json.dumps(str(binary)) + ","))
    child = exec_pid(
        re.escape('execve("/bin/sh", ["/bin/sh", "-c", ' + json.dumps(command) + "]")
    )
    assert child != parent and forked(parent, child)
    [pipe] = [
        m[1]
        for s in processes[child]
        if (
            m := re.fullmatch(r"dup2\(1<pipe:\[(\d+)\]>, 2<.*>\)\s+= 2<pipe:\[\1\]>", s)
        )
    ]
    [status] = [
        int(m[1])
        for s in processes[parent]
        if (
            m := re.match(
                rf"wait4\({child}, .*WEXITSTATUS\(s\) == (\d+).* = {child}$", s
            )
        )
    ]
    missing = command.startswith("cat ")
    writer, fd = child, 1
    if missing:
        name = command.removeprefix("cat ")
        writer = exec_pid(
            r'execve\("[^"]*/cat", ' + re.escape('["cat", ' + json.dumps(name) + "]")
        )
        assert writer == child or forked(child, writer)
        fd = 2
    reads = [
        int(m[1])
        for s in processes[parent]
        if (m := re.match(rf"read\(\d+<pipe:\[{pipe}\]>, .*\)\s+= (\d+)$", s))
    ]
    writes = [
        int(m[1])
        for s in processes[writer]
        if (m := re.match(rf"write\({fd}<pipe:\[{pipe}\]>, .*\)\s+= (\d+)$", s))
    ]
    value = ast.literal_eval(result.stdout)
    if api == "getstatusoutput":
        assert isinstance(value, tuple) and len(value) == 2 and value[0] == status
        output = value[1]
    else:
        output = value
    assert reads[-1] == 0 and sum(reads) == sum(writes) == len(output.encode()) + 1
    assert status == (1 if missing else 0)
    if missing:
        path = Path(name)
        assert path.name == "foo" and path.parent.is_dir() and not path.exists()
        assert name in output and "No such file or directory" in output
        shown_command = "cat <original-tempdir>/foo"
        shown_output = output.replace(str(path.parent), "<original-tempdir>")
    else:
        assert command == "echo xyzzy" and output == "xyzzy"
        shown_command, shown_output = command, output
    return value, dict(
        api=api,
        command=shown_command,
        output=shown_output,
        native_exit=status,
        native_reaps=1,
        child_write_fd=fd,
        stderr_duplicated_to_stdout=True,
        native_drain_to_eof=True,
        native_read_bytes=sum(reads),
        child_write_bytes=sum(writes),
    )


def run_original(tc, binary, root, manifest, report, mode):
    from test import test_subprocess as upstream

    ops, original_dirs = [], []

    def measure(case, original):
        if mode == "original":
            body = root / "body"
            body.mkdir()
            try:
                owner, pre = fixture(case, body)
                execute = native_method(tc, body, SOURCE, owner, ID, pre)
            except (RuntimeError, ValueError) as exc:
                reason = str(exc).replace(str(body), "<case>").strip().splitlines()[-1]
                report["body_attempt"] = dict(status="blocked", reason=reason)
            else:
                report["body_attempt"] = dict(
                    status="completed", **execute(case, original)
                )

        def call(api, command):
            value, observed = trace(binary, api, command, root / f"{mode}-{len(ops)}")
            ops.append(observed)
            if command.startswith("cat "):
                original_dirs.append(Path(command[4:]).parent)
                if mode == "withhold-status":
                    return 0, value[1]
            return value

        with patch.object(
            upstream,
            "subprocess",
            SimpleNamespace(
                getoutput=lambda cmd: call("getoutput", cmd),
                getstatusoutput=lambda cmd: call("getstatusoutput", cmd),
            ),
        ):
            original()
        return dict(
            body="cpython", assertions="cpython", fixture="cpython", operation="imre"
        )

    rows, failures, skips = run_in_fixtures(
        load_suite(manifest["module"]), {ID: measure}
    )
    assert not skips and len(original_dirs) == 1 and not original_dirs[0].exists(), (
        failures
    )
    assert [op["native_exit"] for op in ops] == [0, 0, 1]
    if mode == "original":
        assert not failures and rows.keys() == {ID}, failures
        [roles] = rows[ID]
        report["rows"] = {
            ID: [dict(roles=roles, calls=ops, original_tempdir_removed=True)]
        }
    else:
        [(case_id, traceback)] = failures
        assert (
            case_id == ID
            and not rows
            and traceback.strip().endswith("AssertionError: 0 == 0")
        ), failures
        report["withheld_status"] = dict(
            native_exit=ops[-1]["native_exit"],
            original_failure=traceback.strip().splitlines()[-1],
            original_tempdir_removed=True,
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    manifest = verify()
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(tmp)
        actor = root / "actor"
        actor.mkdir()
        binary = tc.build(ACTOR, actor)
        report = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            toolchain={k: sha(getattr(args, k)) for k in ("imre", "runtime", "clang")},
            native_actor_sha256=sha(binary),
        )
        for mode in ("original", "withhold-status"):
            run_original(tc, binary, root, manifest, report, mode)
        args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
