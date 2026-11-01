"""Compare closed structural pattern matching with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "matching"

PAIR_MATCH = """\
from typing import assert_never
def classify(left: bool, right: bool) -> int:
    match left, right:
        case (True, True) | (True, False) | (False, True):
            return 1
"""
LAST_PAIR = "        case (False, False):\n            return 0\n"
PAIR_FALLBACK = "        case _:\n            assert_never((left, right))  # ty: ignore[type-assertion-failure]\n"

REJECTED = {
    "bool_pair_incomplete": PAIR_MATCH + PAIR_FALLBACK,
    "bool_pair_missing_return": PAIR_MATCH,
    "bool_pair_int_subject": PAIR_MATCH.replace("left: bool", "left: int")
    + LAST_PAIR
    + PAIR_FALLBACK,
    "bool_pair_guarded": PAIR_MATCH
    + LAST_PAIR.replace("):", ") if left:")
    + PAIR_FALLBACK,
    "bool_pair_reordered": PAIR_MATCH
    + LAST_PAIR
    + PAIR_FALLBACK.replace("(left, right)", "(right, left)"),
    "bool_pair_guard_rebind": PAIR_MATCH
    + "        case (False, False) if (left := right):\n            return 2\n"
    + LAST_PAIR
    + PAIR_FALLBACK,
    "bool_pair_capture_rebind": PAIR_MATCH
    + "        case (right, False) if right:\n            return 2\n"
    + LAST_PAIR
    + PAIR_FALLBACK,
    "bool_pair_body_rebind": PAIR_MATCH
    + LAST_PAIR
    + PAIR_FALLBACK.replace(
        "            assert_never",
        "            left = not left\n            assert_never",
    ),
    "duplicate_capture": "match [1, 2]:\n    case [value, value]:\n        pass\n",
    "duplicate_as_capture": "match 1:\n    case value as value:\n        pass\n",
    "duplicate_mapping_rest": 'match {"key": 1}:\n    case {"key": value, **value}:\n        pass\n',
    "or_binding_names": "match [1]:\n    case [left] | [right]:\n        pass\n",
    "or_binding_types": 'match (1, "text"):\n    case (value, _) | (_, value):\n        print(value)\n',
    "irrefutable_case": "match 1:\n    case value:\n        pass\n    case 1:\n        pass\n",
    "irrefutable_or": "match 1:\n    case (_ as value) | (1 as value):\n        pass\n",
    "multiple_stars": "match [1, 2]:\n    case [*first, *last]:\n        pass\n",
    "capture_not_definite": "def bad(values: list[int]) -> int:\n    match values:\n        case [captured]:\n            pass\n    return captured\n",
    "guard_capture_not_definite": "def bad(values: list[int]) -> int:\n    match values:\n        case [captured] if False:\n            pass\n    return captured\n",
    "capture_type_change": 'value = "text"\nmatch 1:\n    case value:\n        pass\n',
    "capture_builtin": "match 1:\n    case int:\n        pass\n",
    "capture_function": "def fixed() -> int:\n    return 1\nmatch 1:\n    case fixed:\n        pass\n",
    "heterogeneous_tuple_rest": 'match (1, "text"):\n    case [*rest]:\n        pass\n',
    "tuple_shape_overflow": "match (1,):\n    case [first, second]:\n        pass\n",
    "sequence_string": 'match "abc":\n    case [first, *rest]:\n        pass\n',
    "sequence_dictionary": 'match {"key": 1}:\n    case [first]:\n        pass\n',
    "mapping_list": 'match [1]:\n    case {"key": value}:\n        pass\n',
    "duplicate_mapping_int_keys": 'match {1: "x"}:\n    case {1: first, True: second}:\n        pass\n',
    "duplicate_mapping_string_keys": 'match {"x": 1}:\n    case {"x": first, "x": second}:\n        pass\n',
    "duplicate_mapping_zero_keys": 'match {0: "x"}:\n    case {0: first, -0: second}:\n        pass\n',
    "duplicate_mapping_int_float_keys": 'match {1: "x"}:\n    case {1: first, 1.0: second}:\n        pass\n',
    "duplicate_mapping_bool_float_keys": 'match {1: "x"}:\n    case {True: first, 1.0: second}:\n        pass\n',
    "duplicate_mapping_float_zero_keys": 'match {0: "x"}:\n    case {0.0: first, -0.0: second}:\n        pass\n',
    "duplicate_mapping_bytes_keys": 'match {b"x": 1}:\n    case {b"x": first, b"\\x78": second}:\n        pass\n',
    "duplicate_mapping_fractional_keys": 'match {"x": 1}:\n    case {0.5: first, 5e-1: second}:\n        pass\n',
    "duplicate_mapping_infinite_keys": "match {0: 1}:\n    case {1e400: first, 2e400: second}:\n        pass\n",
    "duplicate_mapping_negative_infinite_keys": "match {0: 1}:\n    case {-1e400: first, -2e400: second}:\n        pass\n",
    "duplicate_mapping_large_integral_keys": "match {0: 1}:\n    case {9007199254740992: first, 9007199254740993.0: second}:\n        pass\n",
    "builtin_class_pattern": "match 1:\n    case int():\n        pass\n",
    "dead_case_dynamic": 'match 1:\n    case 0:\n        eval("bad")\n    case _:\n        pass\n',
    "dead_guard_dynamic": 'match 1:\n    case 0 if eval("bad"):\n        pass\n    case _:\n        pass\n',
    "recursive_guard": "def recursive(value: int) -> bool:\n    match value:\n        case 0 if recursive(value):\n            return True\n        case _:\n            return False\n",
}

CLASS = "class Cell:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n"
REJECTED.update(
    {
        "positional_class": CLASS
        + "match Cell(1):\n    case Cell(value):\n        pass\n",
        "unknown_class_field": CLASS
        + "match Cell(1):\n    case Cell(missing=value):\n        pass\n",
        "duplicate_class_field": CLASS
        + "match Cell(1):\n    case Cell(value=first, value=second):\n        pass\n",
        "dynamic_value_pattern": CLASS
        + "cell = Cell(1)\nmatch 1:\n    case cell.value:\n        pass\n",
    }
)

COMPATIBILITY_REJECTIONS = {
    "operator_in_dead_code": (
        "def unused(value: float | None) -> float:\n    return 0.0\n    return value + 1.0\n",
        "Imre: arithmetic requires int or float operands",
    ),
    "assignment_in_unused_function": (
        "def unused() -> None:\n    value: float = None\n",
        "Imre: expected float, got None",
    ),
    "argument_in_dead_code": (
        "def identity(value: float) -> float:\n    return value\ndef unused() -> float:\n    return 0.0\n    return identity(None)\n",
        "Imre: expected float, got None",
    ),
    "return_in_unused_function": (
        "def unused(value: float | None) -> float:\n    return value\n",
        "Imre: expected float, got float | None",
    ),
}
REJECTED.update(
    {name: source for name, (source, _) in COMPATIBILITY_REJECTIONS.items()}
)

IMPORTED_REJECTIONS = {
    "unused_recursive_case": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": "def recursive(value: int) -> int:\n    match value:\n        case 0:\n            return recursive(value)\n        case _:\n            return value\n",
    },
    "inactive_main_match": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": 'if __name__ == "__main__":\n    match 1:\n        case _:\n            eval("bad")\n',
    },
}

ERRORS = {
    "subject_failure": (
        'def subject() -> list[int]:\n    print("subject")\n    raise ValueError("subject failure")\nmatch subject():\n    case [value]:\n        print("wrong case")\n    case _:\n        print("wrong fallback")\n',
        "ValueError",
    ),
    "guard_failure": (
        'def guard() -> bool:\n    print("guard")\n    raise ValueError("guard failure")\nmatch [1000000000000000000000000000000]:\n    case [value] if guard():\n        print("wrong case")\n    case _:\n        print("wrong fallback")\n',
        "ValueError",
    ),
    "selected_case_failure": (
        'match {"row": ["owned " + str(42)], "other": ["extra"]}:\n    case {"row": row, **rest}:\n        print(row[0])\n        raise RuntimeError("selected failure")\n    case _:\n        print("wrong fallback")\n',
        "RuntimeError",
    ),
}


def check_rejection(name: str, stderr: str) -> None:
    if "recursive" in name:
        assert "recursive calls are unsupported" in stderr, (name, stderr)
    if name in COMPATIBILITY_REJECTIONS:
        assert COMPATIBILITY_REJECTIONS[name][1] in stderr, (name, stderr)


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors=ERRORS,
        label="structural matching",
        description=__doc__,
        require_imported=True,
        check_rejection=check_rejection,
    )
