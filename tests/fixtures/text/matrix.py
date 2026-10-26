from http import HTTPStatus


def find_status(start: HTTPStatus | None) -> int:
    return ("x" * 600).find("x", start)


print(("x" * 600).find("x", HTTPStatus.NOT_FOUND))
print(find_status(HTTPStatus.OK))
print(find_status(None))
for use_status in [True, False]:
    print(("x" * 600).find("x", HTTPStatus.CREATED if use_status else None))

sources: list[str] = [
    "", "a", "aaa", "a,b,,c,", "λ,🙂,,終,", "λ🙂a🙂🙂", " a  b c ",
    "\x1c a\u00a0b\u3000", "\t\n\x1f", "a\u200bb", "\0a\0b",
]
separators: list[str | None] = [None, ",", "aa", "🙂", "\0", ""]
limits: list[int] = [-3, -1, 0, 1, 2, 4, 9223372036854775807, 9223372036854775808, -9223372036854775809]
for source in sources:
    print(repr(source.split()))
    print(repr(source.rsplit()))
    for separator in separators:
        for limit in limits:
            try:
                print(repr(source.split(separator, limit)))
            except ValueError as error:
                print("ValueError:" + str(error))
            except OverflowError as error:
                print("OverflowError:" + str(error))
            try:
                print(repr(source.rsplit(maxsplit=limit, sep=separator)))
            except ValueError as error:
                print("ValueError:" + str(error))
            except OverflowError as error:
                print("OverflowError:" + str(error))

whitespace: list[str] = [
    "\t", "\n", "\v", "\f", "\r", "\x1c", "\x1d", "\x1e", "\x1f", " ",
    "\x85", "\u00a0", "\u1680", "\u2000", "\u2001", "\u2002", "\u2003", "\u2004",
    "\u2005", "\u2006", "\u2007", "\u2008", "\u2009", "\u200a", "\u2028", "\u2029",
    "\u202f", "\u205f", "\u3000", "\u200b", "\ufeff",
]
for space in whitespace:
    text = space + "λ🙂" + space
    print(repr(text.strip()))
    print(repr(text.lstrip()))
    print(repr(text.rstrip()))
    print(repr(text.split(None, 0)))
    print(repr(text.rsplit(None, 0)))

characters: list[str | None] = [None, "", "ab", "🙂λ", "\0"]
needles: list[str] = ["", "a", "b", "λ", "🙂", "a🙂", "not present", "\0"]
starts: list[int | None] = [None, -1000000000000000000000000000000, -5, -1, 0, 1, 2, 4, 5, 10, 1000000000000000000000000000000]
line_parts: list[str] = [
    "", "x", "\n", "\r", "\r\n", "\v", "\f", "\x1c", "\x1d", "\x1e",
    "\x1f", "\x85", "\u2028", "\u2029", "éλ🙂", "¢—",
]
chunk = "0123456789abcdef"
padding = chunk + chunk + chunk + chunk + "x"
for size in [31, 32, 33, 63, 64, 65]:
    for separator in line_parts:
        print(repr((padding[:size] + separator + "last\r\n").splitlines()))
for left in line_parts:
    for right in line_parts:
        print(repr((left + right).splitlines()))
        print(repr(("éλ🙂" + left + right + "\r\nlast").splitlines()))
for source in sources:
    for chars in characters:
        print(repr(source.strip(chars)))
        print(repr(source.lstrip(chars)))
        print(repr(source.rstrip(chars)))
    for needle in needles:
        print(source.startswith(needle))
        print(source.endswith(needle))
        print(source.find(needle))
        for start in starts:
            print(source.find(needle, start))
        try:
            print(repr(source.partition(needle)))
        except ValueError as error:
            print("ValueError:" + str(error))


for data in [b"", b"a\r\nb\r", b"a\xc2\x85b", b"a\xe2\x80\xa8b", b"a\n\xff"]:
    try:
        print(repr(data.decode("utf-8").splitlines()))
    except UnicodeDecodeError as error:
        print(str(error))
