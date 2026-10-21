"""Compare output and interactive I/O behavior with CPython, without timing claims."""

import argparse
import errno
import os
import pty
import selectors
import subprocess
import sys
import tempfile
import termios
import time
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
    "large": "print(" + repr("prefix\0" + "éλ🙂" * 8192 + "\0suffix") + ")\n"
    'for index in range(160):\n    print(index)\nprint("large output finished")\n',
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--timeout", type=float, default=10, help="interactive readiness deadline")
    args = parser.parse_args()
    assert args.timeout > 0
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    passed = 0
    with tempfile.TemporaryDirectory(prefix="relar-io-") as directory:
        temporary = Path(directory)
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
                require_success(run(command), f"build {name} O{level}")
                binaries[name] = str(binary)

            for name in ["small", "large"]:
                result = run([binaries[name]], input=b"")
                require_success(result, f"{name} O{level}")
                same_output(result.stdout, reference[name], f"{name} O{level}")
                passed += 1

            for name, category in ERRORS.items():
                # Merging streams also verifies pending stdout precedes Relar's diagnostic.
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

            # CPython may report status 120 during shutdown. Relar's contract is OSError/1.
            for name, category in [
                ("small", b"OSError"),
                ("large", b"OSError"),
                ("division_error", b"ZeroDivisionError"),
            ]:
                with open("/dev/full", "wb", buffering=0) as sink:
                    result = subprocess.run(
                        [binaries[name]], stdin=subprocess.DEVNULL, stdout=sink,
                        stderr=subprocess.PIPE, timeout=60, check=False, env=environment(),
                    )
                assert result.returncode == 1 and result.stderr.startswith(category + b":"), (
                    "/dev/full", name, level, result.returncode, result.stderr[:2000],
                )
                assert result.stderr.count(b"\n") == 1 and result.stderr.endswith(b"\n"), (
                    "/dev/full", name, level, result.stderr[:2000],
                )
                passed += 1
    print(f"Passed {passed} I/O checks (optimization levels {args.opt_levels}; ASan={args.sanitize})")


if __name__ == "__main__":
    main()
