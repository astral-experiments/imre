from urllib.parse import unquote


def decode_path(policy: str) -> str:
    return unquote("café%2F%FF", encoding="utf-8", errors=policy)


assert decode_path("surrogateescape").encode("utf-8", "surrogateescape") == b"caf\xc3\xa9/\xff"
assert ord(unquote(b"%80", encoding="ascii", errors="surrogateescape")) == 0xDC80
assert ord(unquote("%ED%A0%80", errors="surrogatepass")) == 0xD800
assert decode_path("replace") == "café/\ufffd"

try:
    decode_path("strict")
except UnicodeDecodeError as error:
    assert error.object == b"/\xff" and error.start == 1
else:
    raise AssertionError("decoded an invalid UTF-8 byte")
