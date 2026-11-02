import sys
from typing import Literal

members = dir(Literal[1, 2, 3])
actual = ("__args__" in members, "__parameters__" in members, "__origin__" in members)
expected = (True, True, False) if "wrong" in sys.argv else (True, True, True)
print("LITERAL_DIR", actual)
if actual != expected:
    raise AssertionError(("literal.dir", actual, expected))
