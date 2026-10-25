"""Compare loop exhaustion, break paths, and finally-aware else suites with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "loop_else"

REJECTED = {
    "for_break_leaves_unbound": "def bad(values: list[int]) -> int:\n    for value in values:\n        if value == 1:\n            break\n    else:\n        result = 2\n    return result\n",
    "while_break_leaves_unbound": "def bad(flag: bool) -> int:\n    while flag:\n        break\n    else:\n        result = 2\n    return result\n",
    "for_else_target_unbound": "for value in range(0):\n    pass\nelse:\n    print(value)\n",
    "while_else_body_binding_unbound": "def bad(flag: bool) -> int:\n    while flag:\n        result = 1\n        flag = False\n    else:\n        return result\n",
    "finally_break_leaves_unbound": "def bad(values: list[int]) -> int:\n    for value in values:\n        try:\n            continue\n        finally:\n            if value == 1:\n                break\n    else:\n        result = 2\n    return result\n",
    "except_target_deleted_on_break": 'def bad(values: list[int]) -> str:\n    for value in values:\n        try:\n            raise ValueError("caught")\n        except ValueError as error:\n            break\n    else:\n        error = ValueError("empty")\n    return str(error)\n',
    "for_else_break_without_outer": "for value in range(1):\n    pass\nelse:\n    break\n",
    "while_else_continue_without_outer": "while False:\n    pass\nelse:\n    continue\n",
    "dead_for_else_dynamic": 'for value in range(1):\n    break\nelse:\n    eval("1")\n',
    "dead_while_else_dynamic": 'while True:\n    break\nelse:\n    eval("1")\n',
    "wrong_else_assignment_type": 'def bad(values: list[int]) -> int:\n    result = 0\n    for value in values:\n        break\n    else:\n        result = "wrong"\n    return result\n',
}

ERRORS = {
    "range_zero_step_skips_else": (
        'for value in range(0, 0, 0):\n    print(value)\nelse:\n    print("must not execute")\n',
        "ValueError",
    ),
    "dictionary_changed_after_last_item": (
        'mapping = {"only": 1}\nfor key in mapping:\n    print(key)\n    mapping["added"] = 2\nelse:\n    print("must not execute")\n',
        "RuntimeError",
    ),
    "iterator_failure_skips_else": (
        'def source() -> list[int]:\n    print("source")\n    raise ValueError("iterator")\nfor value in source():\n    print(value)\nelse:\n    print("must not execute")\n',
        "ValueError",
    ),
    "condition_failure_skips_else": (
        'def condition() -> bool:\n    print("condition")\n    raise ValueError("condition")\nwhile condition():\n    print("body")\nelse:\n    print("must not execute")\n',
        "ValueError",
    ),
    "for_else_failure": (
        'for value in range(0):\n    print(value)\nelse:\n    print("else")\n    raise ValueError("for else")\n',
        "ValueError",
    ),
    "while_else_failure": (
        'while False:\n    print("body")\nelse:\n    print("else")\n    raise ValueError("while else")\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections={},
        errors=ERRORS,
        label="loop else",
        description=__doc__,
    )
