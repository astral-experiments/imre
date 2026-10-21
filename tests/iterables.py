"""Compare concrete iterable consumers, lazy conversion, and iterator ownership."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "iterables"

REJECTED = {
    "list_scalar": "print(len(list(1)))\n",
    "list_none": "print(len(list(None)))\n",
    "list_extra": "values = list([1], [2])\n",
    "list_keyword": "values = list(iterable=[1])\n",
    "list_heterogeneous_without_context": 'values = list((1, "two"))\n',
    "list_wrong_context": "values: list[str] = list([1])\n",
    "list_optional_source": "def take(values: list[int] | None) -> None:\n    result = list(values)\n",
    "sum_missing": "print(sum())\n",
    "sum_extra": "print(sum([1], 2, 3))\n",
    "sum_strings": 'print(sum("12"))\n',
    "sum_floats": "print(sum([1.0, 2.0]))\n",
    "sum_bools": "print(sum([True, False]))\n",
    "sum_tuple_mixed": "print(sum((1, True)))\n",
    "sum_pairs": "print(sum(enumerate([1])))\n",
    "sum_empty_strings": "values: list[str] = []\nprint(sum(values))\n",
    "sum_bool_start": "print(sum([1], True))\n",
    "sum_float_start": "print(sum([1], 1.0))\n",
    "sum_string_start": 'print(sum([1], "1"))\n',
    "sum_iterable_keyword": "print(sum(iterable=[1]))\n",
    "sum_unknown_keyword": "print(sum([1], initial=2))\n",
    "sum_duplicate_start": "print(sum([1], 2, start=3))\n",
    "map_escaped": 'values = map(int, "12")\n',
    "map_wrong_callback": "values = list(map(str, [1]))\n",
    "map_missing_source": "values = list(map(int))\n",
    "map_extra_source": 'values = list(map(int, "12", "34"))\n',
    "map_keyword": 'values = list(map(int, iterable="12"))\n',
    "map_bytes_elements": 'values = list(map(int, [b"1"]))\n',
    "map_none_elements": "values = list(map(int, [None]))\n",
    "map_tuple_elements": "values = list(map(int, [(1, 2)]))\n",
}

ERRORS = {
    "list_zero_step": (
        'print("before")\nvalues = list(range(0, 3, 0))\n',
        "ValueError",
    ),
    "sum_range_before_start": (
        'def start() -> int:\n    print("must not run")\n    return 1\nprint("before")\nprint(sum(range(0, 3, 0), start()))\n',
        "ValueError",
    ),
    "map_range_before_start": (
        'def start() -> int:\n    print("must not run")\n    return 1\nprint("before")\nprint(sum(map(int, range(0, 3, 0)), start()))\n',
        "ValueError",
    ),
    "map_conversion_after_start": (
        'def start() -> int:\n    print("start evaluated")\n    return 1\nprint("before")\nprint(sum(map(int, "1x"), start()))\n',
        "ValueError",
    ),
    "start_failure_before_conversion": (
        'def source() -> list[str]:\n    print("source evaluated")\n    return ["bad"]\ndef start() -> int:\n    print("start evaluated")\n    raise ValueError("start failure")\nprint(sum(map(int, source()), start()))\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors=ERRORS,
        label="iterable consumer",
        description=__doc__,
    )
