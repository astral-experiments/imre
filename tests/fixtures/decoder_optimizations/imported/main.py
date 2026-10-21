import library
from bridge import ALIAS
from library import (
    TABLE,
    mutated_bytes_source,
    mutated_source,
    translate,
    translate_bytes,
)


def show(values: list[int]) -> None:
    for value in values:
        print(value)


show(translate("aabbé"))
retained = translate("bb")
retained_bytes = translate_bytes(b"bb")
ALIAS["a"] = 19
TABLE["b"] = -(1 << 140)
show(translate("aabbé"))
show(retained)
show(retained_bytes)
show(translate_bytes(b"aabb\xc3\xa9"))
show([library.TABLE[character] for character in "ébaa"])
show(mutated_source())
show(mutated_bytes_source(False))
try:
    mutated_bytes_source(True)
except UnicodeDecodeError as error:
    print(str(error))
print(TABLE["a"])
del ALIAS["a"]
try:
    translate("baé")
except KeyError as error:
    print(str(error))
try:
    translate_bytes(b"ba\xc3\xa9")
except KeyError as error:
    print(str(error))
ALIAS["a"] = 0
show(translate("aabbé"))
print(len(translate("")))
print(len(translate_bytes(b"")))
