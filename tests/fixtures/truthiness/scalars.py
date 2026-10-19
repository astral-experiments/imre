class Box:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


print(bool())  # noqa: UP018 - exercise the zero-argument constructor.
print(bool(None))
print(not None)
for number in [0, 1, -1, 1000000000000000000000000000000]:
    print(bool(number))
    print(not number)
    print(number or 19)
    print(number and 23)
for decimal in [0.0, -0.0, 1.25, -2.5, float("nan"), float("inf")]:
    print(bool(decimal))
    print(not decimal)
    print(decimal or 1.0)
    print(decimal and -0.0)
for flag in [False, True]:
    print(bool(flag))
    print(not flag)
for text in ["", "\0", "é🌍"]:
    print(bool(text))
    print(text or "fallback")
    print(text and "selected")
for data in [b"", b"\0", b"\xff"]:
    print(bool(data))
    print(data or b"fallback")
    print(data and b"selected")
empty = ()
single = (None,)
pair = (0, "")
print(bool(empty))
print(bool(single))
print(bool(pair))
print(not Box(0))
print(bool(Box(0)))
print(1 if "present" else 2)
print(1 if "" else 2)


def conditional(value: int | None) -> int:
    return 1 if value else 2


print(conditional(None))
print(conditional(0))
print(conditional(3))
