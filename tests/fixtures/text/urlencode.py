from enum import StrEnum
from http import HTTPStatus
from urllib.parse import quote, quote_plus, urlencode, urlsplit, urlunsplit


class Word(StrEnum):
    PATH = "a /b"
    SAFE = "/"
    CODEC = "ascii"
    ERRORS = "strict"


pairs = [("path", "one two/three"), ("path", "a+b"), ("", "&")]
def mutate_before_encoding(values: list[tuple[str, str]]) -> str:
    values.append(("last", "now"))
    return "/"


class Doseq:
    def __bool__(self) -> bool:
        print("doseq once")
        return True


print(urlencode([("one", "two"), ("three", "four")], doseq=Doseq()))
print(urlencode([("number", [1, 2])], doseq=1))
print(urlencode({"keys": {"x": 0, "y": 1}}, doseq=True))
print(urlencode(pairs, safe=mutate_before_encoding(pairs)))
print(urlencode(pairs))
print(urlencode(pairs, safe=b"/", quote_via=quote))
print(urlencode(pairs, safe="!$'()*,/:;?@"))
print(urlencode({"more": [1, 2], "empty": []}, doseq=True))
print(urlencode({"more": [1, 2], "empty": []}))
print(urlencode(((b"\xff", "Snow ☃"), ("x", (4, "ü"))), True, safe="", encoding="latin-1", errors="replace"))
print(urlencode((("key", 1267650600228229401496703205376),)))
print(urlencode({}))
print(urlencode([]))
print(urlencode([(Word.PATH, HTTPStatus.OK)], safe=Word.SAFE, encoding=Word.CODEC, errors=Word.ERRORS))
print(urlencode([(HTTPStatus.NOT_FOUND, [Word.PATH, Word.SAFE])], doseq=True, quote_via=quote))
print(quote_plus(b"a b+c/%", safe="/"))
print(quote_plus("a b+c/%", safe="/", encoding="cp1251"))
print(quote_plus("Ё", encoding="windows-1251"))

print(urlunsplit(("https", "example.com", "p/☃", "a=1", "x")))
print(urlunsplit(components=("file", "", "/tmp/a", "", "")))
print(urlunsplit(("", "", "//path", "", "")))
print(urlunsplit((b"https", b"host", b"", b"x", b"y")).decode())
print(urlunsplit(urlsplit("https://host?a=+&a=%20#xyz")))

try:
    print(urlencode({"snow": "☃"}, encoding="ascii"))
except UnicodeEncodeError as error:
    print(str(error))
