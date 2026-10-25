"""Compare sealed scalar f-strings with CPython, including ownership and file effects."""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# The validation jobs run this script with -I, which omits its directory from
# sys.path. Load only the trusted sibling harness, never the execution directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import (
    reference,
    require_result,
    run,
    snapshot,
    write_files,
)

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "fstrings"

# Literal whitespace is data, while expression/code indentation remains checked.
ACCEPTED = {
    "shadowed_ascii": 'def ascii(value: str) -> str:\n    return "shadow: " + value\nprint(ascii("é"))\nprint(f"{\'é\'!a}")\n',
    "ascii_conversion": 'print(f"{1!a}")\n',
    "repr_conversion": 'print(f"{1!r}:{True!r}:{None!r}")\n',
    "literal_control_characters": 'value = 7\nprint(f"""first\n\tliteral tab {value}\n\fliteral form feed\nlast""")\n',
    "nested_literal_whitespace": 'print(f"""outer:{"""inner\n\ttext\n\fform feed"""}:end""")\n',
}

REJECTED = {
    "descriptor_user_repr": 'class Item:\n    def __repr__(self) -> str:\n        return "é"\nprint(repr([Item()]))\n',
    "nested_user_repr": 'class Item:\n    def __repr__(self) -> str:\n        return "é"\nprint(ascii([Item()]))\n',
    "ascii_format_spec": 'print(f"{1!a:+d}")\n',
    "debug_field": 'value = 1\nprint(f"{value=}")\n',
    "debug_str_conversion": 'value = 1\nprint(f"{value=!s}")\n',
    "debug_empty_spec": 'value = 1\nprint(f"{value=:}")\n',
    "integer_width_spec": 'print(f"{1:+5d}")\n',
    "decimal_float": 'print(f"{1.5:+}")\n',
    "decimal_conversion": 'print(f"{1!r:+d}")\n',
    "string_format_spec": "print(f\"{'text':s}\")\n",
    "dynamic_empty_format_spec": 'spec = ""\nprint(f"{1:{spec}}")\n',
    "literal_empty_dynamic_spec": "print(f\"{1:{''}}\")\n",
    "nested_empty_dynamic_spec": "print(f\"{1:{f''}}\")\n",
    "excessive_precision_spec": 'print(f"{1:.1001f}")\n',
    "numeric_string_conversion": 'print(f"{1!s:.2f}")\n',
    "numeric_string_field": "print(f\"{'text':.2f}\")\n",
    "dict_field": 'values = {1: 2}\nprint(f"{values}")\n',
    "tuple_field": 'print(f"{(1, 2)}")\n',
    "optional_integer_field": 'def render(value: int | None) -> str:\n    return f"{value}"\nprint(render(1))\n',
    "builtin_rebinding": 'def render() -> None:\n    str = 101\n    print(f"{str!s}")\nrender()\n',
    "surrogate_literal_span": 'print(f"\\uD800{1}")\n',
    "surrogate_long_escape": 'print(f"{1}\\U0000DFFF")\n',
    "surrogate_literal_without_field": 'print(f"\\uD800")\n',
    "surrogate_adjacent_ordinary": 'print(rf"{1}" "\\uD800")\n',
    "surrogate_inner_ordinary": "print(rf\"{'\\uD800'}\")\n",
    "surrogate_inner_fstring": "print(rf\"{f'\\uD800{1}'}\")\n",
    "tab_expression_indentation": 'print(f"""{\n\t1\n}""")\n',
    "form_feed_expression_indentation": 'print(f"""{\n\f1\n}""")\n',
    "tab_code_indentation": 'if True:\n\tprint(f"{1}")\n',
    "unused_unsupported_field": 'def unused() -> str:\n    return f"{(1, 2)}"\nprint(1)\n',
    "unused_dynamic_field": "def unused() -> str:\n    return f\"{eval('1')}\"\nprint(1)\n",
}

ERRORS = {
    "late_field": (
        'def field(number: int) -> int:\n    print(number)\n    if number == 2:\n        raise ValueError("late field")\n    return number\nprint(f"{field(1)}:{field(2)}:{field(3)}")\n',
        "ValueError",
    ),
    "missing_item": (
        'values = {"present": "owned value"}\nprint("before")\nprint(f"{values[\'present\']}:{values[\'missing\']}")\n',
        "KeyError",
    ),
}


def rejection_message(name: str) -> str:
    if name in ("decimal_float", "decimal_conversion"):
        return "decimal integer formats require an unconverted int or bool"
    if name in ("numeric_string_conversion", "numeric_string_field"):
        return "numeric format specifications require an unconverted int, float, or bool"
    if name == "builtin_rebinding":
        return "rebinding a builtin, definition, or imported module is unsupported"
    if name.startswith("debug_"):
        return "debug f-string fields are unsupported"
    if name == "nested_user_repr":
        return "ascii requires a supported representation"
    if name == "descriptor_user_repr":
        return "generated repr cannot format"
    if name == "ascii_format_spec":
        return "formatted strings support !a with an empty format specification"
    if name.endswith("_spec"):
        if "dynamic" in name:
            return "f-string format specifications must be literal"
        return "numeric formats support signed decimal integers or f and % with literal precision from 0 through 1000"
    if name.startswith("surrogate_"):
        return "surrogate string escapes are unsupported"
    if name.endswith("_indentation"):
        return "indentation must use spaces, not tabs or form feeds"
    if name == "unused_dynamic_field":
        return ""
    return "unsupported value type for default f-string conversion"


def integer_limits(python: str, source: Path, binary: Path, cwd: Path) -> int:
    # Each invocation starts a fresh interpreter. In particular malformed values
    # must exit before executing the first print in the fixture.
    settings = ("0", "640", "4301", "2147483647", "640 ", "-1")
    for setting in settings:
        env = {
            **os.environ,
            "PYTHONINTMAXSTRDIGITS": setting,
            "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1",
            "LSAN_OPTIONS": "exitcode=23",
        }

        def output(argv: list[str]) -> tuple[int, str, str]:
            result = subprocess.run(
                argv, cwd=cwd, env=env, capture_output=True, text=True, timeout=60
            )
            return result.returncode, result.stdout, result.stderr

        expected = output([python, "-B", str(source)])
        actual = output([str(binary)])
        assert actual == expected, (setting, expected, actual)
    return len(settings)


def cases(project: Path) -> list[tuple[str, Path, str | None]]:
    result = [(path.stem, path, None) for path in sorted(project.glob("*.py"))]
    result.append(("imported", project / "imported/main.py", None))
    for name, source in ACCEPTED.items():
        path = project / "generated" / name / "main.py"
        write_files(path.parent, {"main.py": source})
        result.append((name, path, None))
    for name, (source, error) in ERRORS.items():
        path = project / "errors" / name / "main.py"
        write_files(path.parent, {"main.py": source})
        result.append((name, path, error))
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
    with tempfile.TemporaryDirectory(prefix="imre-fstrings-") as directory:
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
                if name == "ascii_integer_limit":
                    checks += integer_limits(python, source, binary, actual_dir)
                actual = run([str(binary)], actual_dir)
                require_result(actual, error)
                assert actual.stdout == expected.stdout, (
                    name,
                    level,
                    actual.stdout,
                    expected.stdout,
                    actual.stderr,
                )
                if error is not None:
                    assert (
                        actual.stderr.splitlines()[-1]
                        == expected.stderr.splitlines()[-1]
                    ), (
                        name,
                        level,
                        actual.stderr,
                        expected.stderr,
                    )
                assert snapshot(actual_dir) == expected_files, (
                    name,
                    level,
                    snapshot(actual_dir),
                    expected_files,
                )
                checks += 1
        for name, source in REJECTED.items():
            case = temporary / "rejected" / name
            write_files(case, {"main.py": source})
            result = run([compiler, "check", str(case / "main.py")], temporary)
            assert result.returncode > 0 and result.stderr, (name, result)
            assert (
                "panicked" not in result.stderr
                and "AddressSanitizer" not in result.stderr
            ), (
                name,
                result.stderr,
            )
            assert rejection_message(name) in result.stderr, (name, result.stderr)
            checks += 1
    print(
        f"Passed {checks} f-string checks ({len(REJECTED)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
