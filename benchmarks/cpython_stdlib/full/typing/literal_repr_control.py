from typing import Literal
import sys

alias = Literal[Literal[0], False, 0]
actual = repr(alias)
expected = "typing.Literal[0]" if "wrong" in sys.argv else "typing.Literal[0, False]"
print("LITERAL_CONTROL", actual)
if actual != expected:
    raise AssertionError(("literal.repr", actual, expected))
