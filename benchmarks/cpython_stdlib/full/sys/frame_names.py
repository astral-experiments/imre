"""Compare real module frames, index conversion, and exception unwinding."""
import sys
from frame_names_owner import current_name, inspect_call, raising, report


class CallerDepth:
    def __index__(self) -> int:
        return 1


def nested_name() -> str | None:
    return current_name()


def outer() -> None:
    inspect_call()
    try:
        raising()
    except ValueError:
        report(sys._getframemodulename())
        report(sys._getframemodulename(CallerDepth()))
        report(sys._getframemodulename(2))
    report(sys._getframemodulename(2000000000))
    try:
        sys._getframemodulename(1 << 80)
    except OverflowError as error:
        print(str(error))
    try:
        sys._getframemodulename("bad depth")
    except TypeError as error:
        print(str(error))
    try:
        sys._getframemodulename(1, 2)
    except TypeError as error:
        print(str(error))
    try:
        sys._getframemodulename(missing=0)
    except TypeError as error:
        print(str(error))
    try:
        sys._getframemodulename(0, depth=1)
    except TypeError as error:
        print(str(error))


report(sys._getframemodulename())
report(sys._getframemodulename(1))
outer()
report(sys._getframemodulename())
report(sys._getframemodulename(1))

# A failed entry must pop its named frame before the next boundary check.
old_limit = sys.getrecursionlimit()
for limit in (2, 3, 2, 3):
    sys.setrecursionlimit(limit)
    try:
        name = nested_name()
    except RecursionError:
        name = None
    finally:
        sys.setrecursionlimit(old_limit)
    if limit == 2:
        assert name is None
    else:
        assert name is not None
        assert name == "frame_names_owner"
    assert sys._getframemodulename() is __name__
