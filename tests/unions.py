"""Compare closed unions, exact isinstance tests, narrowing, and owned storage with CPython."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "unions"

REJECTED = {
    "unsupported_union": "def bad(value: int | str) -> None:\n    pass\n",
    "unsupported_typing_union": "from typing import Union\ndef bad(value: Union[int, str]) -> None:\n    pass\n",
    "mutable_union": "def bad(value: list[str] | list[bytes]) -> None:\n    pass\n",
    "wrong_argument": "def accept(value: str | bytes) -> None:\n    pass\naccept(1)\n",
    "wrong_return": "def bad() -> str | bytes:\n    return 1\n",
    "numeric_import_wrong_return": "from numbers import Number\ndef bad() -> int:\n    return 'wrong'\nprint(bad())\n",
    "numeric_registration": "from numbers import Number\nNumber.register(str)\n",
    "any_parameter_wrong_return": "from typing import Any\ndef bad(value: Any) -> int:\n    return 'wrong'\nprint(bad(3))\n",
    "wrong_store": "value: str | bytes = 'ok'\nvalue = 1\n",
    "none_needs_optional": "value: str | bytes = None\n",
    "optional_argument": "def accept(value: str | bytes) -> None:\n    pass\ndef bad(value: str | None) -> None:\n    accept(value)\n",
    "member_needs_guard": "def bad(value: str | bytes) -> str:\n    return value.decode()\n",
    "invariant_lists": "texts: list[str] = []\nvalues: list[str | bytes] = texts\n",
    "invariant_dicts": "texts: dict[str, str] = {}\nvalues: dict[str, str | bytes] = texts\n",
    "wrong_list_element": "values: list[str | bytes] = [1]\n",
    "wrong_union_list_element": "value: str | list[str] | None = [1]\n",
    "invariant_union_list_member": "items: list[int] = []\nvalue: str | list[str] = items\n",
    "wrong_dictionary_value": "values: dict[str, str | bytes] = {'bad': 1}\n",
    "union_key": "values: dict[str | bytes, int] = {}\n",
    "union_set_element": "values: set[str | bytes] = set()\n",
    "unnarrowed_comparison": "def bad(left: str | bytes, right: str | bytes) -> bool:\n    return left == right\n",
    "alias_field_fact": "class Box:\n    value: str | bytes\n    def __init__(self, value: str | bytes) -> None:\n        self.value = value\ndef bad(box: Box) -> str:\n    if isinstance(box.value, bytes):\n        return box.value.decode()\n    return ''\n",
    "alias_item_fact": "def bad(values: list[str | bytes]) -> str:\n    if isinstance(values[0], bytes):\n        return values[0].decode()\n    return ''\n",
    "walrus_invalidates_fact": "def bad(value: str | bytes) -> str:\n    if isinstance(value, str) and not isinstance((value := b'new'), str):\n        return value + 'bad'\n    return ''\n",
    "loop_invalidates_fact": "def bad(value: str | bytes) -> None:\n    if isinstance(value, str):\n        while len(value) > 0:\n            value = b'new'\n",
    "branch_may_not_assign": "def bad(value: str | bytes | None) -> str:\n    if isinstance(value, str):\n        result = value\n    elif isinstance(value, bytes):\n        result = value.decode()\n    return result\n",
    "dead_branch_still_checked": "def bad(value: bytes) -> None:\n    if isinstance(value, str):\n        missing()\n",
    "parameterized_isinstance": "print(isinstance([], list[int]))\n",
    "dynamic_isinstance": "name = 'str'\nprint(isinstance('ok', name))\n",
    "keyword_isinstance": "print(isinstance('ok', classinfo=str))\n",
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors={},
        label="finite union",
        description=__doc__,
    )
