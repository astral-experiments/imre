"""Check typed container semantics against CPython at O0/O3, optionally with ASan."""

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from textwrap import dedent

sys.path.insert(0, str(Path(__file__).resolve().parent))
import comparison_allocation
import deferred_diff_allocation

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
    "dict_copy_constructor": 'original = {"one": 1}\nvalues: dict[str, int] = dict(original)\n',
    "set_literal_expansion": "values = {1}\nexpanded = {*values}\nexpanded.add(2)\nprint(len(values))\nprint(1 in expanded)\nprint(2 in expanded)\n",
    "dict_literal_expansion": 'values = {"one": 1}\ncopy = {**values}\ncopy["one"] = 2\nprint(values["one"])\nprint(copy["one"])\n',
    "dict_values_iteration": """
rows = {"first": [1], "second": [2]}
for row in rows.values():
    rows["second"] = [3]
    print(row[0])
""",
    "iteration_list_unpacking": "first, second = [1, 2]\nprint(first)\nprint(second)\n",
    "iteration_starred_unpacking": "first, *rest = (1, 2, 3)\nprint(first)\nprint(rest[0])\nprint(rest[1])\n",
    "iteration_mutable_unpacking_leaf": "values = [0]\nvalues[0], other = (1, 2)\nprint(values[0])\nprint(other)\n",
    "dict_contextual_empty": """
def empty() -> dict[str, list[int]]:
    return {}

def constructed() -> dict[str, list[int]]:
    return dict()

def use(values: dict[str, int]) -> None:
    values["number"] = 7
    print(values["number"])

def main() -> None:
    use({})
    use(dict())
    created: dict[str, int] = dict()
    print(len(created))
    print(len(constructed()))
    rows = empty()
    rows["row"] = []
    rows["row"].append(8)
    print(rows["row"][0])
    nested: dict[str, dict[int, int]] = {"inner": {}}
    nested["inner"][1] = 9
    print(nested["inner"][1])
    maps: list[dict[str, int]] = [{}]
    maps[0]["value"] = 10
    print(maps[0]["value"])
    pair: tuple[dict[str, int], int] = ({}, 11)
    pair[0]["value"] = pair[1]
    print(pair[0]["value"])
    fallback = nested.get("missing", {})
    print(len(fallback))

main()
""",
    "dict_literal_evaluation": """
def key(log: list[int], event: int) -> str:
    log.append(event)
    return "duplicate"

def value(log: list[int], event: int) -> int:
    log.append(event)
    return event

log: list[int] = []
values = {key(log, 1): value(log, 2), key(log, 3): value(log, 4)}
print(len(values))
print(values["duplicate"])
for event in log:
    print(event)
""",
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
    "deepcopy_explicit_memo": "from copy import deepcopy\nx = [1]\nmemo: dict[int, list[int]] = {}\ny = deepcopy(x, memo)\n",
    "deepcopy_live_resource": "from copy import deepcopy\nwith open('input.txt') as f:\n    x = deepcopy([f])\n",
    "deepcopy_unproved_cycle": "from copy import deepcopy\nx = []\nx.append(x)\ny = deepcopy(x)\n",
    "namedtuple_duplicate_field": 'from collections import namedtuple\nPair = namedtuple("Pair", "x x")\n',
    "namedtuple_private_field": 'from collections import namedtuple\nPair = namedtuple("Pair", "_x")\n',
    "namedtuple_unchecked_options": 'from collections import namedtuple\nPair = namedtuple("Pair", "x", rename=True)\n',
    "namedtuple_missing_argument": 'from collections import namedtuple\nPair = namedtuple("Pair", "x y")\nprint(Pair(1))\n',
    "namedtuple_repeated_argument": 'from collections import namedtuple\nPair = namedtuple("Pair", "x")\nprint(Pair(1, x=2))\n',
    "namedtuple_readonly_field": 'from collections import namedtuple\nPair = namedtuple("Pair", "x")\np = Pair(1)\np.x = 2\n',
    "namedtuple_wrong_annotated_class": 'from collections import namedtuple\nA = namedtuple("A", "x")\nB = namedtuple("B", "x")\ndef f() -> A:\n    return B(1)\nprint(f().x)\n',

    "counter_wrong_key": "from collections import Counter\nc = Counter[int]()\nc.update(['str'])",
    "counter_wrong_count": "from collections import Counter\nc = Counter({'key': 'not an int'})",
    "counter_wrong_generic": "from collections import Counter\nc = Counter[bytes]()",
    "counter_plain_dict": "from collections import Counter\nc: Counter[str] = {'key': 1}",
    "counter_schema": """
        from collections import Counter
        from dataclasses import dataclass
        from imre.json import decode
        @dataclass
        class Box:
            counts: Counter[str]
        box = decode("{}", Box)
    """,
    "set_empty_without_context": "values = set()\n",
    "set_iterable_constructor": "values: set[int] = set([1, 2])\n",
    "set_copy_wrong_element": "original = {1, 2}\nvalues: set[str] = set(original)\n",
    "set_bool_annotation": "values: set[bool] = set()\n",
    "set_bool_literal": "values = {True, False}\n",
    "set_bool_int_literal": "values = {1, True}\n",
    "set_mixed_literal": 'values = {1, "one"}\n',
    "set_list_elements": "values: set[list[int]] = set()\n",
    "set_mutable_tuple_elements": "values: set[tuple[int, list[int]]] = set()\n",
    "set_optional_elements": "values: set[int | None] = set()\n",
    "set_none_elements": "values: set[None] = set()\n",
    "set_nominal_elements": ITEM + "values: set[Item] = set()\n",
    "set_recursive_nominal_elements": """
        class Node:
            children: set["Node"]
            def __init__(self, children: set["Node"]) -> None:
                self.children = children
    """,
    "set_wrong_add": 'values = {"one"}\nvalues.add(1)\n',
    "set_wrong_alias_add": 'values = {"one"}\nalias = values\nalias.add(1)\n',
    "set_bool_int_add": "values = {1}\nvalues.add(True)\n",
    "set_wrong_discard": 'values = {"one"}\nvalues.discard(1)\n',
    "set_bool_int_discard": "values = {1}\nvalues.discard(True)\n",
    "set_wrong_membership": 'values = {"one"}\nprint(1 in values)\n',
    "set_bool_int_membership": "values = {1}\nprint(True in values)\n",
    "set_add_arity": "values = {1}\nvalues.add(2, 3)\n",
    "set_discard_arity": "values = {1}\nvalues.discard()\n",
    "set_add_keyword": "values = {1}\nvalues.add(element=2)\n",
    "set_copy_method": "values = {1}\ncopy = values.copy()\n",
    "set_pop_method": "values = {1}\nvalues.pop()\n",
    "set_remove_method": "values = {1}\nvalues.remove(1)\n",
    "set_union_mismatched_keys": 'values = {1} | {"two"}\n',
    "dict_keys_union_mismatched": 'values = {"one": 1}.keys() | {2: "two"}.keys()\n',
    "dict_keys_view_escape": 'values = {"one": 1}\nview = values.keys()\n',
    "defaultdict_requires_factory": "from collections import defaultdict\na: defaultdict[str, list[int]] = defaultdict()\n",
    "defaultdict_other_factory": "from collections import defaultdict\na: defaultdict[str, list[int]] = defaultdict(set)\n",
    "defaultdict_wrong_value": "from collections import defaultdict\na: defaultdict[str, int] = defaultdict(list)\n",
    "defaultdict_plain_dict": "from collections import defaultdict\na: defaultdict[str, list[int]] = dict()\n",
    "defaultdict_wrong_entry": 'from collections import defaultdict\na: defaultdict[str, list[int]] = defaultdict(list)\na["a"].append("bad")\n',
    "set_mixed_equality": 'left = {1}\nright = {"1"}\nprint(left == right)\n',
    "set_subset_comparison": "left = {1}\nright = {1, 2}\nprint(left < right)\n",
    "set_index": "values = {1}\nprint(values[0])\n",
    "set_index_write": "values = {1}\nvalues[0] = 2\n",
    "set_first_class_iterator": "values = {1}\niterator = iter(values)\n",
    "set_first_class_enumerate": "values = {1}\niterator = enumerate(values)\n",
    "set_optional_receiver": "def add(values: set[int] | None) -> None:\n    values.add(1)\n",
    "set_optional_membership": "def has(values: set[int] | None) -> bool:\n    return 1 in values\n",
    "set_maybe_unbound_target": """
        def last(values: set[int]) -> int:
            for value in values:
                pass
            return value
    """,
    "set_optional_target_invalidates_fact": """
        def use(values: set[int] | None, sources: list[set[int] | None]) -> None:
            if values is not None:
                for values in sources:
                    print(len(values))
    """,
    "set_optional_loop_backedge": """
        def use(values: set[int] | None) -> None:
            if values is not None:
                for index in {1, 2}:
                    print(len(values))
                    values = None
    """,
    "set_optional_loop_exit": """
        def use(values: set[int] | None) -> None:
            if values is not None:
                for index in {1}:
                    values = None
                print(len(values))
    """,
    "dict_empty_without_context": "values = {}\n",
    "dict_empty_constructor_without_context": "values = dict()\n",
    "dict_bool_key_annotation": "values: dict[bool, int] = {}\n",
    "dict_list_key_annotation": "values: dict[list[int], int] = {}\n",
    "dict_mutable_tuple_key_annotation": "values: dict[tuple[int, list[int]], int] = {}\n",
    "dict_mixed_literal_keys": 'values = {1: 2, "one": 3}\n',
    "dict_mixed_literal_values": 'values = {1: 2, 3: "four"}\n',
    "dict_bool_int_literal_values": "values = {1: 2, 3: True}\n",
    "dict_wrong_key_write": "values: dict[str, int] = {}\nvalues[1] = 2\n",
    "dict_bool_int_key_write": "values: dict[int, int] = {}\nvalues[True] = 2\n",
    "dict_bool_int_key_read": "values = {1: 2}\nprint(values[True])\n",
    "dict_bool_int_membership": "values = {1: 2}\nprint(True in values)\n",
    "dict_wrong_key_delete": 'values = {1: 2}\ndel values["one"]\n',
    "dict_wrong_value_write": 'values = {"one": 1}\nvalues["one"] = "two"\n',
    "dict_wrong_alias_write": 'values = {"one": 1}\nalias = values\nalias["one"] = False\n',
    "dict_wrong_nested_alias_write": 'values: dict[str, list[int]] = {"row": [1]}\nalias = values["row"]\nalias.append("bad")\n',
    "dict_wrong_get_default": 'values = {"one": 1}\nprint(values.get("one", "wrong"))\n',
    "dict_bool_int_get_default": 'values = {"one": 1}\nprint(values.get("one", True))\n',
    "dict_none_get_wrong_default": 'values: dict[str, None] = {"none": None}\nvalues.get("none", 1)\n',
    "dict_get_wrong_key": 'values = {"one": 1}\nvalues.get(1)\n',
    "dict_get_wrong_arity": 'values = {"one": 1}\nvalues.get("one", 2, 3)\n',
    "dict_get_keyword": 'values = {"one": 1}\nvalues.get("one", default=2)\n',
    "dict_get_without_narrowing": 'values = {"one": 1}\nrequired: int = values.get("one")\n',
    "dict_get_repeated_call_narrowing": 'values = {"one": 1}\nif values.get("one") is not None:\n    required: int = values.get("one")\n',
    "dict_optional_element_without_narrowing": 'values: dict[str, int | None] = {"one": 1}\nrequired: int = values["one"]\n',
    "dict_optional_receiver": "def show(values: dict[int, int] | None) -> None:\n    print(values[1])\n",
    "dict_string_augmented_assignment": 'values = {1: "one"}\nvalues[1] += "two"\n',
    "dict_items_escapes": 'values = {"one": 1}\nview = values.items()\n',
    "dict_items_arguments": 'values = {"one": 1}\nfor key, value in values.items(1):\n    pass\n',
    "dict_keys_view": 'values = {"one": 1}\nfor key in values.keys():\n    pass\n',
    "dict_values_view_escape": 'values = {"one": 1}\nview = values.values()\n',
    "dict_first_class_iterator": 'values = {"one": 1}\niterator = iter(values)\n',
    "dict_wrong_items_target": 'values = {"one": 1}\nfor key, value, extra in values.items():\n    pass\n',
    "dict_invariant_values": """
        def use(values: dict[str, int | None]) -> None:
            values["none"] = None
        values: dict[str, int] = {"one": 1}
        use(values)
    """,
    "dict_class_cycle": """
        class Node:
            edges: dict[str, "Node"]
            def __init__(self, edges: dict[str, "Node"]) -> None:
                self.edges = edges
    """,
    "dict_optional_class_cycle": """
        class Node:
            edges: dict[int, "Node | None"]
            def __init__(self, edges: dict[int, "Node | None"]) -> None:
                self.edges = edges
    """,
    "iteration_maybe_unbound_target": """
        def last(values: list[int]) -> int:
            for item in values:
                pass
            return item
    """,
    "iteration_maybe_unbound_enumerate_target": """
        def last(values: list[str]) -> str:
            for index, item in enumerate(values):
                pass
            return item
    """,
    "iteration_maybe_unbound_tuple_target": """
        def last(values: list[tuple[int, str]]) -> int:
            for number, name in values:
                pass
            return number
    """,
    "iteration_target_type_mismatch": 'value = "text"\nfor value in [1]:\n    pass\n',
    "iteration_tuple_target_arity": "for first, second in [(1, 2, 3)]:\n    pass\n",
    "iteration_range_tuple_target": "for (value,) in range(2):\n    pass\n",
    "iteration_heterogeneous_tuple": 'for value in (1, "two"):\n    print(value)\n',
    "iteration_optional_iterable": """
        def show(values: list[int] | None) -> None:
            for value in values:
                print(value)
    """,
    "iteration_enumerate_wrong_start": 'for index, value in enumerate([1], "bad"):\n    pass\n',
    "iteration_enumerate_bool_start": "for index, value in enumerate([1], True):\n    pass\n",
    "iteration_enumerate_arity": "for index, value in enumerate([1], 0, 1):\n    pass\n",
    "iteration_enumerate_keyword": "for index, value in enumerate([1], start=2):\n    pass\n",
    "iteration_enumerate_escapes": "iterator = enumerate([1])\n",
    "iteration_nested_wrong_arity": 'first, (second, third) = (1, ("two",))\n',
    "iteration_repeated_name_type_mismatch": 'same, same = (1, "two")\n',
    "iteration_rhs_precedes_target_declaration": "first, second = (1, first)\n",
    "iteration_nested_rhs_precedes_target_declaration": "first, (second, third) = (1, (2, first))\n",
    "iteration_optional_target_invalidates_fact": """
        def show(value: int | None, values: list[int | None]) -> None:
            if value is not None:
                for value in values:
                    required: int = value
    """,
    "iteration_optional_tuple_target_invalidates_fact": """
        def show(value: int | None, values: list[tuple[int | None, int]]) -> None:
            if value is not None:
                for value, index in values:
                    required: int = value
    """,
    "iteration_optional_unpacking_backedge": """
        def show(value: int | None) -> None:
            if value is not None:
                for index in [1, 2]:
                    required: int = value
                    value, index = (None, index)
    """,
    "iteration_optional_nested_unpacking_backedge": """
        def show(value: int | None) -> None:
            if value is not None:
                for index in [1, 2]:
                    required: int = value
                    index, (value, again) = (index, (None, True))
    """,
    "mixed_elements": 'values = ["text", 1]\n',
    "concat_mixed_lists": 'values = [1] + ["text"]\n',
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
                return value
            return 0
        use(None, True)
    """,
    "optional_and_false_has_no_fact": """
        def use(value: int | None, flag: bool) -> int:
            if value is None and flag:
                return 0
            return value
        use(None, False)
    """,
    "optional_boolean_fact_does_not_escape": """
        def use(value: int | None) -> int:
            positive = value is not None and value > 0
            return value
        use(None)
    """,
    "optional_for_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                for index in range(2):
                    required: int = value
                    value = None
        use(1)
    """,
    "optional_while_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                index = 0
                while index < 2:
                    required: int = value
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
                    required: int = value
                    if index == 0:
                        value = None
        use(1)
    """,
    "optional_nested_loop_backedge": """
        def use(value: int | None) -> None:
            if value is not None:
                for outer in range(2):
                    required: int = value
                    for inner in range(1):
                        value = None
        use(1)
    """,
    "optional_for_exit": """
        def use(value: int | None) -> int:
            if value is not None:
                for index in range(1):
                    value = None
                return value
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
                return values[0]
            return 0
        use([1])
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
    "set_iteration_insert_size_change": (
        "values = {1}\nfor value in values:\n    print(value)\n    values.add(2)\n",
        "RuntimeError",
    ),
    "set_iteration_discard_size_change": (
        'values = {"owned string"}\nalias = values\nfor value in values:\n    alias.discard(value)\n    print(value)\n',
        "RuntimeError",
    ),
    "set_enumerate_size_change": (
        "values = {3}\nfor index, value in enumerate(values, 10):\n    print(index)\n    print(value)\n    values.add(4)\n",
        "RuntimeError",
    ),
    "set_outer_iterator_size_change": (
        "values = {1}\nfor outer in values:\n    for inner in values:\n        values.add(2)\n        print(inner)\n        break\n",
        "RuntimeError",
    ),
    "set_membership_element_error_before_receiver": (
        'def element() -> int:\n    print("element")\n    return 1 // 0\ndef receiver() -> set[int]:\n    print("receiver must not run")\n    return {1}\nprint(element() in receiver())\n',
        "ZeroDivisionError",
    ),
    "set_method_receiver_error_before_argument": (
        'def receiver() -> set[int]:\n    print("receiver")\n    return {1 // 0}\ndef element() -> int:\n    print("argument must not run")\n    return 1\nreceiver().add(element())\n',
        "ZeroDivisionError",
    ),
    "set_literal_element_order": (
        "def element(value: int) -> int:\n    print(value)\n    if value == 1:\n        return 1 // 0\n    return value\nvalues = {element(1), element(2)}\n",
        "ZeroDivisionError",
    ),
    "dict_missing_index": (
        'values: dict[str, int] = {}\nprint(values["missing"])\n',
        "KeyError",
    ),
    "dict_missing_delete": ("values: dict[int, int] = {}\ndel values[1]\n", "KeyError"),
    "dict_missing_augmented_before_rhs": (
        'def rhs() -> int:\n    print("rhs must not run")\n    return 1\nvalues: dict[str, int] = {}\nvalues["missing"] += rhs()\n',
        "KeyError",
    ),
    "dict_get_eager_default_failure": (
        'def fallback() -> int:\n    print("default")\n    return 1 // 0\nvalues = {"found": 1}\nprint(values.get("found", fallback()))\n',
        "ZeroDivisionError",
    ),
    "dict_get_key_failure_before_default": (
        'def key() -> int:\n    print("key")\n    return 1 // 0\ndef fallback() -> int:\n    print("default must not run")\n    return 2\nvalues = {1: 1}\nprint(values.get(key(), fallback()))\n',
        "ZeroDivisionError",
    ),
    "dict_index_receiver_failure_before_key": (
        'def receiver() -> dict[int, int]:\n    print("receiver")\n    return {1: 1 // 0}\ndef key() -> int:\n    print("key must not run")\n    return 1\nprint(receiver()[key()])\n',
        "ZeroDivisionError",
    ),
    "dict_delete_receiver_then_key": (
        'def receiver() -> dict[int, int]:\n    print("receiver")\n    return {}\ndef key() -> int:\n    print("key")\n    return 1\ndel receiver()[key()]\n',
        "KeyError",
    ),
    "dict_membership_key_before_receiver_failure": (
        'def key() -> str:\n    print("key")\n    return "missing"\ndef receiver() -> dict[str, int]:\n    print("receiver")\n    return {"bad": 1 // 0}\nprint(key() in receiver())\n',
        "ZeroDivisionError",
    ),
    "dict_assignment_rhs_before_receiver_failure": (
        'def receiver() -> dict[str, int]:\n    print("receiver must not run")\n    return {}\ndef rhs() -> int:\n    print("rhs")\n    return 1 // 0\nreceiver()["key"] = rhs()\n',
        "ZeroDivisionError",
    ),
    "dict_iteration_insert_size_change": (
        'values = {"first": 1, "second": 2}\nfor key in values:\n    print(key)\n    values["new"] = 3\n',
        "RuntimeError",
    ),
    "dict_values_insert_size_change": (
        'values = {"first": 1, "second": 2}\nfor value in values.values():\n    print(value)\n    values["new"] = 3\n',
        "RuntimeError",
    ),
    "dict_items_delete_size_change": (
        'values = {"first": [1], "second": [2]}\nfor key, row in values.items():\n    del values[key]\n    print(key)\n    print(row[0])\n',
        "RuntimeError",
    ),
    "dict_enumerate_items_size_change": (
        "values = {1: True}\nfor index, (key, value) in enumerate(values.items(), 10):\n    print(index)\n    print(key)\n    print(value)\n    values[2] = False\n",
        "RuntimeError",
    ),
    "iteration_iterable_error_precedes_start": (
        (
            "def values() -> list[int]:\n"
            '    print("iterable")\n    return [1 // 0]\n'
            "def start() -> int:\n"
            '    print("start")\n    return 1 // 0\n'
            "for index, value in enumerate(values(), start()):\n"
            '    print("body")\n'
        ),
        "ZeroDivisionError",
    ),
    "iteration_start_error_precedes_body": (
        (
            "def values() -> list[int]:\n"
            '    print("iterable")\n    return [1]\n'
            "def start() -> int:\n"
            '    print("start")\n    return 1 // 0\n'
            "for index, value in enumerate(values(), start()):\n"
            '    print("body")\n'
        ),
        "ZeroDivisionError",
    ),
    "iteration_snapshot_before_body_failure": (
        (
            "values = [1000000000000000000000000000000]\n"
            "for value in values:\n"
            "    values.pop()\n    print(value)\n    print(values[0])\n"
        ),
        "IndexError",
    ),
    "iteration_unpacking_rhs_effects_before_failure": (
        (
            "def first() -> int:\n"
            '    print("first")\n    return 1\n'
            "def second() -> int:\n"
            '    print("second")\n    return 1 // 0\n'
            "left, right = (first(), second())\n"
        ),
        "ZeroDivisionError",
    ),
    "index_copy_both_bounds": (
        'values: list[int] = []\nprint("before")\nvalues[1] = values[0]\n',
        "IndexError",
    ),
    "index_copy_destination_bounds": (
        "values = [1000000000000000000000000000000]\nvalues[1] = values[0]\n",
        "IndexError",
    ),
    "index_copy_source_before_target_arithmetic": (
        "values: list[int] = []\nvalues[1 // 0] = values[0]\n",
        "IndexError",
    ),
    "index_copy_source_before_target_call": (
        (
            "def target(values: list[int]) -> list[int]:\n"
            '    print("target")\n    return values\n'
            "values: list[int] = []\ntarget(values)[0] = values[0]\n"
        ),
        "IndexError",
    ),
    "index_compare_read_before_division": (
        "values: list[int] = []\nprint(values[0] < 1 // 0)\n",
        "IndexError",
    ),
    "index_compare_division_before_read": (
        "values: list[int] = []\nprint(1 // 0 < values[0])\n",
        "ZeroDivisionError",
    ),
    "index_proof_alias_shrink": (
        (
            "values = [10, 20]\nalias = values\nindex = 1\n"
            "if 0 <= index and index < len(values):\n"
            "    print(alias.pop())\n    print(values[index])\n"
        ),
        "IndexError",
    ),
    "index_proof_range_shrink": (
        (
            "values = [10, 20]\nfor index in range(len(values)):\n"
            "    if index == 0:\n        print(values.pop())\n        continue\n"
            "    print(values[index])\n"
        ),
        "IndexError",
    ),
    "index_proof_nested_alias_shrink": (
        (
            "row = [10, 20]\nrows = [row, row]\nindex = 1\n"
            "if 0 <= index and index < len(rows[0]):\n"
            "    print(rows[1].pop())\n    print(rows[0][index])\n"
        ),
        "IndexError",
    ),
    "index_proof_rebound_list": (
        (
            "values = [10]\nindex = 0\n"
            "if 0 <= index and index < len(values):\n"
            '    values = []\n    print("rebound")\n    print(values[index])\n'
        ),
        "IndexError",
    ),
    "index_proof_call_in_guard": (
        (
            "def shrink(values: list[int]) -> bool:\n"
            '    print("shrink")\n    values.pop()\n    return True\n'
            "values = [10]\nindex = 0\n"
            "if 0 <= index and index < len(values) and shrink(values):\n"
            "    print(values[index])\n"
        ),
        "IndexError",
    ),
    "index_proof_set_rhs_shrink": (
        (
            "def right(values: list[int]) -> int:\n"
            '    print("right")\n    values.pop()\n    return 7\n'
            "values = [1000000000000000000000000000000]\nindex = 0\n"
            "if 0 <= index and index < len(values):\n"
            "    values[index] = right(values)\n"
        ),
        "IndexError",
    ),
    "index_proof_aug_rhs_shrink": (
        (
            "def right(values: list[int]) -> int:\n"
            '    print("right")\n    values.pop()\n    return 7\n'
            "values = [1000000000000000000000000000000]\nindex = 0\n"
            "if 0 <= index and index < len(values):\n"
            "    values[index] += right(values)\n"
        ),
        "IndexError",
    ),
    "index_proof_aug_get_before_rhs": (
        (
            'def right() -> int:\n    print("right")\n    return 1 // 0\n'
            "values = [10]\nindex = 1\nif index >= 0:\n"
            "    values[index] += right()\n"
        ),
        "IndexError",
    ),
    "index_proof_set_rhs_before_bounds": (
        (
            'def right() -> int:\n    print("right")\n    return 1 // 0\n'
            "values = [10]\nindex = 1\nif index >= 0:\n"
            "    values[index] = right()\n"
        ),
        "ZeroDivisionError",
    ),
    "index_proof_huge_nonnegative": (
        (
            "def read(values: list[int], index: int) -> int:\n"
            "    if index >= 0:\n        return values[index]\n    return 0\n"
            "print(read([10], 18446744073709551616))\n"
        ),
        "IndexError",
    ),
    "index_proof_promoted_index": (
        (
            "values = [10]\nindex = 4611686018427387903\n"
            "index += 1\nindex *= 2\nprint(index)\nprint(values[index])\n"
        ),
        "IndexError",
    ),
    "index_proof_divisor_loop_zero": (
        (
            "divisor = 1\nfor turn in range(2):\n"
            "    if turn == 1:\n        divisor = 0\n"
            "    print(7 // divisor)\n"
        ),
        "ZeroDivisionError",
    ),
    "index_proof_modulo_branch_zero": (
        (
            "def remainder(flag: bool) -> None:\n    divisor = 2\n"
            "    if flag:\n        divisor = 0\n    print(7 % divisor)\n"
            "remainder(False)\nremainder(True)\n"
        ),
        "ZeroDivisionError",
    ),
    "index_proof_divisor_guard_reassigned": (
        (
            "divisor = 1\nif divisor > 0:\n    divisor -= 1\n"
            '    print("zero")\n    print(7 // divisor)\n'
        ),
        "ZeroDivisionError",
    ),
    "tuple_unused_element_error": (
        (
            "def broken() -> tuple[int, int]:\n    return (7, 1 // 0)\n"
            'print("before")\npair = broken()\nprint(pair[0])\n'
        ),
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


def check_join_under_memory_limit(command: list[str]) -> None:
    import resource

    def limit() -> None:
        resource.setrlimit(resource.RLIMIT_AS, (96 << 20, 96 << 20))

    # The private list holds repeated references to one string. Building the
    # 256 MiB result must fail at the join, after the user's print has run.
    result = subprocess.run(
        [*command, "bounded"], capture_output=True, check=True, timeout=30, preexec_fn=limit
    )
    assert result.stdout == b"finished collecting\njoin raised MemoryError\n", result


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
    with tempfile.TemporaryDirectory(prefix="imre-containers-") as directory:
        temporary = Path(directory)
        # GNU ld's allocator wrappers run on the native Linux host; ASan supplies its own allocator.
        if sys.platform.startswith("linux") and args.target is None and not args.sanitize:
            passed += comparison_allocation.check(args, temporary)
            passed += deferred_diff_allocation.check(args, temporary)
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(dedent(source), encoding="utf-8")
            checked = run([compiler, "check", str(path)])
            assert checked.returncode > 0 and checked.stderr, (name, checked)
            if name.endswith("class_cycle"):
                assert "cyclic heap types" in checked.stderr, (name, checked.stderr)
            if name == "counter_schema":
                assert "JSON schemas require ordinary dictionaries" in checked.stderr, checked.stderr
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
                if name == "private_join_memory" and sys.platform == "linux" and not args.sanitize:
                    check_join_under_memory_limit([sys.executable, str(path)])
                    check_join_under_memory_limit([str(binary)])
                passed += 1
    print(
        f"Passed {passed} container checks ({len(REJECTED)} rejected programs; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
