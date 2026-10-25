"""Compare output and interactive I/O behavior with CPython, without timing claims."""

import argparse
import errno
import os
import pty
import selectors
import signal
import subprocess
import sys
import tempfile
import termios
import time
from contextlib import suppress
from pathlib import Path

PROGRAMS = {
    "small": '''print("small output")
print("")
print("caf\u00e9 \u03bb \U0001f642 \\0 tail")
print(True)
print(False)
print(None)
print(0)
print(-17)
print(1000000000000000000000000000000000000000000000000000000000001)
''',
    # Exceed the 128 KiB binary buffer CPython 3.14 uses for a pipe.
    "large": "print(" + repr("prefix\0" + "éλ🙂" * 32768 + "\0suffix") + ")\n"
    'for index in range(160):\n    print(index)\nprint("large output finished")\n',
    "blocked_write": (Path(__file__).parent / "fixtures/sys/blocked_write.py").read_text(encoding="utf-8"),
    "print_errno": """import os
import sys
for fd in [1, 2]:
    saved = os.dup(fd)
    os.dup2(int(sys.argv[1]), fd)
    try:
        if fd == 1:
            print("x" * 131072)
        else:
            print("x", file=sys.stderr)
    except OSError as error:
        code = repr(error.errno)
    else:
        code = "missing error"
    finally:
        os.dup2(saved, fd)
        os.close(saved)
    print(code)
""",
    "division_error": '''print("before division error")
print(1000000000000000000000000000000000000000000000000000000000001)
zero = int("0")
print(1 // zero)
''',
    "input_eof": 'print("before input EOF")\ninput()\n',
    "pipe_input": 'print("input-ready")\nanswer = input()\nprint(answer)\nprint("input-finished")\n',
    "terminal_loop": 'print("terminal-ready")\nwhile True:\n    pass\n',
}
ERRORS = {"division_error": b"ZeroDivisionError", "input_eof": b"EOFError"}
PAYLOADS = {
    "LF": "pipe reply: π\0tail\n".encode(),
    "CRLF": "pipe reply: π\0tail\r\n".encode(),
}


def environment() -> dict[str, str]:
    return {**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"}


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
        check=False,
        env=environment(),
        **kwargs,
    )


def require_success(result: subprocess.CompletedProcess[bytes], label: str) -> None:
    assert result.returncode == 0 and not result.stderr, (
        label,
        result.returncode,
        result.stderr[:2000],
    )


def same_output(actual: bytes, expected: bytes, label: str) -> None:
    if actual != expected:
        offset = next(
            (index for index, pair in enumerate(zip(actual, expected)) if pair[0] != pair[1]),
            min(len(actual), len(expected)),
        )
        raise AssertionError(
            f"{label}: stdout differs at byte {offset}; "
            f"actual={actual[offset:offset + 120]!r}, expected={expected[offset:offset + 120]!r}; "
            f"lengths={len(actual)}/{len(expected)}"
        )


def blocked_write(command: list[str], free: int, mode: str) -> subprocess.CompletedProcess[bytes]:
    reader, writer = os.pipe()
    try:
        os.set_blocking(writer, False)
        for chunk in (b"x" * 4096, b"x"):
            try:
                while True:
                    os.write(writer, chunk)
            except BlockingIOError:
                pass
        if free:
            os.read(reader, free)
        return run([*command, str(writer), mode], pass_fds=(writer,))
    finally:
        os.close(reader)
        os.close(writer)


def visible_output(fd: int, expected: bytes, timeout: float) -> bytes:
    """Wait for a protocol message, without assuming a buffer size or write count."""
    output = bytearray()
    deadline = time.monotonic() + timeout
    with selectors.DefaultSelector() as selector:
        selector.register(fd, selectors.EVENT_READ)
        while len(output) < len(expected):
            remaining = deadline - time.monotonic()
            assert remaining > 0 and selector.select(remaining), (
                "output was not visible before the next program action",
                bytes(output),
                expected,
            )
            try:
                part = os.read(fd, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                part = b""  # Linux PTYs report EIO when their last slave closes.
            assert part, ("child closed its output before the message", bytes(output))
            output.extend(part)
    same_output(bytes(output), expected, "interactive message")
    return bytes(output)


def stop(process: subprocess.Popen[bytes]) -> tuple[bytes | None, bytes | None]:
    """Always kill and reap an interactive child, including failed assertions."""
    if process.poll() is None:
        process.kill()
    return process.communicate(timeout=10)


def pipe_exchange(command: list[str], payload: bytes, timeout: float) -> bytes:
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=0,
        env=environment(),
    )
    try:
        assert process.stdout is not None
        prefix = visible_output(process.stdout.fileno(), b"input-ready\n", timeout)
        assert process.poll() is None, "child exited before receiving input"
        rest, error = process.communicate(input=payload, timeout=timeout)
        assert process.returncode == 0 and not error, (
            "input handshake",
            process.returncode,
            error[:2000],
        )
        return prefix + rest
    finally:
        stop(process)


def terminal_message(command: list[str], timeout: float) -> bytes:
    master, slave = pty.openpty()
    process = None
    try:
        attributes = termios.tcgetattr(slave)
        attributes[1] &= ~termios.OPOST  # Compare LF bytes without terminal CRLF translation.
        termios.tcsetattr(slave, termios.TCSANOW, attributes)
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=slave,
            stderr=subprocess.PIPE,
            env=environment(),
        )
        os.close(slave)
        slave = -1
        output = visible_output(master, b"terminal-ready\n", timeout)
        assert process.poll() is None, "busy-loop child exited unexpectedly"
    finally:
        if process is not None:
            _, error = stop(process)
        if slave >= 0:
            os.close(slave)
        os.close(master)
    assert not error, ("terminal child stderr", error[:2000])
    return output


def check_link_options(directory: Path, compiler: str, runtime: str) -> int:
    source = directory / "link.py"
    source.write_text("print(1)\n")
    driver = directory / "record clang args"
    driver.write_text("#!/bin/sh\nprintf '%s\\0' \"$@\"\n")
    driver.chmod(0o755)
    base = [compiler, "build", str(source), "--runtime", runtime, "--clang", str(driver)]
    clean_env = os.environ.copy()
    clean_env.pop("IMREFLAGS", None)
    clean_env.pop("IMRE_ENCODED_FLAGS", None)

    def link(*flags: str, **env: str) -> list[bytes]:
        result = subprocess.run(
            [*base, *flags], cwd=directory, env=clean_env | env,
            capture_output=True, check=True, timeout=60,
        )
        assert (os.fsencode(driver) in result.stderr) == ("--verbose-link" in flags), result.stderr
        return result.stdout.split(b"\0")[:-1]

    linux = ["--target", "x86_64-unknown-linux-gnu"]
    macos = ["--target", "aarch64-apple-darwin"]
    elf = link(*linux, "--opt-level", "0")
    assert b"-Wl,--gc-sections" in elf and b"-Wl,-z,relro,-z,now" in elf
    assert b"-Wl,-dead_strip" not in elf and b"-liconv" not in elf
    macho = link(*macos)
    assert b"-Wl,-dead_strip" in macho and b"-liconv" in macho
    assert not any(b"relro" in arg or b"gc-sections" in arg for arg in macho)
    assert b"-Wl,-dead_strip" not in link(*macos, IMREFLAGS="--no-gc-sections")

    elf = link(*linux, IMREFLAGS="--no-gc-sections\t-C relro-level=off")
    assert b"-Wl,--no-gc-sections" in elf and b"-Wl,-z,norelro" in elf
    args = link(
        *linux, "--gc-sections", "-Crelro-level=partial", "-C", "link-arg=-lm", "--verbose-link",
        IMREFLAGS="--invalid", IMRE_ENCODED_FLAGS="--no-gc-sections\x1f-Clink-arg=one argument",
    )
    assert b"-Wl,--gc-sections" in args and b"-Wl,-z,relro,-z,lazy" in args
    assert args[args.index(b"one argument") + 1] == b"-lm"
    assert b"-Wl,--gc-sections" in link(*linux, IMREFLAGS="--invalid", IMRE_ENCODED_FLAGS="")
    missing = subprocess.run(
        [*base, "--gc-sections"], env=clean_env | {"IMREFLAGS": "-C"},
        capture_output=True, timeout=60,
    )
    assert missing.returncode and b"-C requires a value" in missing.stderr
    return 7


def check_run_command(directory: Path, compiler: str, flags: list[str]) -> int:
    help_text = run([compiler, "run", "--help"], cwd=directory, input=b"")
    require_success(help_text, "imre run --help")
    assert b"imre run SOURCE.py" in help_text.stdout
    script_dir = directory / "scripts"
    script_dir.mkdir()
    source = script_dir / "show args.py"
    source.write_text(
        "import os\nfrom pathlib import Path\nfrom sys import argv\n"
        "for arg in argv:\n    print(arg)\n"
        "print(Path('message').read_text(encoding='utf-8'))\n"
        "print(os.environ.copy()['IMRE_RUN_TEST'])\nprint(input())\n",
        encoding="utf-8",
    )
    (directory / "message").write_text("current working directory", encoding="utf-8")
    scratch = directory / "compiler-temporaries"
    scratch.mkdir()
    env = {**environment(), "IMRE_RUN_TEST": "inherited environment", "TMPDIR": str(scratch)}
    path = "./scripts/show args.py"
    program_args = ["--opt-level", "", "two words", "--", "--help", "--allocator", "jemalloc"]
    expected = [path, *program_args, "current working directory", "inherited environment"]
    ready = ("\n".join(expected) + "\n").encode()
    for interrupt in (False, True):
        with subprocess.Popen(
            [compiler, "run", path, *flags, "--", *program_args], cwd=directory, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
        ) as process:
            try:
                visible_output(process.stdout.fileno(), ready, 60)
                assert not list(scratch.iterdir()), "run left temporary files while the child waited for input"
                if interrupt:
                    os.killpg(process.pid, signal.SIGKILL)
                out, error = process.communicate(None if interrupt else b"from stdin\n", timeout=60)
            finally:
                if process.returncode is None:
                    with suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=10)
        expected_code = -signal.SIGKILL if interrupt else 0
        assert process.returncode == expected_code and not error, (process.returncode, error)
        same_output(out, b"" if interrupt else b"from stdin\n", "imre run stdin")
        assert not list(scratch.iterdir()) and not (directory / source.stem).exists()
    rejected = [
        (["--allocator", "other"], b"--allocator expects system or jemalloc"),
        (["--allocator", "jemalloc", "--sanitize"], b"incompatible with --sanitize"),
        (["--target", "aarch64-apple-darwin", "--allocator", "jemalloc"], b"requires a Linux target"),
        (["--allocator", "jemalloc", "--target", "aarch64-linux-android"], b"requires a Linux target"),
        (["--allocator", "system", "--runtime", "custom.a"], b"--runtime already selects the allocator"),
        (["--runtime", "libimre_runtime_jemalloc.a", "--sanitize"], b"requires a system-allocator runtime"),
        (["-C", "relro-level=other"], b"relro-level expects full, partial, or off"),
    ]
    for arguments, message in rejected:
        result = run([compiler, "build", path, *arguments], cwd=directory, input=b"")
        assert result.returncode and message in result.stderr, result

    source.write_text("raise ValueError('script failed')\n", encoding="utf-8")
    compiled = [compiler, "run", path, *flags]
    failed = subprocess.run(compiled, cwd=directory, env=env, capture_output=True, timeout=60)
    assert failed.returncode == 1 and failed.stderr == b"ValueError: script failed\n", failed
    assert not failed.stdout and not list(scratch.iterdir())
    before = source.read_bytes()
    collision = run([*compiled, "--emit-llvm", path], cwd=directory, input=b"")
    assert collision.returncode and b"distinct paths" in collision.stderr and source.read_bytes() == before
    return 5 + len(rejected)


def broken_pipe(command: list[str]) -> subprocess.CompletedProcess[bytes]:
    reader, writer = os.pipe()
    os.close(reader)
    with os.fdopen(writer, "wb", buffering=0) as sink:
        return subprocess.run(
            command, stdin=subprocess.DEVNULL, stdout=sink, stderr=subprocess.PIPE,
            timeout=60, check=False, env=environment(),
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--timeout", type=float, default=10, help="interactive readiness deadline")
    args = parser.parse_args()
    build_flags = ["--quiet"]
    if args.lto_linker is not None:
        build_flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.target is not None:
        build_flags += ["--target", args.target]
    assert args.timeout > 0
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    passed = 0
    with tempfile.TemporaryDirectory(prefix="imre-io-") as directory:
        temporary = Path(directory)
        flags = ["--runtime", runtime, "--clang", args.clang, "--opt-level", args.opt_levels[0], *build_flags]
        if args.sanitize:
            flags.append("--sanitize")
        passed += check_link_options(temporary, compiler, runtime)
        passed += check_run_command(temporary, compiler, flags)
        sources = {}
        for name, source in PROGRAMS.items():
            path = temporary / f"{name}.py"
            path.write_text(source, encoding="utf-8")
            sources[name] = path

        reference = {}
        for name in ["small", "large", *ERRORS]:
            result = run([args.python, "-I", str(sources[name])], input=b"")
            if name in ERRORS:
                assert result.returncode > 0 and ERRORS[name] in result.stderr, (name, result)
            else:
                require_success(result, f"CPython {name}")
            reference[name] = result.stdout
        pipe_reference = {
            ending: pipe_exchange(
                [args.python, "-I", str(sources["pipe_input"])], payload, args.timeout
            )
            for ending, payload in PAYLOADS.items()
        }
        reference["terminal_loop"] = terminal_message(
            [args.python, "-I", str(sources["terminal_loop"])], args.timeout
        )
        large_pipe = broken_pipe([args.python, "-I", str(sources["large"])])
        assert large_pipe.returncode == 1, (large_pipe.returncode, large_pipe.stderr[-2000:])
        immediate = large_pipe.stderr.splitlines(keepends=True)[-1]
        assert immediate.startswith(f"BrokenPipeError: [Errno {errno.EPIPE}] ".encode()), immediate

        for level in args.opt_levels:
            binaries = {}
            for name, source in sources.items():
                binary = temporary / f"{name}-O{level}"
                command = [
                    compiler, "build", str(source), "-o", str(binary),
                    "--runtime", runtime, "--clang", args.clang, "--opt-level", level,
                ]
                if args.sanitize:
                    command.append("--sanitize")
                command += build_flags
                require_success(run(command), f"build {name} O{level}")
                binaries[name] = str(binary)

            for name in ["small", "large"]:
                result = run([binaries[name]], input=b"")
                require_success(result, f"{name} O{level}")
                same_output(result.stdout, reference[name], f"{name} O{level}")
                passed += 1

            for name, category in ERRORS.items():
                # Merging streams also verifies pending stdout precedes Imre's diagnostic.
                result = subprocess.run(
                    [binaries[name]], input=b"", stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT, timeout=60, check=False, env=environment(),
                )
                prefix = reference[name]
                same_output(result.stdout[:len(prefix)], prefix, f"{name} O{level}")
                diagnostic = result.stdout[len(prefix):]
                assert result.returncode == 1 and diagnostic.startswith(category + b":"), (
                    name, level, result.returncode, diagnostic[:2000],
                )
                assert diagnostic.count(b"\n") == 1 and diagnostic.endswith(b"\n"), (
                    name, level, diagnostic[:2000],
                )
                passed += 1

            for ending, payload in PAYLOADS.items():
                same_output(
                    pipe_exchange([binaries["pipe_input"]], payload, args.timeout),
                    pipe_reference[ending], f"pipe input {ending} O{level}",
                )
                passed += 1
            same_output(
                terminal_message([binaries["terminal_loop"]], args.timeout),
                reference["terminal_loop"], f"terminal output O{level}",
            )
            passed += 1

            for mode, free in [("write", 0), ("write", 4096), ("flush", 0)]:
                reference_run = blocked_write([args.python, "-I", str(sources["blocked_write"])], free, mode)
                native_run = blocked_write([binaries["blocked_write"]], free, mode)
                assert reference_run.returncode == native_run.returncode == 0
                assert b"BlockingIOError(" in reference_run.stderr
                same_output(native_run.stderr, reference_run.stderr, f"blocked {mode} with {free} free")
                same_output(native_run.stdout, reference_run.stdout, "buffer retained after blocked write")
                passed += 1

            # A caught print failure on either stream retains the pipe errno.
            reader, writer = os.pipe()
            os.close(reader)
            with os.fdopen(writer, "wb", buffering=0):
                commands = ([args.python, "-I", str(sources["print_errno"])], [binaries["print_errno"]])
                reference_run, native_run = [run([*cmd, str(writer)], pass_fds=(writer,)) for cmd in commands]
                assert reference_run.returncode == native_run.returncode == 0
                same_output(reference_run.stdout, (str(errno.EPIPE) + "\n").encode() * 2, "reference errno")
                same_output(native_run.stdout, reference_run.stdout, "print errno")
                same_output(native_run.stderr, reference_run.stderr, "retained stderr")
            passed += 1

            # The large print writes immediately. The smaller buffered writes fail
            # during shutdown, including after an unhandled division error.
            shutdown = b"Exception ignored while flushing sys.stdout:\nBrokenPipeError:"
            for name, status, prefix, lines in [
                ("small", 120, shutdown, 2),
                ("large", 1, immediate, 1),
                ("division_error", 120, b"ZeroDivisionError:", 3),
            ]:
                result = broken_pipe([binaries[name]])
                assert result.returncode == status and result.stderr.startswith(prefix), (
                    "broken pipe", name, level, result.returncode, result.stderr[:2000],
                )
                if status == 120:
                    assert shutdown in result.stderr, (name, level, result.stderr[:2000])
                assert result.stderr.count(b"\n") == lines and result.stderr.endswith(b"\n"), (
                    "broken pipe", name, level, result.stderr[:2000],
                )
                passed += 1
    print(f"Passed {passed} I/O checks (optimization levels {args.opt_levels}; ASan={args.sanitize})")


if __name__ == "__main__":
    main()
