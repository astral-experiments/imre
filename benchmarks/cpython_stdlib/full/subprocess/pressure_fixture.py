"""Exercise all 15 original three-pipe launches in one native parent with ten free fds."""

import argparse
import ast
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import select
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

IDS = {
    f"test.test_subprocess.{cls}.test_no_leaking"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
}
ACTOR = """import os
import selectors
import subprocess
import sys
SELECTOR
for number in sys.argv[1].split(","):
    os.set_inheritable(int(number), False)
args = sys.argv[2:]
print("ready")
while input() == "launch":
    child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    print("spawned")
    output, errors = child.communicate(input().encode())
    print(repr(output))
    print(repr(errors))
    code = child.returncode
    if code is None:
        raise AssertionError("communicate did not reap the child")
    else:
        print(code)
"""


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def fd_state(pid):
    state = {}
    for path in Path(f"/proc/{pid}/fd").iterdir():
        try:
            state[int(path.name)] = str(path.readlink())
        except FileNotFoundError:
            pass  # Opening /proc/self/fd for enumeration briefly uses one of the free slots.
    return state


def line(process):
    # An unbuffered binary transport avoids a text reader prefetching the next response.
    if not select.select([process.stdout], [], [], 30)[0]:
        raise AssertionError("native parent did not reply")
    value = process.stdout.readline()
    if not value:
        raise AssertionError(process.stderr.read())
    return value.rstrip(b"\n")


def exercise(binary, directory, case, original):
    from test import test_subprocess as upstream
    from test.support import os_helper

    selector = upstream.subprocess._PopenSelector.__name__
    assert selector == (
        "SelectSelector" if type(case).__name__.endswith("NoPoll") else "PollSelector"
    )
    process = pid = command = baseline = None
    pressure = {}
    calls = 0
    reaped = 0
    trace = directory / "pid"
    syscalls = "execve,clone,clone3,fork,vfork,wait4,waitid,poll,ppoll,select,pselect6"

    def send(value):
        process.stdin.write(value + b"\n")

    class Proxy:
        def communicate(self, data):
            nonlocal reaped
            if type(data) is not bytes or b"\n" in data or not data.isascii():
                raise AssertionError("unexpected upstream input representation")
            send(data)
            out, err = (ast.literal_eval(line(process).decode()) for _ in range(2))
            if line(process) != b"0":
                raise AssertionError(
                    "the native parent did not report a successful wait"
                )
            if fd_state(pid) != baseline:
                raise AssertionError(
                    "a launch or communicate changed the native descriptor table"
                )
            reaped += 1
            return out, err

    def launch(args, **kw):
        nonlocal process, pid, command, baseline, calls, pressure
        if (
            type(args) is not list
            or not all(type(s) is str for s in args)
            or kw
            != {
                "stdin": subprocess.PIPE,
                "stdout": subprocess.PIPE,
                "stderr": subprocess.PIPE,
            }
        ):
            raise AssertionError(("unexpected original launch", args, kw))
        if process is None:
            command = args.copy()
            host = fd_state("self")
            pressure = {
                fd: path
                for fd, path in host.items()
                if Path(path).name == os_helper.TESTFN
            }
            limit = resource.getrlimit(resource.RLIMIT_NOFILE)[0]
            if set(host) - pressure.keys() != {0, 1, 2} or limit - len(host) != 10:
                raise AssertionError(
                    (
                        "unexpected pressure in original fixture",
                        limit,
                        len(host),
                        set(host) - pressure.keys(),
                    )
                )
            if any(os.get_inheritable(fd) for fd in pressure):
                raise AssertionError("original pressure descriptors must be CLOEXEC")
            numbers = tuple(sorted(pressure))
            # The native executable inherits the actual open file descriptions
            # at their original numbers. It restores their original CLOEXEC
            # flags before responding. Its control data uses only stdin/stdout.
            process = subprocess.Popen(
                [
                    "strace",
                    "-ff",
                    "-qq",
                    "-s",
                    "512",
                    "-e",
                    "signal=none",
                    "-e",
                    "trace=" + syscalls,
                    "-o",
                    trace,
                    "--",
                    binary,
                    ",".join(map(str, numbers)),
                    *args,
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                pass_fds=numbers,
                bufsize=0,
            )
            if line(process) != b"ready":
                raise AssertionError("actor did not initialize")
            [pid] = map(
                int,
                Path(f"/proc/{process.pid}/task/{process.pid}/children")
                .read_text()
                .split(),
            )
            if not Path(f"/proc/{pid}/exe").samefile(binary):
                raise AssertionError(
                    "fixture descriptor table does not belong to the Imre parent"
                )
            native_limit = next(
                v
                for v in Path(f"/proc/{pid}/limits").read_text().splitlines()
                if v.startswith("Max open files")
            ).split()[3]
            baseline = fd_state(pid)
            if (
                int(native_limit) != limit
                or baseline.keys() != host.keys()
                or any(baseline[fd] != path for fd, path in pressure.items())
            ):
                raise AssertionError(
                    (
                        "native descriptors do not match the original pressure",
                        limit,
                        len(baseline),
                    )
                )
            for fd in numbers:
                flag = next(
                    v
                    for v in Path(f"/proc/{pid}/fdinfo/{fd}").read_text().splitlines()
                    if v.startswith("flags:")
                )
                if not int(flag.split()[1], 8) & os.O_CLOEXEC:
                    raise AssertionError(("original CLOEXEC not restored", fd))
        elif args != command:
            raise AssertionError("upstream changed the command between launches")
        send(b"launch")
        if line(process) != b"spawned":
            raise AssertionError("native Popen did not launch")
        calls += 1
        return Proxy()

    try:
        with patch.object(
            upstream, "subprocess", SimpleNamespace(Popen=launch, PIPE=subprocess.PIPE)
        ):
            original()
        if (calls, reaped) != (15, 15):
            raise AssertionError((calls, reaped))
        send(b"stop")
        if process.wait(timeout=10):
            raise AssertionError(process.stderr.read())
    finally:
        if process is not None:
            if process.poll() is None:
                process.stdin.close()
                process.wait(timeout=10)
            for pipe in (process.stdin, process.stdout, process.stderr):
                pipe.close()

    parent = (directory / f"pid.{pid}").read_text().splitlines()
    spawned = {
        int(m[1])
        for s in parent
        if (m := re.match(r"(?:clone3?|fork|vfork)\(.*\)\s+= (\d+)$", s))
    }
    waited = {
        int(m[1])
        for s in parent
        if (m := re.match(r"wait4\((\d+),.*WEXITSTATUS\(s\) == 0.*\)\s+= \1$", s))
    }
    if len(spawned) != 15 or spawned != waited:
        raise AssertionError(
            ("distinct native launches and reaps", len(spawned), len(waited))
        )
    for child in spawned:
        if not any(
            s.startswith("execve(" + json.dumps(sys.executable) + ",")
            and s.endswith("= 0")
            for s in (directory / f"pid.{child}").read_text().splitlines()
        ):
            raise AssertionError(("missing upstream data-child exec", child))
    selector_calls = Counter(
        m[1] for s in parent if (m := re.match(r"(poll|ppoll|select|pselect6)\(", s))
    )
    allowed = (
        {"select", "pselect6"} if selector == "SelectSelector" else {"poll", "ppoll"}
    )
    if not selector_calls or selector_calls.keys() - allowed:
        raise AssertionError(("wrong native selector", selector_calls))
    return dict(
        roles=dict(
            body="cpython", assertions="cpython", operation="imre", fixture="cpython"
        ),
        fixture_selector=selector,
        native_parent_sha256=digest(binary),
        native_parents=1,
        open_file_limit=len(baseline) + 10,
        free_slots=10,
        pressure_fds=len(pressure),
        original_descriptor_numbers_and_cloexec=True,
        unchanged_fd_table_after_each_communicate=reaped,
        native_creations=len(spawned),
        native_reaps=len(waited),
        data_child="CPython stdin/stdout echo",
        selector_calls=dict(selector_calls),
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as tmp:

        def execute(case, original):
            directory = Path(tmp) / type(case).__name__
            directory.mkdir()
            body = directory / "body"
            body.mkdir()
            try:
                owner, preamble = fixture(case, body)
                run = native_method(tc, body, SOURCE, owner, case.id(), preamble)
            except (RuntimeError, ValueError) as exc:
                attempt = dict(
                    status="blocked",
                    reason=str(exc)
                    .replace(str(body), "<case>")
                    .strip()
                    .splitlines()[-1],
                )
            else:
                attempt = dict(status="completed", **run(case, original))
            binding = (
                "subprocess._PopenSelector = selectors.SelectSelector"
                if type(case).__name__.endswith("NoPoll")
                else ""
            )
            binary = tc.build(ACTOR.replace("SELECTOR", binding).encode(), directory)
            result = exercise(binary, directory, case, original)
            result["body_attempt"] = attempt
            return result

        rows, failed, skipped = run_in_fixtures(
            load_suite(manifest["module"]), dict.fromkeys(IDS, execute)
        )
        if rows.keys() != IDS or failed or skipped:
            raise AssertionError((failed, skipped))
    args.result.write_text(
        json.dumps(
            dict(
                cpython_commit=manifest["commit"],
                opt_level=tc.level,
                toolchain={
                    n: digest(getattr(args, n)) for n in ("imre", "runtime", "clang")
                },
                rows=rows,
            ),
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
