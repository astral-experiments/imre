def mark(label: str, value: int) -> int:
    print(label)
    return value


def pair(fmt: str) -> str:
    return fmt % (mark("first", 7), mark("second", 12))


def empty(fmt: str) -> str:
    return fmt % ()


def text(fmt: str, item: str) -> str:
    return fmt % (item,)


print("int %s" % (12,))
print("%s / %r / %s" % (None, b"x", True))
print("[%*.*f]" % (8, 2, 1.25))
print(text("[%s], %%, done", "café"))
print(pair("%02d:%x"))
print(empty("literal%%"))
try:
    print(pair("%d"))
except TypeError as error:
    print(str(error))
try:
    print(empty("%s"))
except TypeError as error:
    print(str(error))
try:
    print(text("🙂 %q", "bad"))
except ValueError as error:
    print(str(error))
