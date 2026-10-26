"""Trace native child attributes under the three unchanged CPython methods."""

import argparse
import ast
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from launch_errno_originals import digest
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

IDS = {
    "test.test_subprocess.POSIXProcessTestCase." + name
    for name in ("test_start_new_session", "test_process_group_0", "test_umask")
}
ACTOR = b"""import subprocess
import sys
option = sys.argv[1]
value = int(sys.argv[2])
args = sys.argv[3:]
if option == "start_new_session":
    print(subprocess.check_output(args, start_new_session=bool(value)))
elif option == "process_group":
    print(subprocess.check_output(args, process_group=value))
elif option == "umask":
    print(subprocess.check_call(args, umask=value))
else:
    raise ValueError("unknown child setting")
"""


def trace_call(binary, root, api, args, kwargs):
    assert type(args) is list and args[:2] == [sys.executable, "-c"] and len(args) == 3
    [(option, value)] = kwargs.items()
    syscall, query = {
        "start_new_session": ("setsid", "getsid"),
        "process_group": ("setpgid", "getpgid"),
        "umask": ("umask", "openat"),
    }[option]
    assert api == ("check_call" if option == "umask" else "check_output")
    with tempfile.TemporaryDirectory(dir=root) as directory:
        logs = Path(directory)
        result = subprocess.run(
            [
                "strace",
                "-ff",
                "-qq",
                "-s",
                "1024",
                "-e",
                "signal=none",
                "-e",
                "trace=execve,clone,clone3,fork,vfork,wait4,setsid,getpgid,getsid,setpgid,umask,openat",
                "-o",
                logs / "pid",
                "--",
                binary,
                option,
                str(int(value)),
                *args,
            ],
            capture_output=True,
            check=True,
            timeout=30,
        )
        assert not result.stderr, result.stderr
        data = ast.literal_eval(result.stdout.decode())
        traces = {
            int(f.suffix[1:]): f.read_text().splitlines() for f in logs.glob("pid.*")
        }

        def exec_line(pid, path):
            return [
                i
                for i, line in enumerate(traces[pid])
                if line.startswith("execve(" + json.dumps(str(path)) + ",")
                and line.endswith("= 0")
            ]

        [parent] = [pid for pid in traces if exec_line(pid, binary)]
        [child] = [pid for pid in traces if exec_line(pid, args[0])]
        [executed] = exec_line(child, args[0])
        assert parent != child and json.dumps(args[2]) in traces[child][executed]
        assert any(
            re.match(r"(clone3?|fork|vfork)\(", line) and line.endswith(f"= {child}")
            for line in traces[parent]
        )
        assert (
            sum(
                line.startswith(f"wait4({child},") and line.endswith(f"= {child}")
                for line in traces[parent]
            )
            == 1
        )
        [applied] = [
            line for line in traces[child][:executed] if line.startswith(syscall + "(")
        ]
        after = traces[child][executed + 1 :]
        if option == "umask":
            assert (
                type(data) is int
                and data == 0
                and re.fullmatch(r"umask\(053\)\s+= 0[0-7]*", applied)
            )
            # The original method independently stats this file and removes its directory.
            call = ast.parse(args[2]).body[0].value.func.value
            name = ast.literal_eval(call.args[0])
            assert any(
                line.startswith("openat(")
                and json.dumps(name) in line
                and "O_CREAT" in line
                for line in after
            )
            mode = os.stat(name).st_mode & 0o666
            assert mode == 0o624
        else:
            assert type(data) is bytes and int(data) == child
            if option == "start_new_session":
                assert re.fullmatch(rf"setsid\(\)\s+= {child}", applied)
                assert int(data) != os.getsid(0)
            else:
                assert re.fullmatch(r"setpgid\(0, 0\)\s+= 0", applied)
                assert int(data) != os.getpgid(0)
            assert any(
                re.fullmatch(rf"{query}\(0\)\s+= {child}", line) for line in after
            )
            mode = None
        return data, dict(
            api=api,
            option=option,
            value=value,
            pre_exec_syscall=syscall,
            child_observation=query,
            child_id_equals_pid=option != "umask",
            output_file_mode=mode,
            successful_parent_reaps=1,
            data_child="pinned CPython",
        )


def measure(tc, root, binary, case, original):
    from test import test_subprocess as upstream

    with tempfile.TemporaryDirectory(dir=root) as directory:
        work = Path(directory)
        try:
            owner, preamble = fixture(case, work)
            function = native_method(tc, work, SOURCE, owner, case.id(), preamble)
        except (RuntimeError, ValueError) as exc:
            attempt = dict(
                status="blocked",
                reason=str(exc).replace(str(work), "<case>").strip().splitlines()[-1],
            )
        else:
            attempt = dict(status="completed", **function(case, original))
    calls = []

    def forward(api, args, kwargs):
        value, proof = trace_call(binary, root, api, args, kwargs)
        calls.append(proof)
        return value

    class SubprocessCalls:
        def check_call(self, args, **kwargs):
            return forward("check_call", args, kwargs)

        def check_output(self, args, **kwargs):
            return forward("check_output", args, kwargs)

    with patch.object(upstream, "subprocess", SubprocessCalls()):
        original()
    [observed] = calls
    return dict(
        body_attempt=attempt,
        observed=observed,
        roles=dict(
            body="cpython", assertions="cpython", operation="imre", fixture="cpython"
        ),
    )


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    for key in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + key, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with (
        tempfile.TemporaryDirectory() as td,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        root = Path(td)
        binary = tc.build(ACTOR, root)

        def execute(case, original):
            return measure(tc, root, binary, case, original)

        rows, failures, skips = run_in_fixtures(
            load_suite("test.test_subprocess"), dict.fromkeys(IDS, execute)
        )
        assert rows.keys() == IDS and not failures and not skips, (failures, skips)
        report = dict(
            cpython_commit=manifest["commit"],
            opt_level=tc.level,
            rows=rows,
            actor_sha256=digest(binary),
            toolchain={
                key: digest(getattr(args, key)) for key in ("imre", "runtime", "clang")
            },
        )
    args.result.write_text(json.dumps(report, indent=2) + "\n")
