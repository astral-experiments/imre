import hmac
from hmac import compare_digest


def checked(left: bytes, right: bytes) -> bool:
    return hmac.compare_digest(left, right)


def marked(text: str) -> str:
    print(text)
    return text


print((
    checked(b"\x00\xff", b"\x00\xff"),
    checked(b"\x00\xff", b"\x00\xfe"),
    checked(b"", b"x"),
))
print((compare_digest("a1ff", "a1ff"), compare_digest("a1ff", "01ff")))
try:
    compare_digest(marked("é"), marked("right"))
except TypeError as exc:
    print(str(exc))
try:
    compare_digest(b"abc", marked("text"))
except TypeError as exc:
    print(str(exc))
try:
    compare_digest("ascii", 1)
except TypeError as exc:
    print(str(exc))
