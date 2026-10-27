from enum import StrEnum
import urllib.parse as urlparse
from urllib.parse import quote, unquote, urlunsplit
from urllib.parse import quote_from_bytes
from urllib.parse import quote_from_bytes as escape_bytes

class UrlWord(StrEnum):
    PATH = "a/b!é"
    ESCAPED = "a%2Fb%21"
    SAFE = "!"
    CODEC = "latin-1"
    ERROR = "ignore"

def enum_options(codec: UrlWord | None, errors: UrlWord | None) -> str:
    return unquote("caf%E9", encoding=codec, errors=errors)

def components(calls: list[int]) -> tuple[str, str, UrlWord, str, UrlWord]:
    calls.append(1)
    return "https", "example.org", UrlWord.PATH, "", UrlWord.ESCAPED

def encoded(value: str, codec: str | None, errors: str | None) -> str:
    return quote(value, errors=errors, encoding=codec)

def decoded(value: bytes, codec: str | None, errors: str | None) -> str:
    return unquote(value, encoding=codec, errors=errors)

def mark(value: str) -> str:
    print("evaluated " + value)
    return value


print(unquote(b"abc"))
print(quote_from_bytes(b"/% \x00\xff~"))
print(escape_bytes(safe=b"%\xff", bs=b"/% \xff"))
print(urlparse.quote_from_bytes(b"", safe=""))
print(urlparse.quote_from_bytes(b"=+/", safe="=é"))
try:
    print(escape_bytes(safe=mark("!"), bs=mark("wrong")))
except TypeError as error:
    print(str(error))
try:
    print(quote_from_bytes(b"x", encoding=mark("utf-8")))
except TypeError as error:
    print(str(error))
try:
    print(escape_bytes(b"/", saf=mark("misspelled")))
except TypeError as error:
    print(str(error))
try:
    print(escape_bytes(b"x", bs=mark("second")))
except TypeError as error:
    print(str(error))
print(quote(UrlWord.PATH, safe=UrlWord.SAFE, encoding=UrlWord.CODEC, errors=UrlWord.ERROR))
print(unquote(UrlWord.ESCAPED))
print(enum_options(UrlWord.CODEC, UrlWord.ERROR))
print(enum_options(None, None))
print(unquote("abc", encoding="ascii"))
print(unquote("abc", errors="strict"))
print(quote("a /%+é☃", safe=b"/%"))
print(quote(b"\xff\x00 =+/", safe="é=+"))
print(quote("雪ü", safe="", encoding="iso-8859-1", errors="xmlcharrefreplace"))
print(quote("Ё", encoding="windows-1251"))
print(unquote("雪%FC+☃%e9", encoding="latin-1"))
print(unquote(b"raw:\xff%2B%", errors="backslashreplace"))
print(unquote("x%FF雪%ab", encoding="ascii", errors="ignore"))
print(unquote("雪%EF%BB%BFok", encoding="utf-8-sig"))
print(unquote("%xz/%2/%/%%61", None, None))
print(quote("", encoding="invalid", errors="invalid"))
print(unquote(b"", encoding="invalid"))
print(unquote("unchanged雪", encoding="invalid", errors="bad\x00handler"))

print(encoded("a=b ü", None, None))
print(decoded(b"%FA", "latin-1", "strict"))
print(quote(safe=mark("!"), encoding=mark("ascii"), string=mark("! a")))
try:
    print(quote(b"", encoding="utf-8", errors="strict"))
except TypeError as error:
    print(str(error))
try:
    print(quote(b"", errors="replace"))
except TypeError as error:
    print(str(error))
try:
    print(decoded(b"", None, "strict"))
except TypeError as error:
    print(str(error))
try:
    print(decoded(b"", "utf-8", None))
except TypeError as error:
    print(str(error))
try:
    print(unquote("a雪p%ffx", encoding="ascii", errors="strict"))
except UnicodeDecodeError as error:
    print(str(error))
    print(repr(error.args))
try:
    print(quote("x雪風y", encoding="ascii"))
except UnicodeEncodeError as error:
    print(str(error))
    print(repr(error.args))
try:
    print(unquote("%ff", encoding="no_such_codec"))
except LookupError as error:
    print(str(error))
try:
    print(unquote("%ff", errors="no_such_handler"))
except LookupError as error:
    print(str(error))
try:
    print(quote("a", encoding="no_such_codec", errors="bad\x00handler"))
except ValueError as error:
    print(str(error))

calls: list[int] = []
print(urlunsplit(components(calls)))
assert len(calls) == 1
