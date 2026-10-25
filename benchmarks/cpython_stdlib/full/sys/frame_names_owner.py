"""A different source module from the entry that inspects the native call stack."""
import sys


def current_name() -> str | None:
    return sys._getframemodulename()


def report(value: str | None) -> None:
    if value is None:
        print("<absent>")
    else:
        print(value)


def inspect_call() -> None:
    report(sys._getframemodulename())
    report(sys._getframemodulename(depth=1))
    report(sys._getframemodulename(2))
    report(sys._getframemodulename(3))
    found = sys._getframemodulename(-1)
    if found is None:
        raise AssertionError("missing current module")
    print(found is __name__)


def raising() -> None:
    report(sys._getframemodulename())
    report(sys._getframemodulename(True))
    raise ValueError("unwind the imported frame")
