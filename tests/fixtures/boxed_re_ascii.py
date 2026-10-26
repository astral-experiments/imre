import re


def current():
    ascii_digits = re.compile(r"-?\d+", flags=re.ASCII)
    unicode_digits = re.compile(r"-?\d+")
    print(re.A is re.ASCII, re.U is re.UNICODE)
    print(ascii_digits.pattern, ascii_digits.flags, unicode_digits.flags)
    for text in ("200", "-404", "+12", "1_2", "٢٠٠", "-１２", "200\n"):
        print(text, ascii_digits.fullmatch(text) is not None, unicode_digits.fullmatch(text) is not None)
    print(re.fullmatch(r"(a|ab)", "ab", re.A).group(1))
    print(re.fullmatch(r"[^\d].", "٢é", re.ASCII).group())
    print(re.compile(ascii_digits) is ascii_digits)
    for pattern, flags in ((ascii_digits, re.ASCII), ("a", re.ASCII | re.UNICODE)):
        try:
            re.compile(pattern, flags)
        except ValueError as error:
            print(str(error))

    newline = re.compile("[\r\n]")
    for value in ("application/json", "12", "é\rvalue", "é\nvalue"):
        print(newline.search(value) is not None)
    grouped = re.compile("(?P<letter>β+)(x)?")
    print(grouped.search(string="αββ", pos=1, endpos=3).group(0, "letter", 2))
    print(re.search(grouped, "αβ").group("letter"))
    print(re.search(r"\d", "٣2", flags=re.ASCII).group())
    print(re.compile("β$").search("αβx", 1, 2).group())
    print(re.compile("^β").search("αβ", 1) is None)
    print(re.compile("").search("α", 20) is not None, newline.search("α\n", 2, 1) is None)
    for args in ((1,), (b"x",), ("x", None), ("x", 10**100)):
        try:
            newline.search(*args)
        except (TypeError, OverflowError) as error:
            print(str(error))
    for args, kwargs in (((), {}), (("x",), {"string": "y"}), (("x",), {"missing": 1})):
        try:
            newline.search(*args, **kwargs)
        except TypeError as error:
            print(str(error))
