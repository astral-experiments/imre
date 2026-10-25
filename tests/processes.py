"""Compare sealed synchronous processes, deadline byte snapshots, and ownership."""

import argparse
import fcntl
import os
import re
import resource
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions")

sys.path.insert(0, str(Path(__file__).resolve().parent))
import files

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "processes"
IMPORTS = """import os
import subprocess
from subprocess import run, CompletedProcess, CalledProcessError, SubprocessError, TimeoutExpired
from pathlib import Path
"""
CALL = "run(['command'], capture_output=True, encoding='utf-8')"
REJECTED = {
    "rebound_async_capture": (
        "import asyncio\nasync def outer() -> int:\n"
        "    value = 1\n    async def read() -> int:\n        return value\n"
        "    future = read()\n    value = 2\n    return await future\n"
        "print(asyncio.run(outer()))"
    ),
    "missing_args": "run(capture_output=True, encoding='utf-8')",
    "duplicate_args": "run(['command'], args=['other'], capture_output=True, encoding='utf-8')",
    "extra_positional": "run(['command'], True, capture_output=True, encoding='utf-8')",
    "integer_args": "run([1], capture_output=True, encoding='utf-8')",
    "optional_args": "args: list[str] | None = None\nrun(args, capture_output=True, encoding='utf-8')",
    "expanded_args": "run(*[['command']], capture_output=True, encoding='utf-8')",
    "expanded_kwargs": "run(['command'], capture_output=True, encoding='utf-8', **{'check': True})",
    "dynamic_capture": "capture = True\nrun(['command'], capture_output=capture, encoding='utf-8')",
    "other_encoding": "run(['command'], capture_output=True, encoding='latin-1')",
    "dynamic_encoding": "encoding = 'utf-8'\nrun(['command'], capture_output=True, encoding=encoding)",
    "dynamic_text": "text = True\nrun(['command'], capture_output=True, encoding='utf-8', text=text)",
    "wrong_check": "run(['command'], capture_output=True, encoding='utf-8', check=1)",
    "wrong_env": "run(['command'], capture_output=True, encoding='utf-8', env={'KEY': 1})",
    "wrong_env_key": "run(['command'], capture_output=True, encoding='utf-8', env={1: 'value'})",
    "raw_environ": "run(['command'], capture_output=True, encoding='utf-8', env=os.environ)",
    "environ_index": "print(os.environ['KEY'])",
    "environ_assignment": "os.environ['KEY'] = 'value'",
    "environ_copy_args": "os.environ.copy(1)",
    "environ_wrong_key": "print(os.getenv(1))",
    "stored_run": "callback = run\ncallback(['command'])",
    "construct_result": "CompletedProcess(['command'], 0, '', '')",
    "construct_failure": "CalledProcessError(1, ['command'])",
    "construct_parent_error": "SubprocessError('message')",
    "construct_timeout": "TimeoutExpired(['command'], 1.0)",
    "raise_timeout_class": "raise TimeoutExpired",
    "timeout_annotation": "def seconds(error: TimeoutExpired) -> float:\n    return error.timeout",
    "mixed_timeout_field": "try:\n    run(['command'], capture_output=True, text=True, timeout=1)\n    run(['command'], capture_output=True, text=True, timeout=1.0)\nexcept TimeoutExpired as error:\n    print(error.timeout)",
    "boolean_timeout": "run(['command'], capture_output=True, encoding='utf-8', timeout=True)",
    "string_timeout": "run(['command'], capture_output=True, encoding='utf-8', timeout='1.0')",
    "optional_integer_timeout": "def launch(duration: int | None) -> None:\n    run(['command'], capture_output=True, encoding='utf-8', timeout=duration)",
    "timeout_text_projection": "try:\n    " + CALL + "\nexcept TimeoutExpired as error:\n    text: str = error.stdout",
    "timed_base_text_projection": "try:\n    run(['command'], capture_output=True, text=True, timeout=1.0, check=True)\nexcept SubprocessError as error:\n    text: str = error.stdout",
    "timeout_returncode": "try:\n    " + CALL + "\nexcept TimeoutExpired as error:\n    print(error.returncode)",
    "write_timeout_field": "try:\n    " + CALL + "\nexcept TimeoutExpired as error:\n    error.timeout = 1.0",
    "failure_annotation": "def code(error: CalledProcessError) -> int:\n    return error.returncode",
    "bytes_result": "result: CompletedProcess[bytes] = " + CALL,
    "extra_type_arguments": "result: CompletedProcess[str, int] = " + CALL,
    "unknown_result_attribute": "result = " + CALL + "\nprint(result.pid)",
    "check_method_args": "result = " + CALL + "\nresult.check_returncode(1)",
    "stored_check_method": "result = " + CALL + "\ncallback = result.check_returncode\ncallback()",
    "optional_result": "result: CompletedProcess[str] | None = None\nprint(result.stdout)",
    "result_subclass": "class Custom(CompletedProcess):\n    pass",
}
for name, value in {"args": "['other']", "returncode": "0", "stdout": "'text'", "stderr": "'text'"}.items():
    REJECTED[f"write_{name}"] = f"result = {CALL}\nresult.{name} = {value}"
REJECTED["write_error_field"] = "error: Exception = ValueError('message')\nerror.stdout = 'text'"

REDIRECT = """from sys import stderr
def redirect() -> None:
    run(['command'], stdout=stderr, check=True)
"""
REJECTED["indirect_error_stream"] = REDIRECT + """try:
    redirect()
except CalledProcessError as error:
    print(len(error.stderr))
"""
REJECTED["rebound_error_stream"] = REDIRECT + """try:
    redirect()
except CalledProcessError as redirected:
    try:
        run(['command'], capture_output=True, text=True, check=True)
    except CalledProcessError as captured:
        captured = redirected
        print(len(captured.stderr))
"""
REJECTED["reraised_error_stream"] = REDIRECT + """try:
    redirect()
except CalledProcessError:
    try:
        raise
    except CalledProcessError as again:
        print(len(again.output))
"""

INVALID_ENV = """import os
import subprocess
from sys import argv
for turn in range(2):
    try:
        os.environ.copy()
    except UnicodeDecodeError:
        print('invalid environment')
result = subprocess.run([argv[1], '-I', '-c', "print('recovered')"], capture_output=True, encoding='utf-8', env={})
print(result.stdout)
"""


def seed(directory: Path, python: str) -> None:
    directory.mkdir(parents=True)
    (directory / "sentinel.txt").write_bytes(b"preserve this file")
    (directory / "plain-file").write_bytes(b"plain file")
    (directory / "not-executable").write_bytes(b"#!/bin/sh\nexit 0\n")
    (directory / "not-executable").chmod(0o644)
    (directory / "executable-text").write_bytes(b"printf unexpected > fallback.txt\n")
    (directory / "executable-text").chmod(0o755)
    (directory / "directory" / "bin").mkdir(parents=True)
    for name, status in (("exit-zero", 0), ("exit-one", 1)):
        helper = directory / "directory" / name
        helper.write_text(f"#!/bin/sh\nexit {status}\n")
        helper.chmod(0o755)
    (directory / "directory" / "bin" / "imre-process-child").symlink_to(python)
    (directory / "directory" / "relative-executable").symlink_to(python)


def snapshot(directory: Path) -> dict:
    """Compare exact file contents without following executable symlinks."""
    result = {}
    for parent, directories, names in os.walk(directory, followlinks=False):
        for name in directories + names:
            path = Path(parent) / name
            mode = path.lstat().st_mode
            value = ("link", os.readlink(path)) if stat.S_ISLNK(mode) else (
                ("directory", None) if stat.S_ISDIR(mode) else ("file", path.read_bytes())
            )
            result[str(path.relative_to(directory))] = value
    return result


def pipe_memory_limit() -> None:
    # Keep the explicit 1 GiB buffer larger than the complete process.
    resource.setrlimit(resource.RLIMIT_AS, (128 << 20, 128 << 20))


def run(
    command: list[str],
    directory: Path,
    *,
    descriptor: int | None = None,
    limited: bool = False,
    memory_limited: bool = False,
    empty_name: bool = False,
    invalid: tuple[bytes, bytes] | None = None,
    high_fds: bool = False,
):
    environment = files.environment()
    environment.update(IMRE_PROCESS_PARENT="inherited λ", IMRE_PROCESS_OVERRIDE="parent")
    if empty_name:
        environment[""] = "inherited empty name λ"
    else:
        environment.pop("", None)
    environment.pop("IMRE_PROCESS_EMPTY", None)
    if invalid is not None:
        environment = {os.fsencode(key): os.fsencode(value) for key, value in environment.items()}
        environment[invalid[0]] = invalid[1]
    if high_fds:
        # Fill the target parent's descriptor table across exec, without
        # changing the test runner's own soft limit or open files.
        prepare = """import os, resource, sys
soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
if soft != resource.RLIM_INFINITY and soft < 1060:
    resource.setrlimit(resource.RLIMIT_NOFILE, (1060, hard))
while True:
    fd = os.open(os.devnull, os.O_RDONLY)
    os.set_inheritable(fd, True)
    if fd >= 1030:
        break
os.execv(sys.argv[1], sys.argv[1:])
"""
        command = [sys.executable, "-c", prepare, *command]
    return subprocess.run(
        command,
        cwd=directory,
        input="from parent\r\néλ🙂\rfinal\n".encode(),
        capture_output=True,
        timeout=60,
        check=False,
        env=environment,
        pass_fds=() if descriptor is None else (descriptor,),
        preexec_fn=pipe_memory_limit if memory_limited else files.descriptor_limit if limited else None,
    )


def unobserved_children(result):
    """Match emitted warning PIDs to each operation; PIDs and source filenames vary."""
    assert result.returncode == 0, result
    lines = result.stdout.splitlines()
    assert lines[-1] == b"invalid action: 'invented'", result
    pids = dict(zip(lines[:-1:2], lines[1:-1:2], strict=True))
    assert set(pids) == {b"default", b"live", b"exited", b"waited", b"polled", b"other-category", b"ignored"}
    emitted = re.findall(rb"ResourceWarning: subprocess (\d+) is still running", result.stderr)
    assert emitted == [pids[key] for key in (b"live", b"exited", b"other-category")], result
    assert not re.search(rb"(?:Error|Sanitizer|panicked):", result.stderr), result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--opt-levels", nargs="+", choices=["0", "1", "2", "3"], default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    assert len(set(args.opt_levels)) == len(args.opt_levels)
    compiler = str(args.compiler.resolve())
    flags = ["--quiet", "--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    resolved = files.run([args.python, "-I", "-c", "import sys; print(sys.executable)"], FIXTURES)
    files.check_result(resolved, None, "CPython interpreter")
    python = str(Path(resolved.stdout.decode().strip()).resolve())
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-processes-") as directory, open(os.devnull, "rb") as original:
        temporary = Path(directory)
        # Preserve an inherited descriptor above the child's deliberately lowered
        # soft limit. Closing only up to RLIMIT_NOFILE would leave this one open.
        with os.fdopen(fcntl.fcntl(original.fileno(), fcntl.F_DUPFD_CLOEXEC, 211), "rb") as inherited:
            descriptor = inherited.fileno()
            signal_child = temporary / "signal-child"
            built = files.run([args.clang, str(FIXTURES / "signal.c"), "-o", str(signal_child)], temporary)
            files.check_result(built, None, "signal probe build")
            arguments = [python, str(FIXTURES / "child.py"), str(descriptor), str(signal_child)]
            sources = [source for source in sorted(FIXTURES.glob("*.py")) if source.name != "child.py"]
            sources.append(FIXTURES / "imported" / "main.py")
            for source in sources:
                name = source.stem if source.parent == FIXTURES else "imported"
                # ASan reserves more address space than this test can allow.
                if name == "pipe_buffer_allocation" and (args.sanitize or sys.platform != "linux"):
                    continue
                project = temporary / name
                reference_dir = project / "cpython" / "work"
                seed(reference_dir, python)
                options = {
                    "memory_limited": name == "pipe_buffer_allocation",
                    "limited": name in {"cleanup", "stdin_fds", "timeouts"},
                    "descriptor": descriptor if name == "stdin_fds" else None,
                    "empty_name": name == "environment",
                    "high_fds": name == "select_high_fds",
                }
                reference = run([python, "-E", "-B", str(source), *arguments], reference_dir, **options)
                if name == "unobserved_child":
                    unobserved_children(reference)
                elif name == "check_output":
                    assert reference.returncode == 0 and reference.stderr == b"inherited", reference
                else:
                    files.check_result(reference, None, f"CPython {name}")
                expected_files = snapshot(reference_dir)
                if name == "pipe_buffer_allocation":
                    assert reference.stdout == b"stdin: MemoryError()\nstdout: MemoryError()\n"
                    assert "child-ran" not in expected_files
                if name == "stdin_fds":
                    assert b"closed\n" in reference.stdout and reference.stdout.endswith(b"-13\n"), reference.stdout
                for level in args.opt_levels:
                    binary = project / f"native-O{level}"
                    built = files.run([compiler, "build", str(source), "--module-root", str(source.parent), "-o", str(binary), "--opt-level", level, *flags], temporary)
                    files.check_result(built, None, f"build {name} O{level}")
                    native_dir = project / f"imre-O{level}" / "work"
                    seed(native_dir, python)
                    actual = run([str(binary), *arguments], native_dir, **options)
                    if name == "unobserved_child":
                        unobserved_children(actual)
                    elif name == "check_output":
                        assert actual.returncode == 0 and actual.stderr == reference.stderr, actual
                    else:
                        files.check_result(actual, None, f"Imre {name} O{level}")
                    if name != "unobserved_child":
                        assert actual.stdout == reference.stdout, (name, level, actual.stdout, reference.stdout)
                    assert snapshot(native_dir) == expected_files, (name, level, "filesystem effects differ")
                    checks += 1
        for name, source in REJECTED.items():
            path = temporary / f"reject-{name}.py"
            path.write_text(IMPORTS + source + "\n", encoding="utf-8")
            result = files.run([compiler, "check", str(path)], temporary)
            assert result.returncode > 0 and result.stderr, (name, result)
            assert b"panicked" not in result.stderr and b"Sanitizer" not in result.stderr, (name, result.stderr)
            if name in {"boolean_timeout", "optional_integer_timeout"}:
                assert b"integer literal or float | None" in result.stderr, (name, result.stderr)
            if name == "mixed_timeout_field":
                assert b"both int and float deadlines" in result.stderr, (name, result.stderr)
            checks += 1
        for module in ["subprocess", "os"]:
            shadow = temporary / f"shadow-{module}"
            shadow.mkdir()
            (shadow / f"{module}.py").write_text("def value() -> int:\n    return 1\n")
            (shadow / "main.py").write_text(f"import {module}\nprint({module}.value())\n")
            result = files.run([compiler, "check", str(shadow / "main.py")], temporary)
            assert result.returncode > 0 and b"standard-library" in result.stderr, result.stderr
            checks += 1
        source = temporary / "invalid_environment.py"
        source.write_text(INVALID_ENV)
        for level in args.opt_levels:
            binary = temporary / f"invalid-environment-O{level}"
            built = files.run([compiler, "build", str(source), "-o", str(binary), "--opt-level", level, *flags], temporary)
            files.check_result(built, None, "build invalid environment boundary")
            for invalid in [(b"IMRE_INVALID", b"\xff"), (b"IMRE_\xff", b"value")]:
                actual = run([str(binary), python], temporary, invalid=invalid)
                files.check_result(actual, None, "invalid environment boundary")
                assert actual.stdout == b"invalid environment\ninvalid environment\nrecovered\n\n", actual.stdout
                checks += 1
    print(f"Passed {checks} process checks ({len(REJECTED) + 2} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})")


if __name__ == "__main__":
    main()
