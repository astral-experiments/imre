"""Supplemental native checks; these are not additional upstream test IDs."""
from string import ascii_uppercase as upper, capwords, digits

print(upper)
print(digits)
for text in ["", "A", "123ABC", "ßETA", "ﬃOO", "ǳABC", "İSTANBUL", "ΣA", "AΣ", "AΣ\u0301", "AΣ\u0301A", "\u0301ABC", "𐐨ABC", "a\x00B"]:
    print(repr(text.capitalize()))
for text in ["", "\t\r\n", "\x1ca\x1dB\x1e c\x1f", "a\x85b\xa0c\u2003d\u2029e", "ǳABC ﬃrst AΣ", "a--B----ßX--", "a\x00B\x00"]:
    print(repr(capwords(text)))
    print(repr(capwords(text, None)))
    print(repr(capwords(text, "--")))
    print(repr(capwords(text, "\x00")))
    try:
        print(capwords(text, ""))
    except ValueError as exc:
        print(str(exc))
