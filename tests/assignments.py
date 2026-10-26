"""Compare assignment expression evaluation, scope, flow, and ownership with CPython."""

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import BOOTSTRAP, require_result, run, snapshot, write_files

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "assignments"

REJECTED = {
    "conditional_assignment": "def bad(flag: bool) -> int:\n    value = (saved := 1) if flag else 0\n    return saved\n",
    "short_circuit_and": "def bad(flag: bool) -> int:\n    value = flag and (saved := 1) > 0\n    return saved\n",
    "short_circuit_or": "def bad(flag: bool) -> int:\n    value = flag or (saved := 1) > 0\n    return saved\n",
    "chained_late_operand": "value = 5 < 4 < (saved := 9)\nprint(saved)\n",
    "assert_failure_message": "assert True, str(saved := 4)\nprint(saved)\n",
    "optional_fact_replaced": 'value: str | None = "ok"\nvalid = value is not None and (value := None) is None\nif valid:\n    print(value + "bad")\n',
    "optional_boolean_stale": 'value: str | None = "ok"\nif value is not None and (value := None) is None:\n    print(value + "bad")\n',
    "optional_short_circuit_stale": 'value: str | None = "ok"\nvalid = True and (value := None) is None\nrequired: str = value\n',
    "optional_conditional_stale": 'def bad(flag: bool) -> str:\n    value: str | None = "ok"\n    ignored = (value := None) if flag else None\n    return value\n',
    "optional_loop_stale": 'value: str | None = "ok"\nwhile len(value) > 0 and (value := None) is None:\n    pass\n',
    "comprehension_optional_stale": 'value: str | None = "ok"\nvalues = [(value := None) for index in range(2)]\nrequired: str = value\n',
    "comprehension_backedge_stale": 'value: str | None = "ok"\nvalues = [(value := None) for index in range(2) if len(value) > 0]\n',
    "comprehension_nested_backedge_stale": 'value: str | None = "ok"\nvalues = [(value := None) for outer in range(1) if value is not None for inner in range(2) if len(value) > 0]\n',
    "empty_comprehension_binding": "values = [(saved := item) for item in range(0)]\nprint(saved)\n",
    "comprehension_private_target": "saved = 0\nvalues = [(saved := target) for target in range(3)]\nprint(target)\n",
    "rebind_comprehension_target": "values = [(item := 1) for item in range(3)]\n",
    "rebind_later_target": "values = [saved for item in range(3) if (saved := 1) > 0 for saved in range(2)]\n",
    "rebind_enclosing_target": "values = [[(outer := inner) for inner in range(2)] for outer in range(2)]\n",
    "comprehension_iterable": "values = [item for item in (source := [1, 2])]\n",
    "nested_comprehension_iterable": "saved = 0\nvalues = [item for item in [(saved := inner) for inner in range(2)]]\n",
    "comprehension_filter_scope": "def bad(flag: bool) -> list[int]:\n    return [saved for index in range(2) if flag or (saved := index) > 0]\n",
    "wrong_exact_type": 'value = 1\nprint((value := "wrong"))\n',
    "wrong_optional_type": "value: int | None = None\nprint((value := False))\n",
    "attribute_target": "class Item:\n    value: int\n    def __init__(self) -> None:\n        self.value = 1\nitem = Item()\nprint((item.value := 2))\n",
    "builtin_target": "print((int := 1))\n",
    "conditional_type_change": "def scale(value: int, flag: bool) -> float:\n    if flag:\n        value: float = float(value)\n    return float(value)\n",
    "loop_type_change": "def scale(value: int) -> float:\n    while value > 0:\n        value: float = float(value)\n    return float(value)\n",
    "captured_type_change": "def scale(value: int) -> float:\n    def read() -> int:\n        return value\n    value: float = float(value)\n    return float(read())\n",
    "shadowed_class_operand": "def test(bytes: int) -> bool:\n    return isinstance(4, bytes)\n",
    "recursive_comprehension": "def recurse() -> list[int]:\n    saved = 0\n    return [(saved := recurse()[0]) for index in range(0)]\n",
}

IMPORTED_REJECTIONS = {}

ERRORS = {
    "rhs_failure": ('print("start")\nprint((saved := 1 // 0))\n', "ZeroDivisionError"),
    "comprehension_failure": (
        "saved = 0\nvalues = [(saved := index) // (2 - index) for index in range(4)]\n",
        "ZeroDivisionError",
    ),
}


def cases(
    project: Path, errors: dict[str, tuple[str, str]] = ERRORS
) -> list[tuple[str, Path, str | None]]:
    result = [(path.stem, path, None) for path in sorted(project.glob("*.py"))]
    imported = project / "imported/main.py"
    if imported.is_file():
        result.append(("imported", imported, None))
    for name, (source, error) in errors.items():
        path = project / "errors" / name / "main.py"
        write_files(path.parent, {"main.py": source})
        result.append((name, path, error))
    return result


def reference(
    python: str, source: Path, cwd: Path, error: str | None
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
    )
    require_result(result, error)
    return result


def main(
    *,
    fixtures: Path = FIXTURES,
    rejections: dict[str, str] = REJECTED,
    imported_rejections: dict[str, dict[str, str]] = IMPORTED_REJECTIONS,
    errors: dict[str, tuple[str, str]] = ERRORS,
    label: str = "assignment expression",
    description: str | None = __doc__,
    thinlto_build_timeout: int = 60,
) -> None:
    """Run a fixture suite using the shared output, filesystem, and failure oracle."""
    parser = argparse.ArgumentParser(description=description)
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
    with tempfile.TemporaryDirectory(prefix=f"imre-{fixtures.name}-") as directory:
        temporary = Path(directory)
        project = temporary / "project"
        shutil.copytree(fixtures, project, ignore=shutil.ignore_patterns("__pycache__"))
        for name, source, error in cases(project, errors):
            reference_dir = temporary / name / "reference"
            reference_dir.mkdir(parents=True)
            expected = reference(python, source, reference_dir, error)
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
                    timeout=thinlto_build_timeout if args.lto_linker and not args.sanitize else 60,
                )
                assert built.returncode == 0 and not built.stderr, (
                    name,
                    level,
                    built.stdout,
                    built.stderr,
                )
                actual = run([str(binary)], actual_dir)
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
        rejected = {name: {"main.py": source} for name, source in rejections.items()}
        rejected.update(imported_rejections)
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
        f"Passed {checks} {label} checks ({len(rejected)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
