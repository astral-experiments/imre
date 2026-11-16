def bad(left: str | bytes, right: str | bytes) -> bool:
    return left == right


print(bad("a", "a"))
print(bad("a", "b"))
print(bad(b"a", b"a"))
print(bad(b"a", b"b"))
print(bad("a", b"a"))
print(bad("a", b"b"))
print(bad(b"a", "a"))
print(bad(b"a", "b"))
