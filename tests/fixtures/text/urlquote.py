from urllib.parse import quote, unquote

def encoded(value: str, codec: str | None, errors: str | None) -> str:
    return quote(value, errors=errors, encoding=codec)

def decoded(value: bytes, codec: str | None, errors: str | None) -> str:
    return unquote(value, encoding=codec, errors=errors)

def mark(value: str) -> str:
    print("evaluated " + value)
    return value


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
