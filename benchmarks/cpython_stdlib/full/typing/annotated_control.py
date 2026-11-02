"""Independent losing-value comparison for inner-to-outer metadata order."""
from typing import Annotated


def check(wrong=False):
    alias = Annotated[Annotated[int, "inner"], "outer"]
    actual = alias.__metadata__
    expected = ("outer", "inner") if wrong else ("inner", "outer")
    print("ANNOTATED_METADATA", repr(actual), flush=True)
    if actual != expected:
        raise AssertionError(("annotated.metadata_order", actual, expected))
