"""Compare eager typed comprehensions with CPython, including failure cleanup."""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from textwrap import dedent

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "comprehensions"
BOOTSTRAP = (
    "import runpy, sys; root, entry = sys.argv[1:]; "
    "sys.path.insert(0, root); sys.argv = [entry]; "
    "runpy.run_path(entry, run_name='__main__')"
)

REJECTED = {
    "readonly_sequence_method": "from collections.abc import Sequence\ndef mutate(xs: Sequence[int]) -> None:\n    xs.append(2)  # ty: ignore[unresolved-attribute]\n",
    "readonly_sequence_alias": "from typing import Sequence\ndef leak(xs: Sequence[int]) -> None:\n    alias = xs\n    alias[0] = 1\n",
    "sequence_requires_list": "from typing import Sequence\ndef count(xs: Sequence[int]) -> int:\n    return len(xs)\nprint(count((1, 2)))\n",
    "empty_sequence_default_slice": "from typing import Sequence\ndef observe(xs: Sequence[int] = ()) -> str:\n    return repr(xs[:])\n",
    "empty_sequence_default_index": "from typing import Sequence\ndef observe(xs: Sequence[int] = ()) -> int:\n    return xs[0]\n",
    "sequence_element_contract": "from typing import Sequence\ndef count(xs: Sequence[int]) -> int:\n    return len(xs)\nprint(count(['s']))\n",
    "generator_expression": "values = (value for value in range(3))\n",
    "async_comprehension": "async def collect(values: list[int]) -> list[int]:\n    return [value async for value in values]\n",
    "bool_as_integer": "values: list[int] = [True for value in range(3)]\n",
    "wrong_context": "values: list[str] = [value for value in range(3)]\n",
    "wrong_nested_context": "values: list[list[str]] = [[value] for value in range(3)]\n",
    "ambiguous_empty_element": "values = [{} for value in range(0)]\n",
    "ambiguous_empty_value": "values = {value: set() for value in range(0)}\n",
    "heterogeneous_source": 'values = [value for value in [1, "wrong"]]\n',
    "bool_dict_key": "values = {True: value for value in range(3)}\n",
    "mutable_tuple_dict_key": "values = {(value, [value]): value for value in range(3)}\n",
    "bool_set_element": "values = {True for value in range(3)}\n",
    "list_set_element": "values = {[value] for value in range(3)}\n",
    "target_leaks": "values = [target for target in range(3)]\nprint(target)\n",
    "later_target_unbound_filter": "def bad() -> list[int]:\n    later = 7\n    return [later for first in range(1) if later > 0 for later in range(2)]\nbad()\n",
    "later_target_unbound_iterable": "def bad() -> list[int]:\n    later = 7\n    return [first for first in range(1) for later in range(later)]\nbad()\n",
    "target_type_rebinding": 'values = [value for value in [1] for value in ["wrong"]]\n',
    "optional_rebinding_loses_fact": "def bad(values: list[int | None]) -> list[int]:\n    return [value for value in values if value is not None for value in values]\nbad([1, None])\n",
    "optional_unchecked": "def bad(values: list[int | None]) -> list[int]:\n    return [value for value in values]\nbad([None])\n",
    "capture_uninitialized": "def bad(flag: bool) -> list[int]:\n    if flag:\n        offset = 1\n    return [offset + value for value in range(2)]\nbad(False)\n",
    "subscript_target": "source = [0]\nvalues = [source[0] for source[0] in range(3)]\n",
    "attribute_target": "class Item:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\nitem = Item(0)\nvalues = [item.value for item.value in range(3)]\n",
    "shadow_static_call": "def step(value: int) -> int:\n    return value + 1\nvalues = [step(1) for step in range(3)]\n",
    "unsupported_tuple_iterator": 'values = [value for value in ("abc",)]\n',
    "dynamic_dead_element": 'values = [eval("1") for value in range(0)]\n',
    "unused_dynamic_function": 'def unused() -> list[int]:\n    return [eval("1") for value in range(0)]\nprint(1)\n',
    "direct_helper_recursion": "def recursive(value: int) -> list[int]:\n    return [recursive(value)[0] for item in range(0)]\nrecursive(1)\n",
    "nested_helper_recursion": "def first(value: int) -> list[list[int]]:\n    return [[second(value) for inner in range(0)] for outer in range(0)]\ndef second(value: int) -> int:\n    return first(value)[0][0]\nfirst(1)\n",
    "method_helper_recursion": "class Item:\n    def __init__(self) -> None:\n        pass\n    def values(self) -> list[int]:\n        return [self.values()[0] for index in range(0)]\nItem().values()\n",
    "constructor_partial_self_body": "class Item:\n    values: list[int]\n    def __init__(self) -> None:\n        self.values = [len(self.values) for index in range(3)]\nItem()\n",
    "constructor_partial_self_iterable": "class Item:\n    values: list[int]\n    def __init__(self) -> None:\n        self.values = [self for self in self.values]\nItem()\n",
}

FIELD_CASE = """from dataclasses import dataclass
@dataclass
class Row:
    tag: str | None
def invalidate(row: Row) -> int:
    row.tag = None
    return 1
def select(rows: list[Row]) -> dict[int, str]:
"""
REJECTED["dict_key_invalidates_field"] = FIELD_CASE + "    return {invalidate(r): r.tag for r in rows if r.tag is not None}\n"
REJECTED["last_filter_invalidates_field"] = FIELD_CASE + "    return {0: r.tag for r in rows if r.tag is not None if invalidate(r)}\n"

IMPORTED_REJECTIONS = {
    "inactive_dynamic_comprehension": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": 'if __name__ == "__main__":\n    values = [eval("1") for value in range(0)]\n',
    },
    "unused_imported_helper_recursion": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": "def recursive(value: int) -> list[int]:\n    return [recursive(value)[0] for item in range(0)]\n",
    },
    "inactive_helper_recursion": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": "def first(value: int) -> list[int]:\n    return [second(value) for item in range(0)]\ndef second(value: int) -> int:\n    return first(value)[0]\nif __name__ == '__main__':\n    values = [first(index) for index in range(0)]\n",
    },
}

ERRORS = {
    "outer_iterable": (
        'def fail() -> list[int]:\n    print("outer")\n    raise ValueError("stop")\nvalues = [value for value in fail()]\n',
        "ValueError",
    ),
    "inner_iterable": (
        'print("before")\nvalues = [inner for outer in range(1) for inner in range(0, 2, 0) if False]\n',
        "ValueError",
    ),
    "filter_bounds": (
        'print("before")\nflags = [True]\nvalues = [value for value in range(3) if flags[value]]\n',
        "IndexError",
    ),
    "element_division": (
        "def element(value: int) -> int:\n    print(value)\n    return 10 // (2 - value)\nvalues = [element(value) for value in range(4)]\n",
        "ZeroDivisionError",
    ),
    "dictionary_value": (
        'def key(value: int) -> str:\n    print(value)\n    return "owned key " + str(value)\ndef item(value: int) -> list[int]:\n    print(value + 10)\n    if value == 2:\n        raise ValueError("value failed")\n    return [value]\nvalues = {key(value): item(value) for value in range(4)}\n',
        "ValueError",
    ),
    "nested_filter": (
        'def condition(value: int) -> bool:\n    print(value)\n    if value == 2:\n        raise ValueError("filter failed")\n    return True\nvalues = [[inner for inner in range(4) if condition(inner)] for outer in range(2)]\n',
        "ValueError",
    ),
}


def run(
    command: list[str], cwd: Path, *, timeout: int = 60, runner=subprocess.run
) -> subprocess.CompletedProcess[str]:
    return runner(
        command,
        cwd=cwd,
        input="",
        text=True,
        capture_output=True,
        timeout=timeout,
        env={
            **os.environ,
            "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1",
            "LSAN_OPTIONS": "exitcode=23",
        },
        check=False,
    )


def snapshot(directory: Path) -> dict[str, bytes | None]:
    """Compare all fixture-created files and directories outside the source tree."""
    result: dict[str, bytes | None] = {}
    for path in sorted(directory.rglob("*")):
        assert not path.is_symlink(), path
        result[str(path.relative_to(directory))] = (
            None if path.is_dir() else path.read_bytes()
        )
    return result


def require_result(result: subprocess.CompletedProcess[str], error: str | None) -> None:
    assert (
        "AddressSanitizer" not in result.stderr and "LeakSanitizer" not in result.stderr
    ), result
    if error is None:
        assert result.returncode == 0 and not result.stderr, (
            result.args,
            result.stdout,
            result.stderr,
        )
    else:
        assert result.returncode == 1, (result.args, result.returncode, result.stderr)
        lines = result.stderr.strip().splitlines()
        assert lines and (lines[-1] == error or lines[-1].startswith(error + ":")), (
            result.args,
            result.stderr,
        )


def write_files(directory: Path, files: dict[str, str]) -> None:
    for name, source in files.items():
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(dedent(source), encoding="utf-8")


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
    result = run(
        [python, "-I", "-B", "-c", BOOTSTRAP, str(source.parent), str(source)], cwd
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
        # Preserve the ld.lld lexical name used to select the linker flavor.
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags.append("--sanitize")
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-comprehensions-") as directory:
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
                command = [
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
                ]
                built = run(command, temporary)
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
        f"Passed {checks} comprehension checks ({len(REJECTED) + len(IMPORTED_REJECTIONS)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
