"""Compare immutable bytes and binary file effects with CPython."""

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import BOOTSTRAP, require_result, run, snapshot, write_files
from files import run_with_failing_output

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "bytes"

REJECTED = {
    "hashlib_sha256_text": 'from hashlib import sha256\nprint(sha256("text").hexdigest())\n',
    "hashlib_update_text": 'from hashlib import sha256\nhash_obj = sha256()\nhash_obj.update("data")\n',
    "hashlib_digest_args": 'from hashlib import sha256\nhash_obj = sha256()\nprint(hash_obj.digest(4))\n',
    "hashlib_hexdigest_args": 'from hashlib import sha256\nprint(sha256().hexdigest(3))\n',
    "bytes_unicode_literal": 'value = b"é"\n',
    "bytes_mixed_literal": 'value = b"a" "b"\n',
    "bytes_string_annotation": 'value: bytes = "wrong"\n',
    "bytes_bool_count": "print(bytes(True))\n",
    "bytes_bool_element": "print(bytes([True]))\n",
    "bytes_string_constructor": 'print(bytes("text"))\n',
    "bytes_tuple_text_element": 'print(bytes((1, "2")))\n',
    "bytes_keywords": "print(bytes(source=3))\n",
    "bytes_mixed_add": 'print(b"a" + "b")\n',
    "bytes_mixed_compare": 'print(b"a" < "b")\n',
    "bytes_bool_index": 'print(b"a"[True])\n',
    "bytes_bool_slice": 'print(b"a"[::True])\n',
    "bytes_optional_index": 'def bad(index: int | None) -> int:\n    return b"a"[index]\n',
    "bytes_assignment": 'value = b"abc"\nvalue[0] = 1\n',
    "bytes_deletion": 'value = b"abc"\ndel value[0]\n',
    "bytes_slice_assignment": 'value = b"abc"\nvalue[:] = b"x"\n',
    "bytes_contains_memoryview": 'print(memoryview(b"a") in b"abc")\n',
    "bytes_contains_array": 'from array import array\nprint(array("B", [97]) in b"abc")\n',
    "bytes_contains_user_index": (
        "class Index:\n    def __index__(self) -> int:\n        return 97\n"
        'print(Index() in b"abc")\n'
    ),
    "bytes_mutable": 'value = bytearray(b"abc")\n',
    "decode_errors": 'print(b"x".decode(errors="replace"))\n',
    "bytes_iterator_escape": 'value = iter(b"abc")\n',
    "bytes_default_wrong_type": 'def bad(value: bytes = "x") -> bytes:\n    return value\n',
    "binary_text_write": 'with open("out", "wb") as stream:\n    stream.write("wrong")\n',
    "text_bytes_write": 'with open("out", "w", encoding="utf-8") as stream:\n    stream.write(b"wrong")\n',
    "binary_annotation": 'from typing import BinaryIO\nvalue: BinaryIO = open("out", "w", encoding="utf-8")\n',
    "binary_encoding": 'value = open("out", "wb", encoding="utf-8")\n',
    "binary_update": 'value = open("out", "wb+")\n',
    "binary_bool_size": 'with open("input", "rb") as stream:\n    stream.read(True)\n',
    "binary_seek_text_offset": 'with open("input", "rb") as stream:\n    stream.seek("start")\n',
    "binary_closed_write": 'with open("out", "wb") as stream:\n    stream.closed = False\n',
}

IMPORTED_REJECTIONS = {}

ERRORS = {
    "uncaught_decode": (
        'print("before")\nprint(b"\\xff".decode())\n',
        "UnicodeDecodeError",
    ),
    "uncaught_index": ('print(b""[0])\n', "IndexError"),
    "uncaught_constructor": ("print(bytes([1, 256]))\n", "ValueError"),
    "uncaught_count": ("print(bytes(-1))\n", "ValueError"),
    "uncaught_overflow": ("print(bytes(9223372036854775808))\n", "OverflowError"),
    "uncaught_slice": ('print(b"owned"[::0])\n', "ValueError"),
    "uncaught_repeat": ('print(b"ab" * 9223372036854775807)\n', "OverflowError"),
    "uncaught_file": (
        'with open("saved.bin", "wb") as stream:\n    stream.write(b"saved\\0\\xff")\n    raise ValueError("body")\n',
        "ValueError",
    ),
    "exclusive_exists": (
        'with open("exists.bin", "xb") as stream:\n    stream.write(b"saved")\nwith open("exists.bin", "xb"):\n    print("unreachable")\n',
        "FileExistsError",
    ),
}


def cases(project: Path) -> list[tuple[str, Path, str | None]]:
    result = [(path.stem, path, None) for path in sorted(project.glob("*.py"))]
    result.append(("imported", project / "imported/main.py", None))
    for name, (source, error) in ERRORS.items():
        path = project / "errors" / name / "main.py"
        write_files(path.parent, {"main.py": source})
        result.append((name, path, error))
    return result


def reference(
    python: str, source: Path, cwd: Path, error: str | None, runner
) -> subprocess.CompletedProcess[str]:
    # Like exceptions.py, exercise intentional finally returns on Python 3.14+.
    result = run(
        [
            python,
            "-I",
            "-B",
            "-W",
            "ignore::SyntaxWarning",
            "-c",
            BOOTSTRAP,
            str(source.parent),
            str(source),
        ],
        cwd,
        runner=runner,
    )
    require_result(result, error)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument(
        "--opt-levels", nargs="+", choices=["0", "1", "2", "3"], default=["0", "3"]
    )
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    assert len(set(args.opt_levels)) == len(args.opt_levels), (
        "optimization levels must be distinct"
    )
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    python = str(Path(args.python).resolve()) if "/" in args.python else args.python
    clang = str(Path(args.clang).resolve()) if "/" in args.clang else args.clang
    flags = ["--quiet"]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker is not None:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags.append("--sanitize")
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-bytes-") as directory:
        temporary = Path(directory)
        project = temporary / "project"
        shutil.copytree(FIXTURES, project, ignore=shutil.ignore_patterns("__pycache__"))
        for name, source, error in cases(project):
            reference_dir = temporary / name / "reference"
            reference_dir.mkdir(parents=True)
            runner = run_with_failing_output if name == "file_failures" else subprocess.run
            expected = reference(python, source, reference_dir, error, runner)
            expected_files = snapshot(reference_dir)
            for level in args.opt_levels:
                actual_dir = temporary / name / ("O" + level)
                actual_dir.mkdir()
                binary = temporary / name / ("program-O" + level)
                built = run(
                    [
                        compiler,
                        "build",
                        str(source),
                        "--module-root",
                        str(source.parent),
                        "-o",
                        str(binary),
                        "--runtime",
                        runtime,
                        "--clang",
                        clang,
                        "--opt-level",
                        level,
                        *flags,
                    ],
                    temporary,
                )
                assert built.returncode == 0 and not built.stderr, (
                    name,
                    level,
                    built.stdout,
                    built.stderr,
                )
                actual = run([str(binary)], actual_dir, runner=runner)
                require_result(actual, error)
                assert actual.stdout == expected.stdout, (
                    name,
                    level,
                    actual.stdout,
                    expected.stdout,
                    actual.stderr,
                )
                assert snapshot(actual_dir) == expected_files, (
                    name,
                    level,
                    snapshot(actual_dir),
                    expected_files,
                )
                checks += 1
        rejected = {name: {"main.py": source} for name, source in REJECTED.items()}
        rejected.update(IMPORTED_REJECTIONS)
        for name, files in rejected.items():
            case = temporary / "rejected" / name
            write_files(case, files)
            result = run(
                [compiler, "check", str(case / "main.py"), "--module-root", str(case)],
                temporary,
            )
            assert result.returncode > 0 and result.stderr, (name, result)
            assert (
                "panicked" not in result.stderr
                and "AddressSanitizer" not in result.stderr
            ), (name, result.stderr)
            if "recursion" in name:
                assert "recursive calls are unsupported" in result.stderr, (
                    name,
                    result.stderr,
                )
            checks += 1
    print(
        f"Passed {checks} bytes and binary-file checks ({len(rejected)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
