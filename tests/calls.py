"""Compare direct keyword calls and scalar defaults with CPython and ASan."""

import argparse
import shutil
import sys
import tempfile
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import (
    reference,
    require_result,
    run,
    snapshot,
    write_files,
)

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "calls"
FUNCTION = "def add(first: int, second: int = 2) -> int:\n    return first + second\n"
KEYWORD_ONLY = (
    "def add(first: int, *, second: int) -> int:\n    return first + second\n"
)
POSITIONAL_ONLY = (
    "def add(first: int, /, second: int = 2) -> int:\n    return first + second\n"
)
CLASS = "class Item:\n    value: int\n    def __init__(self, value: int = 1) -> None:\n        self.value = value\n    def add(self, *, amount: int) -> int:\n        return self.value + amount\n"

REJECTED = {
    "forwarded_cached_target": 'from functools import cache\n@cache\ndef target(*, x: int = 1) -> int:\n    return x\ndef wrapper(**options) -> int:\n    return target(**options)\nprint(wrapper())\n',
    "forwarded_unknown_keyword": 'def target(*, x: int = 1) -> int:\n    return x\ndef wrapper(**options) -> int:\n    return target(**options)\nprint(wrapper(missing=2))\n',
    "forwarded_keyword_mapping_escapes": 'def target(*, x: int = 1) -> int:\n    return x\ndef wrapper(**options) -> int:\n    print(repr(options))\n    return target(**options)\n',
    "forwarded_callee_shadowed": 'def target(*, x: int = 1) -> int:\n    return x\ndef wrapper(target: int, **options) -> int:\n    return target(**options)\n',
    "forwarded_option_shadows_local": 'def target(*, x: int = 1) -> int:\n    return x\ndef wrapper(**options) -> int:\n    x = 3\n    return target(**options)\n',
    "forwarded_required_keyword": 'def target(*, x: int) -> int:\n    return x\ndef wrapper(**options) -> int:\n    print("entered")\n    return target(**options)\n',
    "unguarded_optional_unpack": FUNCTION + "def unchecked(row: tuple[int, int] | None) -> int:\n    return add(*row)\nprint(unchecked(None))\n",
    "invalidated_optional_unpack": FUNCTION + "def overwritten(row: tuple[int, int] | None) -> int:\n    if row is None:\n        return 0\n    row = None\n    return add(*row)\nprint(overwritten((1, 2)))\n",
    "spread_too_many": FUNCTION + "print(add(*(1, 2, 3, 4)))\n",
    "spread_wrong_type": FUNCTION + "print(add(*(\"a\", 2)))\n",
    "spread_keyword_conflict": 'def f(x: int) -> int:\n    return x\nprint(f(*(1,), x=2))\n',
    "mutated_keyword_mapping_remains_dynamic": 'def accept(x: int) -> str:\n    return "ok"\ndef f(x: int | None) -> str:\n    options = {"x": x}\n    options["x"] = 5\n    return accept(**options)\nprint(f(None))\n',
    "spread_keywords_stay_closed": 'def f(x: int) -> int:\n    return x\nkwargs = {"x": 2}\nprint(f(*(), **kwargs))\n',

    "unexpected_keyword": FUNCTION + "print(add(first=1, unknown=2))\n",
    "duplicate_positional_keyword": FUNCTION + "print(add(1, first=2))\n",
    "duplicate_keyword": FUNCTION + "print(add(first=1, first=2))\n",
    "missing_required_positional": FUNCTION + "print(add(second=2))\n",
    "missing_required_keyword": KEYWORD_ONLY + "print(add(1))\n",
    "too_many_positionals": FUNCTION + "print(add(1, 2, 3))\n",
    "keyword_only_positional": KEYWORD_ONLY + "print(add(1, 2))\n",
    "positional_only_keyword": POSITIONAL_ONLY + "print(add(first=1))\n",
    "positional_only_extra_keyword": POSITIONAL_ONLY + "print(add(1, first=2))\n",
    "bool_keyword_for_int": FUNCTION + "print(add(first=True))\n",
    "wrong_keyword_type": FUNCTION + 'print(add(first="wrong"))\n',
    "wrong_contextual_container": "def take(*, words: list[str]) -> None:\n    pass\ntake(words=[1])\n",
    "wrong_optional_keyword": "def take(*, value: int | None = None) -> None:\n    pass\ntake(value=False)\n",
    "starred_arguments": FUNCTION + "print(add(*[1, 2]))\n",
    "computed_keyword": FUNCTION + 'print(add(**{str("first"): 1}))\n',
    "untyped_variadic_parameter": "def take(*values) -> None:\n    pass\ntake(1)\n",
    "variadic_identity": "def take(*values: int) -> None:\n    print(values)\ntake(1)\n",
    "variadic_escape": "def take(*values: int) -> list[int]:\n    return values\n",
    "variadic_index": "def take(*values: int) -> int:\n    return values[0]\n",
    "variadic_slice": "def take(*values: int) -> list[int]:\n    return values[:]\n",
    "variadic_mutation": "def take(*values: int) -> None:\n    values.append(3)\n",
    "variadic_type": "def take(*values: int) -> None:\n    pass\ntake('wrong')\n",
    "expanded_duplicate": FUNCTION + 'print(add(1, **{"first": 2}))\n',
    "uninitialized_empty_pack": FUNCTION + "def run(flag: bool) -> int:\n    if flag:\n        keywords = {}\n    return add(1, **keywords)\nrun(False)\n",
    "untyped_keyword_rest": "def take(**values) -> None:\n    pass\ntake(first=1)\n",
    "wrong_keyword_rest_type": 'def take(**values: int) -> None:\n    pass\ntake(first="bad")\n',
    "list_default": "def take(values: list[int] = []) -> None:\n    pass\n",
    "dictionary_default": "def take(values: dict[str, int] = {}) -> None:\n    pass\n",
    "set_default": "def take(values: set[int] = {1}) -> None:\n    pass\n",
    "tuple_default": "def take(value: tuple[int, int] = (1, 2)) -> None:\n    pass\n",
    "call_default": 'def take(value: int = int("1")) -> None:\n    pass\n',
    "arithmetic_default": "def take(value: int = 1 + 2) -> None:\n    pass\n",
    "name_default": "default = 1\ndef take(value: int = default) -> None:\n    pass\n",
    "fstring_default": 'def take(value: str = f"literal") -> None:\n    pass\n',
    "unary_bool_default": "def take(value: int = -True) -> None:\n    pass\n",
    "bool_default_for_int": "def take(value: int = True) -> None:\n    pass\n",
    "int_default_for_bool": "def take(value: bool = 1) -> None:\n    pass\n",
    "string_default_for_int": 'def take(value: int = "1") -> None:\n    pass\n',
    "none_default_for_int": "def take(value: int = None) -> None:\n    pass\n",
    "bool_default_for_optional_int": "def take(value: int | None = False) -> None:\n    pass\n",
    "surrogate_default": 'def take(value: str = "\\ud800") -> None:\n    pass\n',
    "method_self_keyword": CLASS + "item = Item()\nitem.add(self=item, amount=2)\n",
    "constructor_self_keyword": CLASS + "Item(self=Item())\n",
    "constructor_unknown_keyword": CLASS + "Item(unknown=1)\n",
    "method_missing_required_keyword": CLASS + "Item().add()\n",
    "method_keyword_as_positional": CLASS + "Item().add(2)\n",
    "builtin_print_keyword": 'print(value="text")\n',
    "builtin_str_keyword": "print(str(object=1))\n",
    "builtin_append_keyword": "values: list[int] = []\nvalues.append(object=1)\n",
}

ERRORS = {
    "late_argument": (
        'def argument(number: int) -> int:\n    print(number)\n    if number == 2:\n        raise ValueError("argument failure")\n    return number\ndef target(first: int, second: int) -> int:\n    print("must not call")\n    return first + second\nprint(target(second=argument(1), first=argument(2)))\n',
        "ValueError",
    ),
    "receiver_failure": (
        'class Item:\n    def __init__(self) -> None:\n        pass\n    def take(self, *, value: int) -> None:\n        print(value)\ndef receiver() -> Item:\n    print("receiver")\n    raise ValueError("receiver failure")\ndef argument() -> int:\n    print("must not evaluate")\n    return 1\nreceiver().take(value=argument())\n',
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
    with tempfile.TemporaryDirectory(prefix="imre-calls-") as directory:
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
                if error is not None:
                    assert (
                        actual.stderr.splitlines()[-1]
                        == expected.stderr.splitlines()[-1]
                    ), (name, level, actual.stderr, expected.stderr)
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
            ), (name, result.stderr)
            checks += 1
    print(
        f"Passed {checks} call checks ({len(REJECTED)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
