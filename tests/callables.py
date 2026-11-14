"""Compare exact callable values, callback pipelines, and ownership with CPython."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "callables"
IMPORT = "from typing import Callable\n"
FUNCTION = "def identity(value: int) -> int:\n    return value\n"

REJECTED = {
    "redefined_first_return": (
        'def f(n: int) -> int:\n    return "bad"\n'
        'old = f\ndef f(n: int) -> int:\n    return n\nprint(old(1), f(1))\n'
    ),
    "redefined_second_return": (
        'def f(n: int) -> int:\n    return n\n'
        'old = f\ndef f(n: int) -> int:\n    return "bad"\nprint(old(1), f(1))\n'
    ),
    "overload_result_annotation": "from typing import get_overloads\nvalues: list[int] = get_overloads(sum)\n",
    "overload_bound_or_plain": (
        "from typing import Callable, overload\n"
        "def register(callback: Callable[[], int]) -> None:\n    overload(callback)\n"
        "def source() -> int: return 1\n"
        "class Reader:\n    def read(self) -> int: return 2\n"
        "register(source)\nregister(Reader().read)\n"
    ),
    "local_callback_captured_map": "def outer(x: int) -> int:\n    def add(y: int) -> int:\n        return x + y\n    return sum(map(add, [1]))\n",
    "local_recursive": "def outer(x: int) -> int:\n    def inner(y: int) -> int:\n        return inner(y)\n    return inner(x)\n",
    "local_recursive_via_outer": "def outer(x: int) -> int:\n    def inner(y: int) -> int:\n        return outer(y)\n    return inner(x)\n",
    "local_forward": "def outer() -> int:\n    value = inner()\n    def inner() -> int:\n        return 1\n    return value\n",
    "local_conditional": "def outer(flag: bool) -> int:\n    if flag:\n        def inner() -> int:\n            return 1\n    return inner()\n",
    "local_uninitialized_capture": "def outer(flag: bool) -> int:\n    x: int\n    def get() -> int:\n        return x\n    if flag:\n        x = 1\n    return get()\n",
    "local_nonlocal": "def outer(x: int) -> int:\n    def inner() -> int:\n        nonlocal x\n        x += 1\n        return x\n    return inner()\n",
    "local_rebound": "def outer() -> int:\n    def f() -> int:\n        return 1\n    f = 1\n    return f\n",
    "ellipsis_signature": IMPORT + "def use(fn: Callable[..., int]) -> int:\n    return fn(1)\n",
    "wrong_parameter": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[str], int] = identity\nrun()\n",
    "wrong_return": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], str] = identity\nrun()\n",
    "bool_is_not_int": IMPORT + FUNCTION + "def run() -> None:\n    fn: Callable[[int], int] = identity\n    print(fn(True))\nrun()\n",
    "raising_before_declared_argument": (
        IMPORT + FUNCTION
        + 'def stop() -> int:\n    raise ValueError("stop")\n'
        + "def pair(x: int, y: int) -> int:\n    return x + y\n"
        + "def run(f: Callable[[int, int], int]) -> int:\n"
        + "    return f(stop(), identity('x'))\nprint(run(pair))\n"
    ),
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
    "lambda_default": IMPORT + "def run() -> None:\n    fn: Callable[[int], int] = lambda value=1: value\nrun()\n",
    "indirect_cycle": IMPORT + "def apply(fn: Callable[[int], int], value: int) -> int:\n    return fn(value)\ndef again(value: int) -> int:\n    return apply(again, value)\nprint(apply(again, 1))\n",
    "map_wrong_input": FUNCTION + 'print(sum(map(identity, ["1"])))\n',
    "map_multiple_sources": FUNCTION + "print(sum(map(identity, [1], [2])))\n",
    "filter_wrong_input": FUNCTION + 'print(len(list(filter(identity, ["1"]))))\n',
    "filter_wrong_predicate": "print(len(list(filter(1, [1]))))\n",
    "filter_missing_source": FUNCTION + "print(len(list(filter(identity))))\n",
    "filter_extra_source": FUNCTION + "print(len(list(filter(identity, [1], [2]))))\n",
    "filter_keyword": FUNCTION + "print(len(list(filter(identity, iterable=[1]))))\n",
    "filter_escaping": "def run() -> None:\n    values = filter(None, [1])\nrun()\n",
}

IMPORTED_REJECTIONS = {
    "callable_imported_argument": {
        "main.py": "import callbacks\nprint(callbacks.fn('wrong'))\n",
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


def check_redefinition_error(name: str, stderr: str) -> None:
    if name == "raising_before_declared_argument":
        assert "main.py:9:31: Imre: expected int, got str" in stderr, stderr
    line = {"redefined_first_return": 2, "redefined_second_return": 5}.get(name)
    if line is not None:
        assert f"main.py:{line}:12: Imre: expected int, got str" in stderr, (name, stderr)


if __name__ == "__main__":
    main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors=ERRORS,
        check_rejection=check_redefinition_error,
        label="callable value",
        description=__doc__,
    )
