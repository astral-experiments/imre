import unicodedata
from unicodedata import category


def codepoint(value: int) -> int:
    print(value)
    return value


for char in ["A", "7", "\u0301", "\u0295", "\u2029", "\ue000", "\u0378", "\U0001fa8a"]:
    print(unicodedata.category(char))
for point in [65, 0xd800, 0xdfff, 0x10ffff, -1, 0x110000, 1 << 100]:
    try:
        print(category(chr(codepoint(point))))
    except ValueError as error:
        print(str(error))

first = category("A")
second = category("B")
print(first == second, first is second)
try:
    category("ab")
except TypeError as error:
    print(str(error))
try:
    category(chr=codepoint(65))
except TypeError as error:
    print(str(error))
