"""Run the original descriptor leak loops against one traced native parent per fixture."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import resource
import select
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, run_in_fixtures

IDS = tuple(
    f"test.test_subprocess.{cls}.test_leaking_fds_on_error"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
)
SELECTORS = ("poll", "ppoll", "select", "pselect6")
ACTOR = b"""import subprocess
import sys
from typing import TextIO

def send(stream: TextIO, message: str) -> None:
    stream.write(message + "\\n")
    stream.flush()

command = (sys.argv[1],)
with open("/dev/stdin") as channel, open("/dev/stdout", "w") as reply:
    send(reply, "ready")
    while channel.readline().strip() == "launch":
        try:
            child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        except FileNotFoundError as exc:
            send(reply, "FileNotFoundError " + str(exc))
        else:
            child.kill()
            child.wait()
            raise RuntimeError("missing executable unexpectedly started")
"""


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def fd_state(pid):
    directory = Path(f"/proc/{pid}/fd")
    return {entry.name: entry.readlink() for entry in directory.iterdir()}


def lower_limit():
    _, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(
        resource.RLIMIT_NOFILE,
        (min(hard, 64) if hard != resource.RLIM_INFINITY else 64, hard),
    )


def receive(process):
    if not select.select([process.stdout], [], [], 30)[0]:
        raise RuntimeError("native parent did not reply")
    line = process.stdout.readline()
    if not line:
        raise RuntimeError("native parent exited: " + process.stderr.read())
    return line.rstrip("\n")


def exercise(executable, directory, case, original):
    from test import test_subprocess as upstream

    command = upstream.NONEXISTING_CMD
    if not (type(command) is tuple and len(command) == 1 and type(command[0]) is str):
        raise ValueError("unexpected original command shape")
    trace = directory / (type(case).__name__ + ".trace")
    syscalls = (*SELECTORS, "clone", "clone3", "fork", "vfork", "wait4", "waitid")
    # No -f: selector, fork and wait evidence must belong to the native parent.
    argv = [
        "strace",
        "-qq",
        "-e",
        "signal=none",
        "-e",
        "trace=" + ",".join(syscalls),
        "-o",
        trace,
        "--",
        executable,
        command[0],
    ]
    with subprocess.Popen(
        argv,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
        preexec_fn=lower_limit,
    ) as tracer:
        count = 0
        try:
            if receive(tracer) != "ready":
                raise AssertionError("actor did not reach its receive loop")
            children = (
                Path(f"/proc/{tracer.pid}/task/{tracer.pid}/children")
                .read_text()
                .split()
            )
            [pid] = map(int, children)
            if not Path(f"/proc/{pid}/exe").samefile(executable):
                raise AssertionError("descriptor observation is not on the Imre parent")
            limit = next(
                line
                for line in Path(f"/proc/{pid}/limits").read_text().splitlines()
                if line.startswith("Max open files")
            ).split()[3]
            if int(limit) != 64:
                raise AssertionError(("native descriptor limit", limit))
            baseline = fd_state(pid)

            def launch(args, **options):
                nonlocal count
                if args is not command or options != {
                    "stdout": subprocess.PIPE,
                    "stderr": subprocess.PIPE,
                }:
                    raise RuntimeError(
                        "the original launch differs from the native request"
                    )
                tracer.stdin.write("launch\n")
                tracer.stdin.flush()
                response = receive(tracer)
                if not response.startswith("FileNotFoundError "):
                    raise RuntimeError(
                        "native launch did not report the expected error: " + response
                    )
                count += 1
                raise FileNotFoundError(response.removeprefix("FileNotFoundError "))

            with patch.object(
                upstream,
                "subprocess",
                SimpleNamespace(Popen=launch, PIPE=subprocess.PIPE),
            ):
                original()
            final = fd_state(pid)
            if count != 1024 or baseline != final:
                raise AssertionError(
                    (count, "parent descriptors changed", len(baseline), len(final))
                )
        finally:
            tracer.stdin.close()
            if tracer.wait(timeout=30) or tracer.stderr.read():
                raise AssertionError("native actor/strace failed")
    calls = Counter()
    for line in trace.read_text().splitlines():
        match = re.match(r"^(\w+)\(.*\)\s+= (\d+)", line)
        if match:
            calls[match[1]] += 1
        if line.startswith(tuple(name + "(" for name in SELECTORS)):
            raise AssertionError("unexpected selector use by failed native Popen")
    forks = sum(calls[name] for name in ("clone", "clone3", "fork", "vfork"))
    waits = calls["wait4"] + calls["waitid"]
    if forks != count or waits != count:
        raise AssertionError(
            ("expected a real child and reap per request", forks, waits, count)
        )
    return {
        "native_parent_count": 1,
        "native_Popen_errors": count,
        "native_child_creations": forks,
        "native_child_reaps": waits,
        "parent_fd_count_before": len(baseline),
        "parent_fd_count_after": len(final),
        "descriptor_targets_unchanged": True,
        "parent_selector_calls": 0,
        "descriptor_soft_limit": 64,
        "roles": {
            "body": "cpython",
            "assertions": "cpython",
            "operation": "imre",
            "fixture": "cpython",
        },
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "work", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    p.add_argument("--mode", choices=("build", "exercise"), required=True)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    from test import support

    support.use_resources = ["cpu"]
    args.work.mkdir(parents=True, exist_ok=True)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    executable, stamp = args.work / "case", args.work / "built.json"
    provenance = {
        name: digest(getattr(args, name)) for name in ("imre", "runtime", "clang")
    }
    provenance.update(
        opt_level=args.opt_level, source_sha256=hashlib.sha256(ACTOR).hexdigest()
    )
    if args.mode == "build":
        tc.build(ACTOR, args.work)
        stamp.write_text(json.dumps({**provenance, "executable": digest(executable)}))
        return
    if json.loads(stamp.read_text()) != {
        **provenance,
        "executable": digest(executable),
    }:
        raise ValueError("wrong compiled actor or toolchain")

    def reference_case(case, original):
        original()
        return "pass"

    reference = dict.fromkeys(IDS, reference_case)
    reference, errors, skips = run_in_fixtures(
        load_suite(manifest["module"]), reference
    )
    if errors or skips or set(reference) != set(IDS):
        raise AssertionError((errors, skips, reference))
    selected = dict.fromkeys(
        IDS, lambda case, original: exercise(executable, args.work, case, original)
    )
    rows, errors, skips = run_in_fixtures(load_suite(manifest["module"]), selected)
    if errors or skips or set(rows) != set(IDS):
        raise AssertionError((errors, skips, rows))
    args.result.write_text(
        json.dumps(
            {
                "cpython_commit": manifest["commit"],
                "opt_level": args.opt_level,
                "build": json.loads(stamp.read_text()),
                "configuration": "Linux, CPU resource enabled",
                "reference": reference,
                "observations": rows,
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
