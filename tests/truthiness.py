"""Compare concrete truth tests and operand-returning Boolean expressions with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "truthiness"

REJECTED = {
    "unrelated_and_result": 'value = 1 and "text"\n',
    "unrelated_or_result": 'value = 1 or "text"\n',
    "different_container_results": 'value = [1] or ["text"]\n',
    "different_optional_results": "def bad(left: int | None, right: str | None) -> int | None:\n    return left or right\n",
    "wrong_contextual_empty": "values: list[str] = [] or [1]\n",
    "existing_walrus_slot_stays_exact": 'existing: list[int] = []\nvalues: list[str] = (existing := []) or ["wrong"]\n',
    "bool_annotation_is_exact": "value: bool = 1\n",
    "bool_parameter_is_exact": "def accept(value: bool) -> bool:\n    return value\nprint(accept(1))\n",
    "bool_result_is_exact": "value: bool = 0 or 1\n",
    "bool_two_arguments": "print(bool(1, 2))\n",
    "bool_keyword": "print(bool(value=1))\n",
    "false_integer_may_be_none": "def bad(value: int | None) -> int:\n    if not value:\n        return value * 2\n    return 0\n",
    "false_string_may_be_none": 'def bad(value: str | None) -> str:\n    if value:\n        return value\n    return value.upper()\n',
    "false_list_may_be_none": "def bad(value: list[int] | None) -> int:\n    if not value:\n        return len(value)\n    return 0\n",
    "truth_fact_does_not_escape": "def bad(value: int | None) -> int:\n    if value:\n        print(value + 1)\n    return value * 2\n",
    "and_rebinding_invalidates_fact": "def bad(value: int | None) -> int:\n    if value and (value := None) is None:\n        return value + 1\n    return 0\n",
    "or_rebinding_invalidates_fact": "def bad(value: int | None) -> int:\n    if value or (value := None) is None:\n        return value * 2\n    return 0\n",
    "while_false_is_not_none_proof": "def bad(value: int | None) -> int:\n    while value:\n        value = None\n    return value * 2\n",
    "skipped_and_binding": "def bad(value: int) -> int:\n    result = value and (saved := 1)\n    return saved\n",
    "skipped_or_binding": "def bad(value: int) -> int:\n    result = value or (saved := 1)\n    return saved\n",
    "dead_dynamic_operand": 'if False and eval("1"):\n    print(1)\n',
    "dead_dynamic_value": 'value = 1 or eval("2")\n',
}

ERRORS = {
    "selected_and_failure": (
        'def fail() -> str:\n    print("selected")\n    raise ValueError("and failure")\nprint("present" and fail())\n',
        "ValueError",
    ),
    "selected_or_failure": (
        'def fail() -> list[int]:\n    print("selected")\n    raise ValueError("or failure")\nempty: list[int] = []\nvalue = empty or fail()\n',
        "ValueError",
    ),
    "truth_operand_failure": (
        'def fail() -> str:\n    print("operand")\n    raise ValueError("truth failure")\nif fail():\n    print("unreachable")\n',
        "ValueError",
    ),
    "false_assertion": (
        'value: list[int] = []\nassert value, "empty payload"\n',
        "AssertionError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors=ERRORS,
        label="truthiness",
        description=__doc__,
    )
