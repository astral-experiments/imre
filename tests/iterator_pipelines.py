"""Compare immediate lazy iterator pipelines and their observable effects."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "iterator_pipelines"

REQUIRE_INT = "def require_int(value: int) -> int:\n    return value\n"

REJECTED = {
    "producer_escape": "from collections.abc import Iterator\ndef values() -> Iterator[int]:\n    for i in range(2):\n        yield i\nsaved = values()\n",
    "producer_handler": "from collections.abc import Iterator\ndef values() -> Iterator[int]:\n    for i in range(2):\n        try:\n            yield i\n        finally:\n            print(i)\nfor x in values():\n    print(x)\n",
    "sentinel_escaped": "saved = iter(lambda: 'x', 'x')\n",
    "sentinel_types": "for x in iter(lambda: b'x', 'x'):\n    print(x)\n",
    "sentinel_defaults": "for x in iter(lambda v=b'': v, b''):\n    print(x)\n",
    "sentinel_optional_capture": "def f(value: str | None) -> None:\n    if value is not None:\n        for x in iter(lambda: value, ''):\n            value = None\n",
    "sentinel_unbound_iter": "def f() -> None:\n    for x in iter(lambda: '', ''):\n        print(x)\n    iter = 3\nf()\n",
    "sentinel_deleted_capture": "def f() -> None:\n    error = ValueError('first')\n    for i, word in enumerate(iter(lambda: str(error), 'stop')):\n        print(i, word)\n        try:\n            raise ValueError('second')\n        except ValueError as error:\n            pass\nf()\n",
    "chain_heterogeneous": "from itertools import chain\nitems = list(chain([1], ['s']))\n",
    "chain_escaped": "from itertools import chain\nitems = chain([1], [2])\n",
    "chain_keyword": "from itertools import chain\nitems = list(chain(source=[1]))\n",
    "min_float_generator": "print(min(float(i) for i in range(2)))\n",
    "max_generator_default": "print(max((i for i in range(2)), default=0))\n",
    "min_mixed_values": "print(min(1, 2.5))\n",
    "groupby_stored": "from itertools import groupby\nruns = groupby([1])\n",
    "groupby_group_escapes": "from itertools import groupby\nfor key, group in groupby([1]):\n    members = list(group)\n    saved = group\n",
    "groupby_group_delayed": "from itertools import groupby\nfor key, group in groupby([1]):\n    print(key)\n    members = list(group)\n",
    "groupby_group_reused": "from itertools import groupby\nfor key, group in groupby([1]):\n    members = list(group)\n    remainder = list(group)\n",
    "groupby_ambiguous_target": "from itertools import groupby\nfor group, group in groupby([1]):\n    members = list(group)\n",
    "groupby_float_key": "from itertools import groupby\nfor key, group in groupby([0.0]):\n    members = list(group)\n",
    "groupby_file_source": 'from itertools import groupby\nwith open("source", "r", encoding="utf-8") as stream:\n    for key, group in groupby(stream):\n        members = list(group)\n',
    "generator_escaped": "values = (value for value in range(3))\n",
    "shadowed_reducer_call": "def f(min: int) -> int:\n    return min(x for x in range(2))\nprint(f(2))\n",
    "generator_returned": "def values():\n    return (value for value in range(3))\nprint(1)\n",
    "generator_stored": "values = [(value for value in range(3))]\n",
    "generator_argument": "def take(value: int) -> int:\n    return value\nprint(take(value for value in range(3)))\n",
    "generator_target_leaks": "values = list(target for target in range(3))\nprint(target)\n",
    "generator_later_target_unbound": "def bad() -> list[int]:\n    later = 9\n    return list(later for first in range(1) if later > 0 for later in range(2))\nbad()\n",
    "generator_optional_unchecked": REQUIRE_INT + "def bad(values: list[int | None]) -> int:\n    return sum(require_int(value) for value in values)\nbad([None])\n",
    "generator_wrong_context": 'values: list[int] = list(value for value in ["wrong"])\n',
    "generator_dynamic_dead_element": 'print(any(eval("1") for value in range(0)))\n',
    "generator_target_rebound": "print(any((value := 1) for value in range(3)))\n",
    "generator_target_index_walrus": "values = [0, 0]\nsaved = 0\nprint(any(0 for values[(saved := 1)] in [4, 5]))\n",
    "generator_uninitialized_export": "print(any((saved := value) for value in range(0)))\nprint(saved)\n",
    "generator_short_circuit_export": "print(any(value > 0 or (saved := value) > 0 for value in [1]))\nprint(saved)\n",
    "generator_constructor_partial_self": "class Item:\n    value: int\n    def __init__(self) -> None:\n        self.value = sum(self.value + value for value in range(0))\nItem()\n",
    "generator_constructor_partial_source": "class Item:\n    values: list[int]\n    def __init__(self) -> None:\n        self.values = list(self for self in self.values)\nItem()\n",
    "generator_async": "async def bad(values: list[int]):\n    return any(value async for value in values)\n",
    "generator_yield_function": "def values():\n    yield 1\nprint(any(values()))\n",
    "zip_escaped": "values = zip([1], [2])\n",
    "zip_scalar": "values = list(zip([1], 2))\n",
    "zip_unknown_keyword": "values = list(zip([1], exact=True))\n",
    "zip_iterable_keyword": "values = list(zip(iterable=[1]))\n",
    "zip_duplicate_strict": "values = list(zip([1], strict=True, strict=False))\n",
    "reversed_escaped": "values = reversed([1])\n",
    "reversed_missing": "values = list(reversed())\n",
    "reversed_extra": "values = list(reversed([1], [2]))\n",
    "reversed_keyword": "values = list(reversed(sequence=[1]))\n",
    "reversed_scalar": "values = list(reversed(1))\n",
    "reversed_generator": "values = list(reversed(value for value in range(3)))\n",
    "reversed_zip": "values = list(reversed(zip([1], [2])))\n",
    "reversed_set": "values = list(reversed({1, 2}))\n",
    "reversed_heterogeneous_tuple": 'values = list(reversed((1, "two")))\n',
    "any_missing": "print(any())\n",
    "any_extra": "print(any([1], [2]))\n",
    "any_keyword": "print(any(iterable=[1]))\n",
    "any_scalar": "print(any(1))\n",
    "all_missing": "print(all())\n",
    "all_extra": "print(all([1], [2]))\n",
    "all_keyword": "print(all(iterable=[1]))\n",
    "all_scalar": "print(all(None))\n",
    "any_not_optional_guard": "def bad(value: int | None) -> None:\n    if any(value is not None for index in range(1)):\n        required: int = value\nbad(None)\n",
    "generator_backedge_optional": 'value: str | None = "ok"\nprint(all((value := None) for index in range(2) if len(value) > 0))\n',
    "generator_inner_backedge_optional": 'value: str | None = "ok"\nprint(any((value := None) for outer in range(1) if value is not None for inner in range(2) if len(value) > 0))\n',
    "generator_export_invalidates_optional": "value: int | None = 1\nprint(any((value := None) for index in range(1)))\nrequired: int = value\n",
    "generator_after_start_optional": "value: int | None = None\nprint(sum(((value := None) or 0 for index in [1]), (value := 1)))\nrequired: int = value\n",
    "generator_after_strict_optional": "value: int | None = None\nfor left, right in zip(((value := None) or 0 for index in [1]), [2], strict=(value := 1)):\n    required: int = value\n",
    "generator_suspended_optional": REQUIRE_INT + "value: int | None = 1\nfor result in (require_int(value) + item for outer in [0] if value is not None for item in range(2)):\n    value = None\n",
    "generator_sibling_suspended_optional": REQUIRE_INT + "value: int | None = 1\nfor result, ignored in zip((require_int(value) + item for outer in [0] if value is not None for item in range(2)), ((value := None) for index in range(2))):\n    print(result)\n",
}

OPTIONAL_ADD_ERROR = "TypeError: unsupported operand type(s) for +: 'NoneType' and 'int'"

ERRORS = {
    "generator_add_none": (
        "def bad(values: list[int | None]) -> int:\n    return sum(value + 1 for value in values)\nbad([None])\n",
        OPTIONAL_ADD_ERROR,
    ),
    "generator_export_add_none": (
        "value: int | None = 1\nprint(any((value := None) for index in range(1)))\nprint(value + 1)\n",
        OPTIONAL_ADD_ERROR,
    ),
    "generator_after_start_add_none": (
        "value: int | None = None\nprint(sum(((value := None) or 0 for index in [1]), (value := 1)))\nprint(value + 1)\n",
        OPTIONAL_ADD_ERROR,
    ),
    "generator_after_strict_add_none": (
        "value: int | None = None\nfor left, right in zip(((value := None) or 0 for index in [1]), [2], strict=(value := 1)):\n    print(value + 1)\n",
        OPTIONAL_ADD_ERROR,
    ),
    "generator_resume_add_none": (
        "value: int | None = 1\nfor result in (value + item for outer in [0] if value is not None for item in range(2)):\n    value = None\n",
        OPTIONAL_ADD_ERROR,
    ),
    "generator_sibling_resume_add_none": (
        "value: int | None = 1\nfor result, ignored in zip((value + item for outer in [0] if value is not None for item in range(2)), ((value := None) for index in range(2))):\n    print(result)\n",
        OPTIONAL_ADD_ERROR,
    ),
    "outer_range_before_start": (
        'def start() -> int:\n    print("unexpected start")\n    return 7\nprint(sum((value for value in range(0, 3, 0)), start()))\n',
        "ValueError",
    ),
    "inner_range_after_start": (
        'def start() -> int:\n    print("start")\n    return 7\nprint(sum((inner for outer in range(1) for inner in range(0, 3, 0)), start()))\n',
        "ValueError",
    ),
    "generator_element_failure": (
        "def item(index: int) -> int:\n    print(index)\n    return 10 // (2 - index)\nprint(sum(item(index) for index in range(4)))\n",
        "ZeroDivisionError",
    ),
    "zip_conversion_failure": (
        'def observe(value: int) -> int:\n    print(value)\n    return value\nvalues = list(zip((observe(value) for value in range(3)), map(int, "1x2"), strict=True))\n',
        "ValueError",
    ),
    "reversed_zero_step": (
        'print("before")\nvalues = list(reversed(range(0, 3, 0)))\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors=ERRORS,
        label="iterator pipeline",
        description=__doc__,
    )
