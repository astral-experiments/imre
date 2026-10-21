"""Compare exact callable values, callback pipelines, and ownership with CPython."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "callables"
IMPORT = "from typing import Callable\n"
FUNCTION = "def identity(value: int) -> int:\n    return value\n"

REJECTED = {
    "ellipsis_signature": IMPORT + "def use(fn: Callable[..., int]) -> int:\n    return fn(1)\n",
    "wrong_parameter": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[str], int] = identity\nrun()\n",
    "wrong_return": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], str] = identity\nrun()\n",
    "bool_is_not_int": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], int] = identity\n    print(fn(True))\nrun()\n",
    "indirect_keyword": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], int] = identity\n    print(fn(value=1))\nrun()\n",
    "indirect_missing": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], int] = identity\n    print(fn())\nrun()\n",
    "indirect_extra": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], int] = identity\n    print(fn(1, 2))\nrun()\n",
    "indirect_default": IMPORT + "def add(value: int = 1) -> int:\n    return value\ndef run() -> None:\n    fn: Callable[[int], int] = add\n    print(fn())\nrun()\n",
    "keyword_only_value": IMPORT + "def take(*, value: int) -> int:\n    return value\ndef run() -> None:\n    fn: Callable[[int], int] = take\nrun()\n",
    "capturing_parameter": IMPORT + "def make(offset: int) -> Callable[[int], int]:\n    return lambda value: value + offset\n",
    "capturing_local": IMPORT + "def run() -> None:\n    offset = 1\n    fn: Callable[[int], int] = lambda value: value + offset\nrun()\n",
    "capturing_map": "def run(offset: int) -> int:\n    return sum(map(lambda value: value + offset, [1]))\nprint(run(2))\n",
    "lambda_arity": IMPORT + "def run() -> None:\n    fn: Callable[[int], int] = lambda left, right: left + right\nrun()\n",
    "lambda_result": IMPORT + 'def run() -> None:\n    fn: Callable[[int], int] = lambda value: "wrong"\nrun()\n',
    "callable_field": IMPORT + FUNCTION + "class Holder:\n    fn: Callable[[int], int]\n    def __init__(self) -> None:\n        self.fn = identity\n",
    "incomplete_bound_receiver": IMPORT + "def apply(fn: Callable[[int], int], value: int) -> int:\n    return fn(value)\nclass Item:\n    value: int\n    def __init__(self) -> None:\n        self.value = apply(self.read, 1)\n    def read(self, value: int) -> int:\n        return value\n",
    "callable_list": IMPORT + "def use(values: list[Callable[[int], int]]) -> None:\n    pass\n",
    "callable_tuple": IMPORT + "def use(values: tuple[Callable[[int], int], int]) -> None:\n    pass\n",
    "callable_dictionary": IMPORT + "def use(values: dict[str, Callable[[int], int]]) -> None:\n    pass\n",
    "optional_without_guard": IMPORT + "def use(fn: Callable[[int], int] | None) -> int:\n    return fn(1)\n",
    "untyped_lambda": "def run() -> None:\n    fn = lambda value: value + 1\nrun()\n",
    "lambda_default": IMPORT + "def run() -> None:\n    fn: Callable[[int], int] = lambda value=1: value\nrun()\n",
    "indirect_cycle": IMPORT + "def apply(fn: Callable[[int], int], value: int) -> int:\n    return fn(value)\ndef again(value: int) -> int:\n    return apply(again, value)\nprint(apply(again, 1))\n",
    "map_wrong_input": FUNCTION + 'print(sum(map(identity, ["1"])))\n',
    "map_multiple_sources": FUNCTION + "print(sum(map(identity, [1], [2])))\n",
    "map_escaping": FUNCTION + "def run() -> None:\n    values = map(identity, [1])\nrun()\n",
    "filter_wrong_input": FUNCTION + 'print(len(list(filter(identity, ["1"]))))\n',
    "filter_wrong_predicate": "print(len(list(filter(1, [1]))))\n",
    "filter_missing_source": FUNCTION + "print(len(list(filter(identity))))\n",
    "filter_extra_source": FUNCTION + "print(len(list(filter(identity, [1], [2]))))\n",
    "filter_keyword": FUNCTION + "print(len(list(filter(identity, iterable=[1]))))\n",
    "filter_escaping": "def run() -> None:\n    values = filter(None, [1])\nrun()\n",
}

IMPORTED_REJECTIONS = {
    "callable_imported_global": {
        "main.py": "import callbacks\n",
        "callbacks.py": IMPORT + FUNCTION + "fn: Callable[[int], int] = identity\n",
    },
}

ERRORS = {
    "argument_failure": (
        IMPORT
        + "def target(value: int) -> int:\n    print(\"must not call\")\n    return value\n"
        + "def choose() -> Callable[[int], int]:\n    print(\"callee\")\n    return target\n"
        + "def argument() -> int:\n    print(\"argument\")\n    raise ValueError(\"argument failed\")\n"
        + "print(choose()(argument()))\n",
        "ValueError",
    ),
    "callback_failure": (
        "def convert(value: str) -> int:\n    print(value)\n    return int(value)\n"
        + 'print(sum(map(convert, ["1", "bad", "must not run"])))\n',
        "ValueError",
    ),
}


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors=ERRORS,
        label="callable value",
        description=__doc__,
    )
