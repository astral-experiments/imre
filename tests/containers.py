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

# Projection-only tuple locals may be scalarized. These cases protect snapshot
# ownership, evaluation order, control-flow rebinding, and boxed escape paths.
ACCEPTED = {
    "tuple_scalar_snapshots": """
class Box:
    number: int
    name: str
    values: list[int]

    def __init__(self, number: int, name: str, values: list[int]) -> None:
        self.number = number
        self.name = name
        self.values = values


class Owner:
    box: Box

    def __init__(self, box: Box) -> None:
        self.box = box


def replace_owner(owner: Owner) -> int:
    owner.box = Box(8, "new", [0])
    return 0


def replace(box: Box, log: list[int]) -> int:
    log.append(4)
    box.values.append(9)
    box.number = 7
    box.name = "changed"
    box.values = [100]
    return 4


def snapshots(box: Box, log: list[int]) -> tuple[int, str, list[int], Box, int]:
    return (box.number, box.name, box.values, box, replace(box, log))


def first_argument(box: Box, log: list[int]) -> Box:
    log.append(1)
    return box


def second_argument(box: Box, log: list[int]) -> int:
    log.append(2)
    box.number = 2000000000000000000000000000000
    return 2


def event(log: list[int], value: int) -> int:
    log.append(value)
    return value


def projected(box: Box, ignored: int, log: list[int]) -> tuple[int, int]:
    return (box.number, event(log, 3))


def main() -> None:
    log: list[int] = []
    box = Box(1000000000000000000000000000000, "original", [5])
    captured = snapshots(box, log)
    print(captured[0])
    print(captured[1])
    print(captured[2][0])
    print(captured[2][1])
    captured[2].append(11)
    print(captured[2][-1])
    print(captured[3] == box)
    print(captured[3].values[0])
    print(captured[-1])
    print(len(captured))
    print(box.number)
    print(box.name)

    pair = projected(first_argument(box, log), second_argument(box, log), log)
    print(pair[0])
    # The second component is unused, but its effect must still occur.
    direct = (event(log, 5), event(log, 6))
    print(direct[0])

    # The first argument loses its last external root while evaluating the next.
    owner = Owner(Box(3000000000000000000000000000000, "temporary", [8]))
    temporary = projected(owner.box, replace_owner(owner), log)
    print(temporary[0])
    print(owner.box.number)
    for index in range(len(log)):
        print(log[index])


main()
""",
    "tuple_scalar_rebinding": """
def pair(name: str, value: int) -> tuple[str, int]:
    return (name, value)


def exercise(flag: bool) -> None:
    swapped = (1000000000000000000000000000001, 1000000000000000000000000000002)
    swapped = (swapped[1], swapped[0])
    print(swapped[0])
    print(swapped[1])
    names = ("left", "right")
    names = (names[1], names[0])
    print(names[0])
    print(names[-1])
    names = (names[0], names[1])
    print(names[0])

    if flag:
        current = pair("yes", 1000000000000000000000000000000)
    else:
        current = ("no", 2000000000000000000000000000000)
    for index in range(6):
        if index % 2 == 0:
            current = (current[0] + "x", current[1] + index)
        else:
            current = pair(current[0] + "y", current[1] + index)
        if index == 1:
            continue
        print(current[0])
        print(current[1])
        if index == 4:
            break
    remaining = 2
    while remaining > 0:
        current = pair(current[0] + "z", current[1] + remaining)
        remaining -= 1
    print(current[0])
    print(current[1])
    print(len(current))
    if flag:
        return
    current = ("last", current[1])
    print(current[0])


exercise(True)
exercise(False)
""",
    "tuple_scalar_boxed_boundaries": """
class Holder:
    pair: tuple[list[int], str]

    def __init__(self, pair: tuple[list[int], str]) -> None:
        self.pair = pair


def make_pair(value: int) -> tuple[list[int], str]:
    return ([value], "saved")


def identity(pair: tuple[list[int], str]) -> tuple[list[int], str]:
    return pair


def main() -> None:
    original = make_pair(1000000000000000000000000000000)
    alias = original
    returned = identity(alias)
    original[0].append(2)
    print(returned[0][0])
    print(returned[0][-1])
    print(returned[1])

    pairs: list[tuple[list[int], str]] = [make_pair(3)]
    held = pairs[0]
    pairs[0] = make_pair(4)
    print(held[0][0])
    print(pairs[0][0][0])
    popped = pairs.pop()
    print(popped[1])

    holder = Holder(make_pair(5))
    old = holder.pair
    holder.pair = make_pair(6)
    print(old[0][0])
    print(holder.pair[0][0])

    nested = (make_pair(7), 8)
    print(nested[0][0][0])
    print(nested[0][1])
    print(nested[-1])

    optional: tuple[list[int], str] | None = make_pair(9)
    if optional is not None:
        kept = optional
        optional = None
        print(kept[0][0])
        print(kept[1])
    print(optional is None)

    # Rebinding the container field cannot change an earlier scalar snapshot.
    snapshot = (holder.pair[0], holder.pair[1])
    holder.pair = make_pair(10)
    print(snapshot[0][0])
    print(snapshot[1])
    print(holder.pair[0][0])


main()
""",
}

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
    "tuple_unused_element_error": (
        'def broken() -> tuple[int, int]:\n    return (7, 1 // 0)\n'
        'print("before")\npair = broken()\nprint(pair[0])\n',
        "ZeroDivisionError",
    ),
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
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    build_flags = []
    if args.lto_linker is not None:
        build_flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.target is not None:
        build_flags += ["--target", args.target]
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
        cases.extend((name, source, None) for name, source in ACCEPTED.items())
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
                command += build_flags
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
