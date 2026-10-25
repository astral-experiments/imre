"""Run unchanged Base45 tests and compare native/Rust results with the upstream codec."""

import argparse
import base64
import hashlib
import importlib.util
import io
import json
import os
import platform
import random
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "examples/base45_codec"
ENTRY = PACKAGE / "driver.py"
BOOTSTRAP = (
    "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
    "entry = sys.argv[2]; sys.argv = sys.argv[2:]; "
    "runpy.run_path(entry, run_name='__main__')"
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def verify_upstream() -> dict:
    record = json.loads((PACKAGE / "UPSTREAM.json").read_text())
    for name, metadata in record["files"].items():
        if digest((PACKAGE / name).read_bytes()) != metadata["sha256"]:
            raise AssertionError(f"upstream source changed: {name}")
    return record


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_upstream_tests():
    sys.path.insert(0, str(PACKAGE))
    import base45

    assert Path(base45.__file__).resolve() == PACKAGE / "base45/__init__.py"
    tests = load_module("base45_upstream_tests", PACKAGE / "upstream/test_base45.py")
    return base45, tests


def python_command(python: str) -> list[str]:
    return [python, "-I", "-B", "-c", BOOTSTRAP, str(PACKAGE), str(ENTRY)]


def error_lines(error: Exception) -> list[str]:
    cause = error.__cause__
    return [
        "ERROR",
        type(error).__name__,
        str(error),
        str(error.__suppress_context__),
        "None" if cause is None else type(cause).__name__,
        "" if cause is None else str(cause),
    ]


def oracle(codec, operation: str, payload: bytes, repetitions: int = 1) -> dict:
    try:
        if operation == "encode":
            result = codec.b45encode(payload)
        elif operation == "decode-bytes":
            result = codec.b45decode(payload)
        elif operation == "decode-text":
            result = codec.b45decode(payload.decode())
        else:
            raise ValueError(operation)
    except (ValueError, KeyError, TypeError, AttributeError) as error:
        return {"stdout": "\n".join(error_lines(error)) + "\n", "output": None}
    checksum = len(result) + (result[0] + result[-1] if result else 0)
    return {"stdout": f"OK\n{checksum * repetitions}\n", "output": result}


def execute(
    command: list[str],
    operation: str,
    payload: bytes,
    directory: Path,
    repetitions: int = 1,
    timeout: float = 60.0,
) -> dict:
    source = directory / "input.bin"
    output = directory / "output.bin"
    source.write_bytes(payload)
    output.write_bytes(b"unchanged output")
    process = subprocess.run(
        [*command, operation, str(source), str(output), str(repetitions)],
        input=b"",
        capture_output=True,
        timeout=timeout,
        cwd=directory,
        check=False,
        env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
    )
    assert process.returncode == 0 and not process.stderr, (
        operation,
        process.returncode,
        process.stdout,
        process.stderr,
    )
    assert source.read_bytes() == payload, "codec changed its input file"
    stdout = process.stdout.decode()
    result = output.read_bytes()
    if stdout.startswith("ERROR\n"):
        assert result == b"unchanged output", "failure modified output file"
        result = None
    return {"stdout": stdout, "output": result}


def corpus(codec, tests) -> list[tuple[str, str, bytes]]:
    cases = []
    for index, (raw, encoded) in enumerate(tests.GOOD_DATA):
        cases.extend(
            [
                (f"upstream-encode-{index}", "encode", raw),
                (f"upstream-bytes-{index}", "decode-bytes", encoded),
                (f"upstream-text-{index}", "decode-text", encoded),
            ]
        )
    malformed = [
        *tests.BAD_BASE45_STRINGS,
        b"\xff",
        b"\xc2",
        b"\xe0\x80\x80",
        b"\xf0\x90",
        b"BB8\n",
        b"BB8\n\n",
        b"BB8\r\n",
        b"bb8",
        b"\x00",
        "é".encode(),
        "🦀".encode(),
        b"   ",
        b"  ",
        b"GGWa",  # A later bad character wins over an earlier range failure.
        b"0000a",  # Alphabet validation precedes the invalid-length check.
        b"a\xff",  # Bytes decode completely before alphabet lookup starts.
        b"BB8\xed\xa0\x80",
        b"BB8\xf4\x90\x80\x80",
        b"'",
        b'"',
        b"\\",
    ]
    for index, value in enumerate(malformed):
        for operation in ("decode-bytes", "decode-text"):
            cases.append((f"malformed-{operation}-{index}", operation, value))
    for index, value in enumerate(("\u0301", "\U000e0100")):
        for operation in ("decode-bytes", "decode-text"):
            # Imre must preserve CPython's printable combining characters. The
            # Rust diagnostic adapter deliberately uses Rust's Unicode quoting.
            cases.append(
                (
                    f"unicode-repr-boundary-{operation}-{index}",
                    operation,
                    value.encode(),
                )
            )
    generator = random.Random(9285)
    for index, size in enumerate(
        [
            0,
            1,
            2,
            3,
            22,
            23,
            24,
            25,
            255,
            256,
            257,
            4096,
            *(generator.randrange(512) for _ in range(48)),
        ]
    ):
        raw = generator.randbytes(size)
        encoded = codec.b45encode(raw)
        cases.extend(
            [
                (f"random-encode-{index}", "encode", raw),
                (f"random-bytes-{index}", "decode-bytes", encoded),
                (f"random-text-{index}", "decode-text", encoded),
            ]
        )
    cases.append(("all-byte-values", "encode", bytes(range(256))))
    return cases


def upstream_suite(tests) -> dict:
    stream = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromModule(tests)
    result = unittest.TextTestRunner(stream=stream, verbosity=2).run(suite)
    assert result.wasSuccessful(), stream.getvalue()
    return {"methods": result.testsRun, "log": stream.getvalue()}


def validate(binary: Path, compiler: Path, directory: Path) -> dict:
    provenance = verify_upstream()
    codec, tests = load_upstream_tests()
    reference = upstream_suite(tests)
    shutil.copytree(PACKAGE / "base45", directory / "base45")
    native_calls = 0
    rejected_calls = []

    def native(operation, value):
        nonlocal native_calls
        valid = type(value) is bytes or (operation == "decode" and type(value) is str)
        if not valid:
            # The upstream suite's two wrong-typed calls exercise the compiler's
            # boundary contract; do not report them as native runtime TypeErrors.
            assert type(value) is int and value == 42
            source = directory / f"rejected_{operation}.py"
            source.write_text(
                f"from base45 import b45{operation}\nb45{operation}(42)\n"
            )
            checked = subprocess.run(
                [str(compiler), "check", str(source), "--module-root", str(directory)],
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            expected_type = "bytes" if operation == "encode" else "str | bytes"
            diagnostic = (
                f"Imre: expected {expected_type}, got int; "
                "annotations are enforced contracts"
            )
            assert checked.returncode == 1 and not checked.stdout, checked
            assert checked.stderr.strip() == f"{source.resolve()}:2:11: {diagnostic}", (
                checked.stderr
            )
            rejected_calls.append(
                {"operation": operation, "diagnostic": checked.stderr}
            )
            raise TypeError("wrong-typed call rejected during native compilation")
        mode = (
            "encode"
            if operation == "encode"
            else "decode-text"
            if type(value) is str
            else "decode-bytes"
        )
        payload = value.encode() if type(value) is str else value
        actual = execute([str(binary)], mode, payload, directory)
        expected = oracle(codec, mode, payload)
        assert actual == expected, (mode, payload, actual, expected)
        native_calls += 1
        if actual["output"] is None:
            # Tests assert the outer category. The comparison above also checked
            # exact messages, cause category/message, and suppression metadata.
            assert actual["stdout"].splitlines()[1] == "ValueError"
            raise ValueError(actual["stdout"].splitlines()[2])
        return actual["output"]

    tests.b45encode = lambda value: native("encode", value)
    tests.b45decode = lambda value: native("decode", value)
    native_reference = upstream_suite(tests)
    cases = []
    for label, operation, payload in corpus(codec, tests):
        expected = oracle(codec, operation, payload)
        actual = execute([str(binary)], operation, payload, directory)
        assert actual == expected, (label, actual, expected)
        cases.append(
            {
                "label": label,
                "operation": operation,
                "input_base64": base64.b64encode(payload).decode(),
                "stdout": actual["stdout"],
                "output_sha256": None
                if actual["output"] is None
                else digest(actual["output"]),
            }
        )
    assert verify_upstream() == provenance
    return {
        "upstream": provenance,
        "python": sys.version,
        "implementation": platform.python_implementation(),
        "binary_sha256": digest(binary.read_bytes()),
        "compiler_sha256": digest(compiler.read_bytes()),
        "upstream_cpython": reference,
        "upstream_native_adapter": native_reference,
        "native_adapter_calls": native_calls,
        "wrong_typed_compile_rejections": rejected_calls,
        "differential_cases": cases,
    }


def validate_rust(binary: Path, directory: Path) -> dict:
    """Check the independent codec through the same typed file protocol and oracle."""
    directory.mkdir(parents=True)
    codec, tests = load_upstream_tests()
    binary_hash = digest(binary.read_bytes())
    cases = []
    diagnostic_boundaries = []
    inputs = [
        (label, operation, payload, 1)
        for label, operation, payload in corpus(codec, tests)
    ]
    for operation, payload in (
        ("encode", b""),
        ("encode", bytes(range(256))),
        ("decode-bytes", b"BB8"),
        ("decode-text", b"BB8\n\n"),
        ("decode-bytes", b"GGWa"),
        ("decode-text", b"\xff"),
    ):
        inputs.append((f"repeated-{operation}-{len(inputs)}", operation, payload, 7))
    for label, operation, payload, repetitions in inputs:
        expected = oracle(codec, operation, payload, repetitions)
        actual = execute([str(binary)], operation, payload, directory, repetitions)
        if label.startswith("unicode-repr-boundary-"):
            assert actual["output"] is expected["output"] is None
            # All metadata except the invalid character's quoted spelling is
            # shared; this adapter does not embed a Python Unicode repr table.
            assert (
                actual["stdout"].splitlines()[:5] == expected["stdout"].splitlines()[:5]
            ), (label, actual, expected)
            diagnostic_boundaries.append(
                {
                    "label": label,
                    "cpython": expected["stdout"],
                    "rust": actual["stdout"],
                }
            )
        else:
            assert actual == expected, (label, actual, expected)
        cases.append(
            {
                "label": label,
                "operation": operation,
                "repetitions": repetitions,
                "input_base64": base64.b64encode(payload).decode(),
                "stdout": actual["stdout"],
                "output_sha256": None
                if actual["output"] is None
                else digest(actual["output"]),
            }
        )
    assert digest(binary.read_bytes()) == binary_hash, (
        "Rust executable changed during validation"
    )
    return {
        "binary_sha256": binary_hash,
        "differential_cases": cases,
        "exact_message_cases": len(cases) - len(diagnostic_boundaries),
        "diagnostic_boundaries": diagnostic_boundaries,
        "scope": "Same bytes/string file protocol. Exact diagnostic text is checked over the declared corpus except explicitly recorded Unicode repr boundaries, where category/cause/suppression and unchanged output are checked. Wrong-typed Python API calls have no Rust CLI representation.",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if bool(args.binary) != bool(args.compiler):
        parser.error("--binary and --compiler must be supplied together")
    if not args.binary and not args.rust:
        parser.error("provide --binary with --compiler, or --rust")
    with tempfile.TemporaryDirectory(prefix="imre-base45-validation-") as temporary:
        if args.binary:
            record = validate(
                args.binary.resolve(), args.compiler.resolve(), Path(temporary)
            )
        else:
            _, tests = load_upstream_tests()
            record = {
                "upstream": verify_upstream(),
                "python": sys.version,
                "implementation": platform.python_implementation(),
                "upstream_cpython": upstream_suite(tests),
            }
        if args.rust:
            record["rust"] = validate_rust(
                args.rust.resolve(), Path(temporary) / "rust"
            )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    if args.binary:
        print(
            f"Passed {record['native_adapter_calls']} upstream native calls, "
            f"{len(record['differential_cases'])} differential cases, and "
            f"{len(record['wrong_typed_compile_rejections'])} compile-time type rejections"
        )
    if args.rust:
        print(
            f"Passed {len(record['rust']['differential_cases'])} Rust differential checks"
        )


if __name__ == "__main__":
    main()
