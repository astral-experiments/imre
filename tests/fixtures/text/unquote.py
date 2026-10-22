import urllib.parse
import urllib.parse as urls
from urllib import parse
from urllib.parse import unquote as decode

values: list[str] = [
    "", "ordinary λ🙂+", "%", "%0", "%xy", "a%20b%2Bc", "%00tail", "%ff",
    "%E1%80", "%ED%A0%80", "%F0%9F%99%82", "%F4%90%80%80", "%C0%AF", "%E9é%80",
]
for value in values:
    print(repr(urllib.parse.unquote(value)))
    print(repr(urls.unquote(string=value)))
    print(repr(parse.unquote(value)))
    print(repr(decode(value)))
    print(repr(decode(("ignored%00/λ/" + value).rsplit("/", maxsplit=1)[-1])))
    print(repr(decode(value.rsplit("λ🙂", maxsplit=1)[-1])))

try:
    print(decode("%41".rsplit("", 1)[-1]))
except ValueError as error:
    print(str(error))

# Every possible byte and pair exercises invalid UTF-8 replacement grouping.
hexadecimal = "0123456789ABCDEF"
for a in hexadecimal:
    for b in hexadecimal:
        first = "%" + a + b
        print(repr(decode(first)))
        for c in hexadecimal:
            for d in hexadecimal:
                print(repr(decode(first + "%" + c + d)))
