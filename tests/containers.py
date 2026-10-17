"""Check typed container semantics against CPython at O0/O3, optionally with ASan."""

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from textwrap import dedent

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "containers"
ITEM = """
class Item:
    name: str
    def __init__(self, name: str) -> None:
        self.name = name
"""

# Every rejected program must reach the checker. Cycle cases additionally require
# the ownership-graph diagnostic, rather than an unsupported-annotation rejection.
REJECTED = {
    "mixed_elements": 'values = ["text", 1]\n',
    "wrong_string_append": "values: list[str] = []\nvalues.append(1)\n",
    "wrong_string_replace": 'values: list[str] = ["x"]\nvalues[0] = False\n',
    "bool_is_not_int": "values: list[bool] = []\nvalues.append(1)\n",
    "int_is_not_bool": "values: list[int] = []\nvalues.append(True)\n",
    "wrong_alias_append": "values: list[str] = []\nalias = values\nalias.append(1)\n",
    "wrong_nested_alias_write": 'rows: list[list[str]] = [["x"]]\nalias = rows[0]\nalias[0] = 1\n',
    "wrong_nested_append": "rows: list[list[str]] = []\nrows.append([1])\n",
    "wrong_empty_context": "def values() -> list[str]:\n    return [[], []]\n",
    "invariant_lists": """
        def append_none(values: list[str | None]) -> None:
            values.append(None)
        values: list[str] = ["x"]
        append_none(values)
    """,
    "wrong_nominal_element": ITEM
    + """
class Other:
    name: str
    def __init__(self, name: str) -> None:
        self.name = name
items: list[Item] = []
items.append(Other("same layout"))
""",
    "wrong_nominal_alias": ITEM
    + """
items: list[Item] = [Item("x")]
alias = items
alias[0] = "wrong"
""",
    "variable_tuple_index": 'pair: tuple[str, int] = ("x", 1)\nindex = 0\nprint(pair[index])\n',
    "tuple_mutation": 'pair: tuple[str, int] = ("x", 1)\npair[0] = "y"\n',
    "tuple_alias_mutation": 'pair: tuple[str, int] = ("x", 1)\nalias = pair\nalias[-1] = 2\n',
    "tuple_positive_bounds": 'pair: tuple[str, int] = ("x", 1)\nprint(pair[2])\n',
    "tuple_negative_bounds": 'pair: tuple[str, int] = ("x", 1)\nprint(pair[-3])\n',
    "wrong_tuple_return": 'def pair() -> tuple[str, int]:\n    return (1, "x")\n',
    "optional_field_without_narrowing": ITEM
    + """
def show(item: Item | None) -> None:
    print(item.name)
show(None)
""",
    "optional_element_without_narrowing": ITEM
    + """
items: list[Item | None] = [None]
print(items[0].name)
""",
    "optional_arithmetic_without_narrowing": "def add(value: int | None) -> int:\n    return value + 1\n",
    "optional_wrong_argument": "def show(value: str | None) -> None:\n    pass\nshow(1)\n",
    "optional_rebind_invalidates_narrowing": ITEM
    + """
def show(item: Item | None) -> None:
    if item is not None:
        item = None
        print(item.name)
show(Item("x"))
""",
    "optional_or_true_has_no_fact": """
        def use(value: int | None, flag: bool) -> int:
            if value is not None or flag:
                return value + 1
            return 0
        use(None, True)
    """,
    "optional_and_false_has_no_fact": """
        def use(value: int | None, flag: bool) -> int:
            if value is None and flag:
                return 0
            return value + 1
        use(None, False)
    """,
    "optional_boolean_fact_does_not_escape": """
        def use(value: int | None) -> int:
            positive = value is not None and value > 0
            return value + 1
        use(None)
    """,
    "optional_for_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                for index in range(2):
                    print(value + 1)
                    value = None
        use(1)
    """,
    "optional_while_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                index = 0
                while index < 2:
                    print(value + 1)
                    value = None
                    index += 1
        use(1)
    """,
    "optional_while_condition_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                while value > 0:
                    value = None
        use(1)
    """,
    "optional_nested_branch_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                for index in range(2):
                    print(value + 1)
                    if index == 0:
                        value = None
        use(1)
    """,
    "optional_nested_loop_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                for outer in range(2):
                    print(value + 1)
                    for inner in range(1):
                        value = None
        use(1)
    """,
    "optional_for_exit": """
        def use(value: int | None) -> int:
            if value is not None:
                for index in range(1):
                    value = None
                return value + 1
            return 0
        use(1)
    """,
    "optional_while_exit": """
        def use(value: int | None) -> int:
            while value is not None:
                value = None
            return value + 1
        use(1)
    """,
    "optional_guarded_subscript": """
        def use(values: list[int | None]) -> int:
            if values[0] is not None:
                return values[0] + 1
            return 0
        use([1])
    """,
    "field_narrowing_across_call": ITEM
    + """
class Box:
    item: Item | None
    def __init__(self, item: Item | None) -> None:
        self.item = item
def clear(box: Box) -> None:
    box.item = None
def show(box: Box) -> None:
    if box.item is not None:
        clear(box)
        print(box.item.name)
show(Box(Item("x")))
""",
    "list_class_cycle": """
        class Node:
            children: list["Node"]
            def __init__(self, children: list["Node"]) -> None:
                self.children = children
    """,
    "tuple_class_cycle": """
        class Node:
            edge: tuple["Node", int]
            def __init__(self, edge: tuple["Node", int]) -> None:
                self.edge = edge
    """,
    "optional_class_cycle": """
        class Node:
            edge: "Node | None"
            def __init__(self, edge: "Node | None") -> None:
                self.edge = edge
    """,
    "nested_container_class_cycle": """
        class Node:
            edges: list[tuple[list["Node"], int]]
            def __init__(self, edges: list[tuple[list["Node"], int]]) -> None:
                self.edges = edges
    """,
}

ERRORS = {
    "string_bounds": ('values: list[str] = ["x"]\nprint(values[1])\n', "IndexError"),
    "string_negative_bounds": (
        'values: list[str] = ["x"]\nprint(values[-2])\n',
        "IndexError",
    ),
    "string_huge_bounds": (
        'values: list[str] = ["x"]\nprint(values[10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000])\n',
        "IndexError",
    ),
    "empty_bool_pop": ("values: list[bool] = []\nprint(values.pop())\n", "IndexError"),
    "empty_nested_pop": ("values: list[list[str]] = []\nvalues.pop()\n", "IndexError"),
    "object_bounds": (
        ITEM + 'items: list[Item] = [Item("x")]\nprint(items[-2].name)\n',
        "IndexError",
    ),
}


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        text=True,
        capture_output=True,
        check=False,
        timeout=60,
        env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    compiler = str(args.compiler.resolve())
    runtime = str(args.runtime.resolve())
    passed = 0
    with tempfile.TemporaryDirectory(prefix="relar-containers-") as directory:
        temporary = Path(directory)
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(dedent(source), encoding="utf-8")
            checked = run([compiler, "check", str(path)])
            assert checked.returncode > 0 and checked.stderr, (name, checked)
            if name.endswith("class_cycle"):
                assert "cyclic heap types" in checked.stderr, (name, checked.stderr)
            passed += 1

        cases = [
            (path.stem, path.read_text(encoding="utf-8"), None)
            for path in sorted(FIXTURES.glob("*.py"))
        ]
        assert cases, f"No fixtures found in {FIXTURES}"
        cases.extend((name, source, error) for name, (source, error) in ERRORS.items())
        for name, source, expected_error in cases:
            path = temporary / f"{name}.py"
            path.write_text(source, encoding="utf-8")
            reference = run([args.python, "-I", str(path)])
            if expected_error:
                assert (
                    reference.returncode > 0 and expected_error in reference.stderr
                ), (name, reference)
            else:
                assert reference.returncode == 0 and not reference.stderr, (
                    name,
                    reference,
                )
            for level in args.opt_levels:
                binary = temporary / f"{name}-O{level}"
                command = [
                    compiler,
                    "build",
                    str(path),
                    "-o",
                    str(binary),
                    "--runtime",
                    runtime,
                    "--opt-level",
                    level,
                    "--clang",
                    args.clang,
                ]
                if args.sanitize:
                    command.append("--sanitize")
                built = run(command)
                assert built.returncode == 0, (name, level, built.stderr)
                actual = run([str(binary)])
                assert actual.stdout == reference.stdout, (
                    name,
                    level,
                    actual.stdout,
                    reference.stdout,
                    actual.stderr,
                )
                if expected_error:
                    assert actual.returncode > 0 and actual.stderr.startswith(
                        expected_error + ":"
                    ), (name, level, actual)
                else:
                    assert actual.returncode == 0 and not actual.stderr, (
                        name,
                        level,
                        actual,
                    )
                assert "AddressSanitizer" not in actual.stderr, (
                    name,
                    level,
                    actual.stderr,
                )
                passed += 1
    print(
        f"Passed {passed} container checks ({len(REJECTED)} rejected programs; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
