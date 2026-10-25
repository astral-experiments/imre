"""Drive native file redirects from the original CPython tempfile fixtures."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, run_in_fixtures

# Original child payload length; the host also executes the upstream assertions
# independently. Its fd offset must move *before* its own read or seek.
METHODS = {
    "test_stdin_filedes": (("stdin",), 4),
    "test_stdout_filedes": (("stdout",), 6),
    "test_stderr_filedes": (("stderr",), 10),
    "test_stdin_fileobj": (("stdin",), 4),
    "test_stdout_fileobj": (("stdout",), 6),
    "test_stderr_fileobj": (("stderr",), 10),
    "test_stdout_stderr_file": (("stdout", "stderr"), 11),
}
SELECT = "poll,ppoll,select,pselect6"
PREAMBLE = """import subprocess
import sys
from typing import TextIO

def send(reply: TextIO, message: str) -> None:
    reply.write(message + "\\n")
    reply.flush()
"""


def file_hash(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def actor_source(arguments, files):
    setup = []
    bindings = []
    slots = {}
    if files:
        setup.append("import os")
    for key, fd in arguments.items():
        if not files:
            bindings.append(f"{key}={fd}")
            continue
        if fd not in slots:
            slot = slots[fd] = f"file{len(slots)}"
            mode = "rb" if key == "stdin" else "wb"
            # Keep the actual open file description. Reopening /proc/self/fd/N
            # on Linux would give the wrapper an independent seek offset.
            setup.append(f'{slot} = open("/dev/null", "{mode}")')
            setup.append(f"os.dup2({fd}, {slot}.fileno(), 0)")
        bindings.append(f"{key}={slots[fd]}")
    launch = "process = subprocess.Popen(sys.argv[1:], " + ", ".join(bindings) + ")"
    return (
        PREAMBLE
        + "\n".join(setup + [launch])
        + """
with open("/dev/stdin") as control, open("/dev/stdout", "w") as reply:
    send(reply, "ready")
    while True:
        request = control.readline().strip()
        if request == "wait":
            send(reply, str(process.wait()))
        elif request == "returncode":
            code = process.returncode
            if code is None:
                send(reply, "None")
            else:
                send(reply, str(code))
        else:
            break
"""
    ).encode()


def receive(actor):
    if not select.select([actor.stdout], [], [], 30)[0]:
        raise RuntimeError("native actor did not reply")
    line = actor.stdout.readline()
    if not line:
        raise RuntimeError("native actor exited: " + actor.stderr.read())
    return line.rstrip("\n")


def traced(binary, trace, argv=()):
    return [
        "strace",
        "-qq",
        "-e",
        "signal=none",
        "-e",
        "trace=" + SELECT,
        "-o",
        trace,
        "--",
        binary,
        *argv,
    ]


def run_case(case, original, tc, work, binaries):
    from test import test_subprocess as upstream

    method = case._testMethodName
    keys, expected = METHODS[method]
    files = not method.endswith("_filedes")
    trace = work / (case.id() + ".trace")
    evidence = {
        "operations": [],
        "representation": "native file object" if files else "integer fd",
    }
    actors = []

    def launch(argv, **kwargs):
        if (
            actors
            or type(argv) is not list
            or any(type(x) is not str for x in argv)
            or tuple(kwargs) != keys
        ):
            raise RuntimeError("unexpected original Popen arguments")
        if any((type(x) is not int) != files for x in kwargs.values()):
            raise RuntimeError("original redirect representation changed")
        descriptors = {
            key: value.fileno() if files else value for key, value in kwargs.items()
        }
        [fd] = set(
            descriptors.values()
        )  # stdout=tf, stderr=tf must use the same object and offset.
        if files and len({id(value) for value in kwargs.values()}) != 1:
            raise AssertionError("expected one original tempfile object")
        if os.lseek(fd, 0, os.SEEK_CUR) != 0:
            raise AssertionError("unexpected original starting offset")
        src = actor_source(descriptors, files)
        digest = hashlib.sha256(src).hexdigest()
        if digest not in binaries:
            directory = work / digest
            directory.mkdir()
            binaries[digest] = tc.build(src, directory)
        actor = subprocess.Popen(
            traced(binaries[digest], trace, argv),
            pass_fds=(fd,),
            text=True,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        actors.append(actor)
        if receive(actor) != "ready":
            raise RuntimeError("native Popen did not complete")
        evidence.update(
            actor_source_sha256=digest,
            actor_binary_sha256=file_hash(binaries[digest]),
            fixture_fd_offset_before=0,
            native_parents=1,
            child="upstream CPython -c data producer/consumer",
        )
        evidence["operations"].append("Popen")

        def query(request):
            actor.stdin.write(request + "\n")
            actor.stdin.flush()
            return receive(actor)

        class Child:
            def wait(self):
                code = int(query("wait"))
                offset = os.lseek(fd, 0, os.SEEK_CUR)
                if offset != expected:
                    raise AssertionError(
                        (
                            "native redirection did not advance the original offset",
                            offset,
                            expected,
                        )
                    )
                evidence["fixture_fd_offset_after_wait"] = offset
                evidence["operations"].append("Popen.wait")
                return code

            @property
            def returncode(self):
                response = query("returncode")
                evidence["operations"].append("Popen.returncode")
                return None if response == "None" else int(response)

        return Child()

    try:
        with patch.object(upstream, "subprocess", SimpleNamespace(Popen=launch)):
            original()
    finally:
        for actor in actors:
            actor.stdin.close()
            if actor.wait(timeout=30) or actor.stderr.read():
                raise AssertionError("native redirect actor or tracer failed")
            actor.stdout.close()
            actor.stderr.close()
    if "Popen.wait" not in evidence["operations"] or trace.read_text().strip():
        raise AssertionError("missing native wait or unexpected selector syscall")
    return dict(
        **evidence,
        native_parent_selector_calls=0,
        roles=dict(
            body="cpython", assertions="cpython", fixture="cpython", operation="imre"
        ),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + key, type=Path, required=True)
    parser.add_argument("--kind", choices=("numbers", "files", "all"), required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        binary = tc.build(
            b'import subprocess\np = subprocess.Popen(["/bin/sh", "-c", "printf control"], stdout=subprocess.PIPE)\nassert p.communicate(timeout=10)[0] == b"control"\n',
            work,
        )
        positive = work / "positive.trace"
        subprocess.run(traced(binary, positive), check=True, timeout=30)
        control = positive.read_text().splitlines()
        if not any(line.startswith("poll(") for line in control):
            raise AssertionError(("positive selector control", control))
        cache = {}

        def execute(case, original):
            return run_case(case, original, tc, work, cache)

        wanted = {
            f"test.test_subprocess.{cls}.{name}": execute
            for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
            for name in METHODS
            if args.kind == "all"
            or (args.kind == "numbers") == name.endswith("_filedes")
        }
        rows, errors, skips = run_in_fixtures(load_suite(manifest["module"]), wanted)
        if errors or skips or rows.keys() != wanted.keys():
            raise AssertionError((errors, skips, wanted.keys() - rows.keys()))
        args.result.write_text(
            json.dumps(
                dict(
                    cpython_commit=manifest["commit"],
                    opt_level=args.opt_level,
                    toolchain={
                        key: file_hash(getattr(args, key))
                        for key in ("imre", "runtime", "clang")
                    },
                    selector_control_count=len(control),
                    rows=rows,
                ),
                indent=2,
            )
            + "\n"
        )


if __name__ == "__main__":
    main()
