"""Compare conditional expressions and concrete sequence access with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "expressions"

REJECTED = {
    "conditional_int_string": 'print(1 if True else "wrong")\n',
    "conditional_container_types": 'values = [1] if True else ["wrong"]\n',
    "conditional_wrong_context": "values: list[str] = [] if True else [1]\n",
    "conditional_wrong_optional": "value: int | None = None if True else False\n",
    "conditional_distinct_optionals": "def bad(number: int | None, flag: bool | None, choose: bool) -> int | None:\n    return number if choose else flag\n",
    "conditional_narrowing_does_not_escape": "def bad(value: int | None) -> int:\n    result = value + 1 if value is not None else 0\n    return value\n",
    "conditional_dead_dynamic": 'print(1 if True else eval("2"))\n',
    "conditional_unused_dynamic": 'def unused() -> int:\n    return eval("1") if False else 2\nprint(1)\n',
    "conditional_recursion": "def recursive(value: int) -> int:\n    return recursive(value) if False else value\nrecursive(1)\n",
    "conditional_indirect_recursion": "def first(value: int) -> int:\n    return second(value) if False else value\ndef second(value: int) -> int:\n    return first(value) if True else 0\nfirst(1)\n",
    "string_index_bool": 'print("abc"[True])\n',
    "string_index_string": 'print("abc"["0"])\n',
    "string_index_optional": 'def bad(index: int | None) -> str:\n    return "abc"[index]\n',
    "string_assignment": 'value = "abc"\nvalue[0] = "x"\n',
    "string_deletion": 'value = "abc"\ndel value[0]\n',
    "slice_bool_start": "values = [1, 2][True:]\n",
    "slice_bool_stop": 'value = "abc"[:False]\n',
    "slice_bool_step": "values = [1, 2][::True]\n",
    "slice_string_bound": 'value = "abc"["1":]\n',
    "slice_optional_bool": 'def bad(step: bool | None) -> str:\n    return "abc"[::step]\n',
    "tuple_slice_dynamic_bound": 'def cut(stop: int) -> None:\n    print((1, "two")[:stop])\n',
    "heterogeneous_tuple_index_out_of_bounds": 'values = (1, "two")\nprint(values[2])\n',
    "slice_existing_type_change": "original: list[int] = [1]\nvalues: list[int | None] = original[:]\n",
    "slice_dictionary": "values = {0: 1}[:]\n",
    "slice_assignment": "values = [1, 2]\nvalues[:] = [3]\n",
    "slice_deletion": "values = [1, 2]\ndel values[:]\n",
    "slice_augmented_assignment": "values = [1, 2]\nvalues[:] += [3]\n",
    "string_iterator_escape": 'iterator = iter("abc")\n',
    "string_loop_uninitialized_target": 'for character in "":\n    pass\nprint(character)\n',
}

IMPORTED_REJECTIONS = {
    "inactive_conditional_dynamic": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": 'if __name__ == "__main__":\n    print(1 if True else eval("2"))\n',
    },
    "unused_conditional_recursion": {
        "main.py": "import helper\nprint(1)\n",
        "helper.py": "def recursive(value: int) -> int:\n    return value if True else recursive(value)\n",
    },
}

ERRORS = {
    "condition_failure": (
        'def condition() -> bool:\n    print("condition")\n    raise ValueError("condition failure")\ndef arm() -> str:\n    print("must not evaluate")\n    return "unused"\nprint(arm() if condition() else arm())\n',
        "ValueError",
    ),
    "selected_arm_failure": (
        'def selected() -> str:\n    print("selected")\n    raise ValueError("arm failure")\ndef unused() -> str:\n    print("must not evaluate")\n    return "unused"\nprint(selected() if True else unused())\n',
        "ValueError",
    ),
    "empty_string_index": ('print("before")\nprint(""[0])\n', "IndexError"),
    "large_string_index": (
        'print("before")\nprint(("owned " + str(42))[1000000000000000000000000000000])\n',
        "IndexError",
    ),
    "negative_string_index": ('print("é🌍"[-3])\n', "IndexError"),
    "zero_string_step": (
        'print("before")\nprint(("owned " + str(42))[None:None:0])\n',
        "ValueError",
    ),
    "zero_list_step": (
        'print("before")\nvalues = [[1000000000000000000000000000000], [2]][::0]\n',
        "ValueError",
    ),
    "slice_receiver_failure": (
        'def receiver() -> str:\n    print("receiver")\n    raise ValueError("receiver failure")\ndef bound() -> int:\n    print("must not evaluate")\n    return 1\nprint(receiver()[bound():bound():bound()])\n',
        "ValueError",
    ),
    "enumerate_start_failure": (
        'def start() -> int:\n    print("start")\n    raise ValueError("start failure")\nfor index, character in enumerate("owned " + str(42), start()):\n    print("must not enter")\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors=ERRORS,
        label="expression and sequence",
        description=__doc__,
        require_imported=True,
    )
