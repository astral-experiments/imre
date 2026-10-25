"""Compare conditional expressions and concrete sequence access with CPython."""

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import BOOTSTRAP, require_result, run, snapshot, write_files

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "expressions"

REJECTED = {
    "conditional_int_string": 'print(1 if True else "wrong")\n',
    "conditional_bool_int": "print(True if False else 1)\n",
    "conditional_container_types": 'values = [1] if True else ["wrong"]\n',
    "conditional_wrong_context": "values: list[str] = [] if True else [1]\n",
    "conditional_wrong_optional": "value: int | None = None if True else False\n",
    "conditional_distinct_optionals": "def bad(number: int | None, flag: bool | None, choose: bool) -> int | None:\n    return number if choose else flag\n",
    "conditional_narrowing_does_not_escape": "def bad(value: int | None) -> int:\n    result = value + 1 if value is not None else 0\n    return value + result\n",
    "conditional_dead_dynamic": 'print(1 if True else eval("2"))\n',
    "conditional_unused_dynamic": 'def unused() -> int:\n    return eval("1") if False else 2\nprint(1)\n',
    "conditional_recursion": "def recursive(value: int) -> int:\n    return recursive(value) if False else value\nrecursive(1)\n",
    "conditional_indirect_recursion": "def first(value: int) -> int:\n    return second(value) if False else value\ndef second(value: int) -> int:\n    return first(value) if True else 0\nfirst(1)\n",
    "string_index_bool": 'print("abc"[True])\n',
    "string_index_string": 'print("abc"["0"])\n',
    "string_index_optional": 'def bad(index: int | None) -> str:\n    return "abc"[index]\n',
    "string_assignment": 'value = "abc"\nvalue[0] = "x"\n',
    "string_deletion": 'value = "abc"\ndel value[0]\n',
    "slice_bool_start": "values = [1, 2][True:]\n",
    "slice_bool_stop": 'value = "abc"[:False]\n',
    "slice_bool_step": "values = [1, 2][::True]\n",
    "slice_string_bound": 'value = "abc"["1":]\n',
    "slice_optional_bool": 'def bad(step: bool | None) -> str:\n    return "abc"[::step]\n',
    "slice_tuple": "values = (1, 2)[:]\n",
    "heterogeneous_tuple_index_out_of_bounds": 'values = (1, "two")\nprint(values[2])\n',
    "slice_existing_type_change": "original: list[int] = [1]\nvalues: list[int | None] = original[:]\n",
    "slice_dictionary": "values = {0: 1}[:]\n",
    "slice_assignment": "values = [1, 2]\nvalues[:] = [3]\n",
    "slice_deletion": "values = [1, 2]\ndel values[:]\n",
    "slice_augmented_assignment": "values = [1, 2]\nvalues[:] += [3]\n",
    "string_iterator_escape": 'iterator = iter("abc")\n',
    "string_loop_uninitialized_target": 'for character in "":\n    pass\nprint(character)\n',
}

IMPORTED_REJECTIONS = {
    "inactive_conditional_dynamic": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": 'if __name__ == "__main__":\n    print(1 if True else eval("2"))\n',
    },
    "unused_conditional_recursion": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": "def recursive(value: int) -> int:\n    return value if True else recursive(value)\n",
    },
}

ERRORS = {
    "condition_failure": (
        'def condition() -> bool:\n    print("condition")\n    raise ValueError("condition failure")\ndef arm() -> str:\n    print("must not evaluate")\n    return "unused"\nprint(arm() if condition() else arm())\n',
        "ValueError",
    ),
    "selected_arm_failure": (
        'def selected() -> str:\n    print("selected")\n    raise ValueError("arm failure")\ndef unused() -> str:\n    print("must not evaluate")\n    return "unused"\nprint(selected() if True else unused())\n',
        "ValueError",
    ),
    "empty_string_index": ('print("before")\nprint(""[0])\n', "IndexError"),
    "large_string_index": (
        'print("before")\nprint(("owned " + str(42))[1000000000000000000000000000000])\n',
        "IndexError",
    ),
    "negative_string_index": ('print("é🌍"[-3])\n', "IndexError"),
    "zero_string_step": (
        'print("before")\nprint(("owned " + str(42))[None:None:0])\n',
        "ValueError",
    ),
    "zero_list_step": (
        'print("before")\nvalues = [[1000000000000000000000000000000], [2]][::0]\n',
        "ValueError",
    ),
    "slice_receiver_failure": (
        'def receiver() -> str:\n    print("receiver")\n    raise ValueError("receiver failure")\ndef bound() -> int:\n    print("must not evaluate")\n    return 1\nprint(receiver()[bound():bound():bound()])\n',
        "ValueError",
    ),
    "enumerate_start_failure": (
        'def start() -> int:\n    print("start")\n    raise ValueError("start failure")\nfor index, character in enumerate("owned " + str(42), start()):\n    print("must not enter")\n',
        "ValueError",
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
    with tempfile.TemporaryDirectory(prefix="imre-expressions-") as directory:
        temporary = Path(directory)
        project = temporary / "project"
        shutil.copytree(FIXTURES, project, ignore=shutil.ignore_patterns("__pycache__"))
        for name, source, error in cases(project):
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
        f"Passed {checks} expression and sequence checks ({len(rejected)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
