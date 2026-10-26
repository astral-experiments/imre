"""Convert the exact str or bytes returned by the upstream FakePath descriptor."""
import sys
from os import fsdecode, fsencode

path: str | bytes
if sys.argv[2] == "bytes":
    with open("input", "rb") as binary:
        path = binary.read()
else:
    with open("input", encoding="utf-8") as text:
        path = text.read()
if sys.argv[1] == "fsencode":
    print(repr(fsencode(path)))
else:
    print(repr(fsdecode(path)))
