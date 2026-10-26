"""Compare chained assignment, sequence unpacking, and container expansion with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "unpacking"

REJECTED = {
    "tuple_after_resize": "def bad(xs: list[int]) -> tuple[int]:\n    if len(xs) != 1:\n        raise ValueError('length')\n    xs.append(2)\n    return tuple(xs)\n",
    "tuple_alias_resize": "def change(xs: list[int]) -> int:\n    xs.append(2)\n    return 7\ndef bad(xs: list[int]) -> tuple[int]:\n    if len(xs) != 1:\n        raise ValueError('length')\n    xs[0] = change(xs)\n    return tuple(xs)\n",
    "tuple_untrusted_cast": 'from typing import cast\ndef bad(xs: list[int]) -> tuple[int]:\n    return cast("tuple[int]", tuple(xs))\n',
    "chain_conflicting_slots": 'number = 0\ntext = ""\nnumber = text = 1\n',
    "chain_bool_is_not_int": "number: int = 0\nflag: bool = False\nnumber = flag = True\n",
    "chain_conflicting_empty_contexts": "numbers: list[int] = []\ntexts: list[str] = []\nnumbers = texts = []\n",
    "sequence_wrong_target_type": 'number = 0\nnumber, = ["wrong"]\n',
    "unpack_nonsequence": "first, second = 12\n",
    "unpack_dictionary": 'first, second = {"a": 1, "b": 2}\n',
    "unpack_set": "first, second = {1, 2}\n",
    "unpack_range": "first, second = range(2)\n",
    "fixed_tuple_wrong_arity": "first, second = (1, 2, 3)\n",
    "heterogeneous_tuple_rest": 'first, *rest = (1, 2, "wrong")\n',
    "wrong_starred_rest_type": "rest: list[str] = []\nfirst, *rest = [1, 2]\n",
    "multiple_starred_targets": "first, *middle, *last = [1, 2, 3]\n",
    "bare_starred_target": "*rest = [1, 2]\n",
    "optional_sequence_needs_guard": "def bad(values: list[int] | None) -> int:\n    first, = values\n    return first\n",
    "optional_element_needs_guard": "def bad(values: list[int | None]) -> int:\n    first, = values\n    return first\n",
    "chain_rhs_may_fail_before_binding": 'def fail() -> int:\n    raise ValueError("rhs")\ndef bad() -> None:\n    try:\n        first = second = fail()\n    except ValueError:\n        print(first)\n',
    "starred_loop_target_may_be_unbound": "rows: list[list[int]] = []\nfor first, *rest in rows:\n    pass\nprint(rest)\n",
    "starred_comprehension_target_is_private": "values = [first for first, *rest in [[1, 2]]]\nprint(rest)\n",
    "list_expansion_noniterable": "values = [*1]\n",
    "list_expansion_wrong_element": 'values: list[int] = [*["wrong"]]\n',
    "list_expansion_bool_is_not_int": "values: list[int] = [*[True]]\n",
    "list_expansion_heterogeneous_tuple": 'values = [*(1, "wrong")]\n',
    "list_expansion_optional_source": "def bad(values: list[int] | None) -> list[int]:\n    return [*values]\n",
    "set_expansion_unhashable_element": "values = {*[[1], [2]]}\n",
    "set_expansion_bool_element": "values = {*[True]}\n",
    "dictionary_expansion_nonmapping": 'values = {**[("a", 1)]}\n',
    "dictionary_expansion_wrong_key": "values: dict[str, int] = {**{1: 2}}\n",
    "dictionary_expansion_wrong_value": 'values: dict[str, int] = {**{"a": "wrong"}}\n',
    "call_argument_expansion": "def add(first: int, second: int) -> int:\n    return first + second\nprint(add(*[1, 2]))\n",
    "call_dynamic_keyword_expansion": 'def accept(value: int) -> int:\n    return value\ndef forward(key: str) -> int:\n    return accept(**{key: 1})\nprint(forward("value"))\n',
    "constructor_duplicate_chained_field": "class Item:\n    value: int\n    def __init__(self) -> None:\n        self.value = self.value = 1\n",
    "constructor_missing_chained_field": "class Item:\n    first: int\n    second: int\n    third: int\n    def __init__(self) -> None:\n        self.first = self.second = 1\n",
    "constructor_undeclared_starred_field": "class Item:\n    first: int\n    def __init__(self, values: list[int]) -> None:\n        self.first, *self.rest = values\n",
    "constructor_chained_rhs_reads_self": "class Item:\n    first: int\n    second: int\n    def __init__(self) -> None:\n        self.first = self.second = self.first\n",
    "constructor_nested_rhs_reads_self": "class Item:\n    first: int\n    second: int\n    def __init__(self) -> None:\n        self.first, (self.second,) = (1, (self.first,))\n",
}

ERRORS = {
    "too_few_list_values": ("first, second = [1]\n", "ValueError"),
    "too_many_list_values": ("first, second = [1, 2, 3]\n", "ValueError"),
    "too_few_starred_values": ("first, *middle, last = [1]\n", "ValueError"),
    "string_length_failure": ('first, second = "é"\n', "ValueError"),
    "bytes_length_failure": ('first, second = b"abc"\n', "ValueError"),
    "loop_unpacking_failure": (
        "for first, second in [[1, 2], [3]]:\n    print(first + second)\n",
        "ValueError",
    ),
    "comprehension_unpacking_failure": (
        "def show(value: int) -> int:\n    print(value)\n    return value\nvalues = [show(first) for first, second in [[1, 2], [3]]]\n",
        "ValueError",
    ),
    "expanded_range_zero_step": (
        'print("before")\nvalues = [1, *range(0, 2, 0), 3]\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors=ERRORS,
        label="unpacking and expansion",
        description=__doc__,
    )
