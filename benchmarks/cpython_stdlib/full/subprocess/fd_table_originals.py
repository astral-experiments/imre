"""Transfer original fixture fds into Imre parents and observe the child's fd table."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import select
import subprocess
import sys
import tempfile
import warnings
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

HERE = Path(__file__).resolve().parent

IDS = {
    f"test.test_subprocess.POSIXProcessTestCase.{name}"
    for name in (
        "test_pass_fds",
        "test_close_fds",
        "test_pass_fds_inheritable",
        "test_pass_fds_redirected",
    )
}


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def table(pid):
    answer = {}
    for p in Path(f"/proc/{pid}/fd").iterdir():
        try:
            target = str(p.readlink())
            info = Path(f"/proc/{pid}/fdinfo/{p.name}").read_text()
        except FileNotFoundError:
            continue
        flags = int(
            next(s for s in info.splitlines() if s.startswith("flags:")).split()[1], 8
        )
        answer[int(p.name)] = (target, bool(flags & os.O_CLOEXEC))
    return answer


def read(stream):
    if not select.select([stream], [], [], 30)[0]:
        raise AssertionError("native parent timed out")
    line = stream.readline()
    if not line:
        raise AssertionError("native parent exited before replying")
    return line.rstrip(b"\n")


def program(options, inherited, command=None):
    # The compiler knows which return slots are pipes. Each actual fixture fd
    # is still installed by the transport and verified in the target process.
    source = ["import os", "import subprocess", "import sys"]
    if command is not None:
        source.append("import warnings")
    for fd, (_, cloexec) in sorted(inherited.items()):
        source.append(f"os.set_inheritable({fd}, {not cloexec!r})")
    params = []
    for name, value in options.items():
        if name == "pass_fds":
            if type(value) is set and all(type(fd) is int for fd in value):
                expr = "{" + ",".join(map(str, sorted(value))) + "}"
            elif type(value) in (tuple, list) and all(type(fd) is int for fd in value):
                expr = repr(value)
            else:
                raise AssertionError(("unexpected keep collection", value))
        elif name == "close_fds" and type(value) is bool:
            expr = repr(value)
        elif name in ("stdin", "stdout", "stderr") and type(value) is int:
            expr = "subprocess.PIPE" if value == subprocess.PIPE else str(value)
        else:
            raise AssertionError(("unexpected original option", name, value))
        params.append(name + "=" + expr)
    if command is not None:
        if type(command) is not tuple or not command:
            raise AssertionError(command)
        # Preserve the exact argument tuple used by the host module fixture.
        args = (
            "("
            + "".join(f"sys.argv[{index}]," for index in range(1, len(command) + 1))
            + ")"
        )
        source.extend(
            [
                'warnings.simplefilter("always", RuntimeWarning)',
                'print("ready")',
                'assert input()=="go"',
                "result=subprocess.call(" + args + ", " + ", ".join(params) + ")",
                'print("returned")',
                'assert input()=="result"',
                "print(result)",
                'print("reaped")',
                'assert input()=="release"',
            ]
        )
        return ("\n".join(source) + "\n").encode()
    source.extend(
        [
            'print("ready")',
            'assert input()=="go"',
            "args=sys.argv[1:]",
            "with subprocess.Popen(args, " + ", ".join(params) + ") as child:",
            '    print("spawned")',
            "    request=input()",
        ]
    )
    if options.get("stdout") == subprocess.PIPE:
        source.extend(
            [
                '    assert request=="communicate"',
                "    out, err = child.communicate()",
                "    print(repr(out))",
                "    print(err is None)",
            ]
        )
    else:
        source.append('    assert request=="exit"')
    source.extend(['print("reaped")', 'assert input()=="release"'])
    return ("\n".join(source) + "\n").encode()


def exercise(tc, directory, case, original, forward_warning=warnings.warn_explicit):
    from test import test_subprocess as upstream

    base = table("self")
    # First attempt the full original body under the shared fixture.
    try:
        owner, pre = fixture(case, directory)
        native = native_method(tc, directory, SOURCE, owner, case.id(), pre)
    except (ValueError, RuntimeError) as exc:
        attempt = {
            "status": "blocked",
            "reason": str(exc)
            .replace(str(directory), "<case>")
            .strip()
            .splitlines()[-1],
        }
    else:
        attempt = {"status": "completed", **native(case, original)}
    actors = []
    controls = set()

    class Native:
        def __init__(self, args, *, original_call=False, **options):
            self.original_call = original_call
            self.warning = None
            if type(args) is not (tuple if original_call else list) or not all(
                type(s) is str for s in args
            ):
                raise AssertionError(args)
            self.directory = directory / str(len(actors))
            self.directory.mkdir()
            host = table("self")
            self.inherited = {
                fd: v for fd, v in host.items() if fd not in base and fd not in controls
            }
            if not self.inherited:
                raise AssertionError(
                    "the original fixture did not open any descriptors"
                )
            keep = options.get("pass_fds", ())
            if set(keep) - self.inherited.keys():
                raise AssertionError(("unknown keep descriptors", keep))
            self.options = options
            self.args = args[:]
            self.binary = tc.build(
                program(options, self.inherited, args if original_call else None),
                self.directory,
            )
            self.actor = subprocess.Popen(
                [
                    "strace",
                    "-ff",
                    "-qq",
                    "-s",
                    "1024",
                    "-e",
                    "signal=none",
                    "-e",
                    "trace=execve,fcntl,close_range,clone,clone3,fork,vfork,wait4,poll,ppoll,select,pselect6,write,writev",
                    "-o",
                    self.directory / "pid",
                    "--",
                    self.binary,
                    *args,
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                pass_fds=tuple(self.inherited),
                bufsize=0,
            )
            self.control = {
                p.fileno()
                for p in (self.actor.stdin, self.actor.stdout, self.actor.stderr)
            }
            controls.update(self.control)
            actors.append(self)
            self.finished = False
            if read(self.actor.stdout) != b"ready":
                raise AssertionError("missing fixture handshake")
            [self.pid] = map(
                int,
                Path(f"/proc/{self.actor.pid}/task/{self.actor.pid}/children")
                .read_text()
                .split(),
            )
            if not Path(f"/proc/{self.pid}/exe").samefile(self.binary):
                raise AssertionError("wrong parent")
            self.before = table(self.pid)
            if {fd: v for fd, v in self.before.items() if fd >= 3} != self.inherited:
                raise AssertionError(
                    (
                        "fd transfer did not preserve numbers, targets and CLOEXEC",
                        self.inherited,
                        self.before,
                    )
                )
            self.actor.stdin.write(b"go\n")
            if read(self.actor.stdout) != (
                b"returned" if original_call else b"spawned"
            ):
                raise AssertionError("Popen did not return")

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            self.finish("exit")

        def communicate(self):
            return self.finish("communicate")

        def finish(self, request):
            self.actor.stdin.write(request.encode() + b"\n")
            out = None
            if request == "communicate":
                out = ast.literal_eval(read(self.actor.stdout).decode())
                if read(self.actor.stdout) != b"True":
                    raise AssertionError("unexpected stderr pipe")
            if request == "result":
                out = int(read(self.actor.stdout))
            if read(self.actor.stdout) != b"reaped":
                raise AssertionError("context did not reap")
            if table(self.pid) != self.before:
                raise AssertionError(
                    "Popen changed a borrowed parent fd or descriptor flag"
                )
            self.actor.stdin.write(b"release\n")
            status = self.actor.wait(timeout=15)
            stderr = self.actor.stderr.read()
            if status:
                raise AssertionError(("native parent failed", status, stderr))
            if self.original_call:
                recognizable = re.fullmatch(
                    rb"(subprocess\.py):(\d+): (RuntimeWarning): ([^\n]+)\n", stderr
                )
                if recognizable is None:
                    raise AssertionError(
                        ("native parent did not emit its warning", stderr)
                    )
                filename, line, category, text = recognizable.groups()
                self.warning = dict(
                    file=filename.decode(),
                    line=int(line),
                    category=category.decode(),
                    message=text.decode(),
                )
                forward_warning(
                    text.decode(), RuntimeWarning, filename.decode(), int(line)
                )
            elif stderr:
                raise AssertionError(("unexpected Popen stderr", stderr))
            self.finished = True
            self.close()
            return out, None

        def close(self):
            if self.actor.poll() is None:
                self.actor.stdin.close()
                self.actor.wait(timeout=15)
            for p in (self.actor.stdin, self.actor.stdout, self.actor.stderr):
                p.close()
            controls.difference_update(self.control)

        def evidence(self):
            if not self.finished:
                raise AssertionError("not reaped")
            traces = {
                int(f.suffix[1:]): f.read_text().splitlines()
                for f in self.directory.glob("pid.*")
            }
            parent = traces[self.pid]
            [child] = [
                pid
                for pid, lines in traces.items()
                if pid != self.pid
                and any(
                    line.startswith("execve(" + json.dumps(self.args[0]) + ",")
                    and line.endswith("= 0")
                    for line in lines
                )
            ]
            if not any(
                re.match(r"(?:clone3?|vfork|fork)\(", s)
                and re.search(rf"= {child}$", s)
                for s in parent
            ):
                raise AssertionError(
                    "native parent not observed creating its data child"
                )
            if not any(
                s.startswith(f"wait4({child},")
                and "WEXITSTATUS(s) == 0" in s
                and re.search(rf"= {child}$", s)
                for s in parent
            ):
                raise AssertionError("native parent did not reap the fd-status child")
            keep = sorted(self.options.get("pass_fds", ()))
            expected_close = self.options.get("close_fds", True) or bool(keep)
            before_exec = []
            for line in traces[child]:
                if line.startswith("execve("):
                    break
                before_exec.append(line)
            marked = any(
                line.startswith("close_range(3,")
                and "CLOSE_RANGE_CLOEXEC" in line
                and line.endswith("= 0")
                for line in before_exec
            )
            # Linux on this devbox supports close_range, and the default
            # inherited/close/keep cases must pick different kernel paths.
            if marked != expected_close:
                raise AssertionError(
                    ("wrong bulk CLOEXEC in fork child", marked, expected_close)
                )
            for fd in keep:
                if fd >= 3 and not any(
                    line.startswith(f"fcntl({fd}, F_SETFD, 0)") for line in before_exec
                ):
                    raise AssertionError(("missing child-only CLOEXEC removal", fd))
            if self.original_call:
                [warned] = [
                    i
                    for i, s in enumerate(parent)
                    if re.match(r"writev?\(2,", s)
                    and "RuntimeWarning: pass_fds overriding close_fds." in s
                ]
                [created] = [
                    i
                    for i, s in enumerate(parent)
                    if re.match(r"(?:clone3?|vfork|fork)\(", s)
                    and s.endswith(f"= {child}")
                ]
                if warned >= created:
                    raise AssertionError(
                        "warning did not precede the native child launch"
                    )
                self.warning["before_child_creation"] = True
            return dict(
                native_parent_sha256=digest(self.binary),
                original_open_fds=len(self.inherited),
                original_cloexec_fds=sum(
                    cloexec for _, cloexec in self.inherited.values()
                ),
                close_fds=self.options.get("close_fds", True),
                pass_fds_collection=type(self.options["pass_fds"]).__name__
                if "pass_fds" in self.options
                else None,
                kept_descriptors=len(keep),
                kept_numbers=keep,
                data_executable=self.args[0],
                borrowed_stdio=sum(
                    name in self.options and self.options[name] >= 0
                    for name in ("stdin", "stdout", "stderr")
                ),
                child_bulk_cloexec=marked,
                parent_table_unchanged=True,
                data_child="original ZERO_RETURN_CMD"
                if self.original_call
                else "original CPython fd_status.py",
                native_warning=self.warning,
                argument_type=type(self.args).__name__,
                native_reaps=1,
            )

    def native_call(args, **options):
        if args is not upstream.ZERO_RETURN_CMD:
            raise AssertionError(
                "the test did not pass the module fixture's original command"
            )
        if warnings.filters[0] != ("always", None, RuntimeWarning, None, 0):
            raise AssertionError(
                ("unexpected original assertWarns filter", warnings.filters[0])
            )
        actor = Native(args, original_call=True, **options)
        code, _ = actor.finish("result")
        return code

    try:
        with patch.object(
            upstream,
            "subprocess",
            SimpleNamespace(Popen=Native, call=native_call, PIPE=subprocess.PIPE),
        ):
            original()
        if not actors:
            raise AssertionError("no native launch")
        if case._testMethodName == "test_pass_fds":
            if len(actors) != 20 or sum(a.original_call for a in actors) != 10:
                raise AssertionError("not every descriptor reached both native APIs")
            if [a.options["pass_fds"] for a in actors[::2]] != [
                a.options["pass_fds"] for a in actors[1::2]
            ]:
                raise AssertionError(
                    "each original descriptor must reach both native calls"
                )
        return dict(
            body_attempt=attempt,
            roles=dict(
                body="cpython",
                assertions="cpython",
                fixture="cpython",
                operation="imre",
            ),
            launches=[a.evidence() for a in actors],
        )
    finally:
        for actor in actors:
            if not actor.finished:
                actor.close()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for field in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + field, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    p.add_argument(
        "--only",
        choices=sorted(IDS),
        action="append",
        help="Select original IDs within this driver",
    )
    p.add_argument(
        "--withhold-warning",
        action="store_true",
        help="Withhold the captured native warning to exercise the original assertion",
    )
    args = p.parse_args()
    selected = set(args.only or IDS)
    warning_id = "test.test_subprocess.POSIXProcessTestCase.test_pass_fds"
    if args.withhold_warning and selected != {warning_id}:
        p.error("--withhold-warning requires --only " + warning_id)
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):
        withheld = []

        def capture(text, category, filename, lineno):
            withheld.append(
                dict(
                    message=text, category=category.__name__, file=filename, line=lineno
                )
            )

        def execute(case, original):
            directory = Path(tmp) / case._testMethodName
            directory.mkdir()
            return exercise(
                tc,
                directory,
                case,
                original,
                capture if args.withhold_warning else warnings.warn_explicit,
            )

        rows, failed, skipped = run_in_fixtures(
            load_suite(manifest["module"]), dict.fromkeys(selected, execute)
        )
        control = {}
        if args.withhold_warning:
            if rows or skipped or len(failed) != 1 or failed[0][0] != warning_id:
                raise AssertionError((rows, failed, skipped))
            diagnostic = failed[0][1].strip().splitlines()[-1]
            if (
                diagnostic != "AssertionError: RuntimeWarning not triggered"
                or len(withheld) != 1
            ):
                raise AssertionError((diagnostic, withheld))
            control["withheld_warning"] = dict(
                original_failure=diagnostic, native_warning=withheld[0]
            )
        elif rows.keys() != selected or failed or skipped:
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
                **control,
            ),
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
