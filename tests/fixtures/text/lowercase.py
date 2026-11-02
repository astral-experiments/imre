"""Default lowercasing includes expansions and the context-sensitive final sigma."""

from enum import StrEnum


class CaseMember(StrEnum):
    MEMBER = "ǳẞ"


for word in ["", "CPython-3.14_", "already lower", "İ", "ẞ", "𐐧𐐧",
             "Σ", "ΟΣ", "ΟΣΑ", "Ο\u0301Σ", "ΟΣ\u0301Α", "ΟΣ\u0301.",
             "A'Σ", "AΣ'A", "AΣ\u0345.", "AΣ\u0345A", "\0Σ", "Σ\0"]:
    print(word.lower())

# Title uses the previous source character. Swapcase leaves titlecase-only
# digraphs alone, while mapping uppercase sigma in the original context.
title = str.title
swapcase = str.swapcase
assert title("ǳǲǱ") == "ǲǳǳ"
assert swapcase("ǳǲǱ") == "Ǳǲǳ"
assert "ß ﬃ".title() == "Ss Ffi"
assert "ß ﬃ".swapcase() == "SS FFI"
assert "AΣͅA".title() == "Aσͅa"
assert "AΣͅ.".swapcase() == "aςΙ."
assert "A'Σ".title() == "A'Σ"
assert "A'Σ".swapcase() == "a'ς"
assert "a\u0301B".title() == "A\u0301B"
assert swapcase("İ") == "i\u0307"

plain = "!"
assert plain.title() is not plain
assert plain.swapcase() is not plain
empty = ""
assert title(empty) is empty
assert swapcase(empty) is empty
try:
    "x".title(1)
except TypeError as error:
    print(error)
try:
    str.swapcase("x", extra=1)
except TypeError as error:
    print(error)

assert str.title(CaseMember.MEMBER) == "ǲß"
assert str.swapcase(CaseMember.MEMBER) == "Ǳß"
assert CaseMember.MEMBER.swapcase() == "Ǳß"
