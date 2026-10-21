"""Compare closed-program signature inference and its exact-type boundaries."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "inference"

REJECTED = {
    "unused_parameter": "def unused(value) -> None:\n    pass\n",
    "unused_parameter_body_constraint": "def unused(value):\n    return value + 1\n",
    "unused_keyword_parameter": "def unused(*, value):\n    return 1\n",
    "conflicting_scalar_calls": 'def identity(value):\n    return value\nidentity(1)\nidentity("one")\n',
    "conflicting_unused_parameter": 'def ignore(value):\n    pass\nignore(1)\nignore("one")\n',
    "bool_is_not_int": "def identity(value):\n    return value\nidentity(True)\nidentity(1)\n",
    "conflicting_container_calls": 'def first(values):\n    return values[0]\nfirst([1])\nfirst(["one"])\n',
    "conflicting_default": 'def identity(value=1):\n    return value\nidentity("one")\n',
    "conflicting_bool_default": "def identity(value=False):\n    return value\nidentity(1)\n",
    "explicit_parameter_contract": 'def identity(value: int):\n    return value\nidentity("one")\n',
    "explicit_return_contract": 'def identity(value) -> int:\n    return value\nidentity("one")\n',
    "unused_explicit_return_contract": 'def unused() -> int:\n    return "one"\n',
    "conflicting_returns": 'def choose(flag: bool):\n    if flag:\n        return 1\n    return "one"\nchoose(True)\n',
    "dead_conflicting_call": 'def identity(value):\n    return value\nidentity(1)\nif False:\n    identity("one")\n',
    "unused_dynamic_body": 'def unused():\n    if False:\n        eval("1")\n    return 1\n',
    "mutable_default": "def take(values=[]):\n    return len(values)\ntake()\n",
    "unknown_value": "def identity(value):\n    return value\nidentity(missing)\n",
    "missing_argument": "def identity(value):\n    return value\nidentity()\n",
    "unknown_keyword": "def identity(value):\n    return value\nidentity(other=1)\n",
    "direct_cycle": "def recurse(value):\n    return recurse(value)\nrecurse(1)\n",
    "mutual_cycle": "def first(value):\n    return second(value)\ndef second(value):\n    return first(value)\nfirst(1)\n",
    "conflicting_method_calls": "class Cell:\n    value: int\n    def __init__(self, value):\n        self.value = value\n    def take(self, value):\n        pass\ncell = Cell(1)\ncell.take(1)\ncell.take(False)\n",
    "inferred_constructor_field_contract": 'class Cell:\n    value: int\n    def __init__(self, value):\n        self.value = value\nCell("one")\n',
    "inferred_constructor_returns_value": "class Cell:\n    value: int\n    def __init__(self):\n        self.value = 1\n        return 1\nCell()\n",
    "conflicting_items_consumer_calls": 'class Box:\n    marker: int\n    def __init__(self, marker: int) -> None:\n        self.marker = marker\n    def items(self, value):\n        return [value]\nbox = Box(1)\nfirst = [*box.items(2)]\nsecond = [*box.items("wrong")]\n',
}

IMPORTED_REJECTIONS = {
    "conflicting_reexports": {
        "main.py": 'from helper import identity\nfrom facade import identity as alias\nidentity(1)\nalias("one")\n',
        "helper.py": "def identity(value):\n    return value\n",
        "facade.py": "from helper import identity\n",
    },
    "unused_imported_ambiguity": {
        "main.py": "from helper import known\nprint(known())\n",
        "helper.py": "def known():\n    return 1\ndef unused(value):\n    return 1\n",
    },
    "unused_imported_invalid_body": {
        "main.py": "from helper import known\nprint(known())\n",
        "helper.py": 'def known():\n    return 1\ndef unused():\n    return eval("1")\n',
    },
}

ERRORS = {
    "inferred_failure": (
        'def parse(value):\n    print("before conversion")\n    return int(value)\nprint(parse("bad"))\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors=ERRORS,
        label="signature inference",
        description=__doc__,
    )
