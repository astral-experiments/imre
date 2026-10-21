"""Compare integer bit operations, divmod, list.extend, and str.rstrip with CPython."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "primitives"

REJECTED = {
    "bitwise_bool": "print(True & False)\n",
    "bitwise_float": "print(1.0 | 2)\n",
    "bitwise_string": 'print("x" ^ 1)\n',
    "shift_float": "print(1 << 2.0)\n",
    "shift_bool": "print(1 >> True)\n",
    "invert_float": "print(~1.0)\n",
    "invert_bool": "print(~True)\n",
    "optional_integer": "def bad(value: int | None) -> int:\n    return value & 3\n",
    "divmod_float": "pair = divmod(1.0, 2)\n",
    "divmod_bool": "pair = divmod(True, 2)\n",
    "divmod_missing": "pair = divmod(1)\n",
    "divmod_extra": "pair = divmod(1, 2, 3)\n",
    "divmod_keyword": "pair = divmod(1, y=2)\n",
    "divmod_rebinding": "divmod = 1\n",
    "extend_integer": "values = [1]\nvalues.extend(2)\n",
    "extend_wrong_type": 'values = [1]\nvalues.extend(["wrong"])\n',
    "extend_heterogeneous_tuple": 'values = [1]\nvalues.extend((2, "wrong"))\n',
    "extend_bool": "values = [1]\nvalues.extend((True,))\n",
    "extend_missing": "values = [1]\nvalues.extend()\n",
    "extend_extra": "values = [1]\nvalues.extend([2], [3])\n",
    "extend_keyword": "values = [1]\nvalues.extend(iterable=[2])\n",
    "extend_optional": "def bad(values: list[int] | None) -> None:\n    values.extend([1])\n",
    "rstrip_integer": 'print("x".rstrip(1))\n',
    "rstrip_bytes": 'print("x".rstrip(b"x"))\n',
    "rstrip_extra": 'print("x".rstrip("x", "y"))\n',
    "rstrip_keyword": 'print("x".rstrip(chars="x"))\n',
}

ERRORS = {
    "negative_left_shift": ('print("before")\nprint(1 << -1)\n', "ValueError"),
    "negative_right_shift": ('print("before")\nprint(1 >> -1)\n', "ValueError"),
    "divmod_zero": ('print("before")\npair = divmod(3, 0)\n', "ZeroDivisionError"),
    "extend_zero_step": (
        'values = [1]\nprint("before")\nvalues.extend(range(0, 3, 0))\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors=ERRORS,
        label="integer and container primitives",
        description=__doc__,
    )
