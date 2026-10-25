"""Compare real module frames, index conversion, and exception unwinding."""
import sys
from frame_names_owner import inspect_call, raising, report


class CallerDepth:
    def __index__(self) -> int:
        return 1


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
