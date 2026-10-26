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
