# Module annotation code starts at the first statement, even with leading comments.

import sys
from annotationlib import Format, ForwardRef, get_annotations


before = __annotate__(1)
Alias = int
first: Alias
if False:
    skipped: Unset
last: Later
(nonsimple): Unset
Alias = str
calls = []


def counted():
    calls.append(1)
    return len(calls)


for fail in (False, True):
    try:
        try:
            if fail:
                raise ValueError
        finally:
            item: counted()
    except ValueError:
        pass
try:
    entered: int
except ValueError:
    failed: bool
else:
    completed: str


def current():
    global Later
    module = sys.modules[__name__]
    assert __annotate__.__code__.co_firstlineno == 3
    assert before == {} and __conditional_annotations__ == {0, 2, 3, 4, 5, 6}
    try:
        __annotate__(1)
    except NameError:
        pass
    else:
        raise AssertionError("missing name was resolved")
    hints = get_annotations(module, format=Format.FORWARDREF)
    assert hints["first"] is str and isinstance(hints["last"], ForwardRef)
    assert hints["last"].__forward_arg__ == "Later"
    assert hints["last"].__owner__ is module
    assert get_annotations(module, format=Format.STRING) == {
        "first": "Alias", "skipped": "Unset", "last": "Later",
        "item": "counted()", "entered": "int", "completed": "str", "failed": "bool",
    }
    Later = bytes
    calls.clear()
    hints = __annotate__(1)
    assert calls == [1, 1]
    assert list(hints.items()) == [
        ("first", str), ("last", bytes), ("item", 2), ("entered", int), ("completed", str)
    ]
    calls.clear()
    assert module.__annotations__ == hints
    assert module.__annotations__ is module.__annotations__ and calls == [1, 1]
