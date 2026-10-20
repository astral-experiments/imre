"""Validate unchanged Luhn tests and a Unicode/error corpus against CPython."""

import argparse
import base64
import doctest
import hashlib
import importlib.util
import io
import json
import os
import random
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "examples/luhn_checksum"
ENTRY = PACKAGE / "driver.py"
OPERATIONS = ("checksum", "verify", "generate", "append")
BOOTSTRAP = (
    "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
    "entry = sys.argv[2]; sys.argv = sys.argv[2:]; "
    "runpy.run_path(entry, run_name='__main__')"
)
PRINTABLE_MARKS = (
    "\u0301",
    "\u036f",
    "\u0483",
    "\ufe00",
    "\ufe0f",
    "\U000e0100",
    "\U000e01ef",
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def verify_upstream() -> dict:
    record = json.loads((PACKAGE / "UPSTREAM.json").read_text())
    for name, metadata in record["files"].items():
        data = (PACKAGE / name).read_bytes()
        assert digest(data) == metadata["sha256"] and len(data) == metadata["bytes"], (
            name
        )
        assert (
            hashlib.sha1(
                f"blob {len(data)}\0".encode() + data, usedforsecurity=False
            ).hexdigest()
            == metadata["git_blob"]
        ), name
    return record


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_upstream():
    codec = load_module("luhn", PACKAGE / "luhn.py")
    previous = sys.modules.get("luhn")
    sys.modules["luhn"] = codec
    try:
        tests = load_module("luhn_upstream_tests", PACKAGE / "upstream/test.py")
    finally:
        if previous is None:
            del sys.modules["luhn"]
        else:
            sys.modules["luhn"] = previous
    return codec, tests


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
        value = getattr(codec, operation)(payload.decode())
    except ValueError as error:
        return {"stdout": "\n".join(error_lines(error)) + "\n", "output": None}
    contribution = len(value) if operation == "append" else int(value)
    return {
        "stdout": f"OK\n{contribution * repetitions}\n",
        "output": str(value).encode(),
    }


def execute(
    command: list[str],
    operation: str,
    payload: bytes,
    directory: Path,
    repetitions: int = 1,
    timeout: float = 60,
) -> dict:
    source, output = directory / "input.bin", directory / "output.bin"
    source.write_bytes(payload)
    output.write_bytes(b"unchanged output")
    result = subprocess.run(
        [*command, operation, str(source), str(output), str(repetitions)],
        input=b"",
        capture_output=True,
        timeout=timeout,
        cwd=directory,
        env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
        check=False,
    )
    assert result.returncode == 0 and not result.stderr, (
        command,
        operation,
        result.returncode,
        result.stdout,
        result.stderr,
    )
    assert source.read_bytes() == payload, "input file changed"
    stdout = result.stdout.decode()
    actual = output.read_bytes()
    if stdout.startswith("ERROR\n"):
        assert actual == b"unchanged output", "error modified output file"
        actual = None
    return {"stdout": stdout, "output": actual}


def upstream_suite(codec, tests, api=None) -> dict:
    api = codec if api is None else api
    original = tests.luhn
    tests.luhn = api
    methods = []
    try:
        for name, function in sorted(vars(tests).items()):
            if name.startswith("test_") and callable(function):
                function()
                methods.append(name)
    finally:
        tests.luhn = original
    stream = io.StringIO()
    runner = doctest.DocTestRunner(verbose=True)
    for test in doctest.DocTestFinder().find(codec):
        test.globs.update({name: getattr(api, name) for name in OPERATIONS})
        runner.run(test, out=stream.write)
    assert runner.failures == 0, stream.getvalue()
    return {
        "tests": methods,
        "doctest_examples": runner.tries,
        "log": stream.getvalue(),
    }


def corpus() -> list[tuple[str, str, bytes, int]]:
    cases = []
    examples = [
        "",
        "0",
        "7",
        "13",
        "383",
        "2827",
        "4346537657597",
        "27184931073326",
        "356938035643809",
        "4222222222222222",
        "7992739871",
        "53461861341123",
        "٠١٢٣٤٥٦٧٨٩",
        "１２３４５６７８９０",
        "1٢३４𝟝6",
        "𝟎𝟙𝟚𝟛𝟜𝟝𝟞𝟟𝟠𝟡",
    ]
    generator = random.Random(3917)
    for size in [15, 16, 17, 255, 256, 257, 4096, *range(32)]:
        examples.append("".join(str(generator.randrange(10)) for _ in range(size)))
    for index, value in enumerate(examples):
        for operation in OPERATIONS:
            cases.append((f"valid-{index}-{operation}", operation, value.encode(), 3))
    # Each decimal digit is checked individually: a whole-block checksum alone
    # could hide compensating digit-conversion mistakes.
    for codepoint in range(0x110000):
        character = chr(codepoint)
        if unicodedata.decimal(character, None) is not None:
            cases.append(
                (f"decimal-U+{codepoint:04X}", "checksum", character.encode(), 1)
            )
    invalid = [
        "x",
        " ",
        "+",
        "-",
        "_",
        "²",
        "½",
        "四",
        "🦀",
        "\0",
        "\t",
        "\n",
        "\r",
        "'",
        '"',
        "\\",
        "\x1f",
        "\x7f",
        "\u00a0",
        "\u200b",
        "1x2y",
        "y2x1",
        "12_3",
        " 123",
        "123 ",
        "١x٢y",
        "\U00011de0",  # A Unicode17 decimal digit is unassigned in Unicode16.
    ]
    for index, value in enumerate(invalid):
        for operation in OPERATIONS:
            cases.append((f"invalid-{index}-{operation}", operation, value.encode(), 2))
    for index, value in enumerate(PRINTABLE_MARKS):
        for operation in OPERATIONS:
            cases.append(
                (f"printable-mark-{index}-{operation}", operation, value.encode(), 1)
            )
    for index, payload in enumerate(
        [
            b"\xff",
            b"\xc2",
            b"\xe0\x80\x80",
            b"\xf0\x90",
            b"x\xff",
            b"123\xed\xa0\x80",
            b"123\xf4\x90\x80\x80",
        ]
    ):
        for operation in OPERATIONS:
            cases.append((f"utf8-{index}-{operation}", operation, payload, 1))
    return cases


def validate(command: list[str], directory: Path) -> dict:
    directory.mkdir(parents=True, exist_ok=True)
    provenance = verify_upstream()
    codec, tests = load_upstream()
    reference = upstream_suite(codec, tests)
    calls = 0

    def native(operation, value):
        nonlocal calls
        assert type(value) is str
        actual = execute(command, operation, value.encode(), directory)
        assert actual == oracle(codec, operation, value.encode())
        calls += 1
        result = actual["output"]
        assert result is not None
        if operation == "append":
            return result.decode()
        if operation == "verify":
            return result == b"True"
        return int(result)

    adapter = SimpleNamespace(
        **{name: lambda value, name=name: native(name, value) for name in OPERATIONS}
    )
    native_tests = upstream_suite(codec, tests, adapter)
    cases = []
    for label, operation, payload, repetitions in corpus():
        expected = oracle(codec, operation, payload, repetitions)
        actual = execute(command, operation, payload, directory, repetitions)
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
    assert verify_upstream() == provenance
    return {
        "upstream_cpython": reference,
        "upstream_native_adapter": native_tests,
        "native_adapter_calls": calls,
        "differential_cases": cases,
        "exact_message_cases": len(cases),
        "scope": "The str call sites of all four upstream APIs through a UTF-8 file protocol. No claim about arbitrary iterable or wrong-typed Python arguments. All successful outputs and error messages/category/cause/suppression are exact, with no diagnostic exclusions.",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    provenance = verify_upstream()
    codec, tests = load_upstream()
    record = {
        "upstream": provenance,
        "python": sys.version,
        "python_executable_sha256": digest(Path(sys.executable).read_bytes()),
        "unicode_version": unicodedata.unidata_version,
        "upstream_cpython": upstream_suite(codec, tests),
    }
    protected = {path.resolve() for path in (args.binary, args.rust) if path}
    if args.output.resolve() in protected:
        parser.error("--output must not replace a validated binary")
    with tempfile.TemporaryDirectory(prefix="relar-luhn-validation-") as temporary:
        for name, binary in (("relar", args.binary), ("rust", args.rust)):
            if binary:
                binary = binary.resolve(strict=True)
                before = digest(binary.read_bytes())
                record[name] = validate([str(binary)], Path(temporary) / name)
                assert digest(binary.read_bytes()) == before, "validated binary changed"
                record[name]["binary_sha256"] = before
                print(
                    f"{name}: {record[name]['native_adapter_calls']} upstream calls, "
                    f"{len(record[name]['differential_cases'])} differential cases"
                )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
