def combine(first, /, second=4, *, factor=2):
    return (first + second) * factor


def text(value="word", *, ending="!"):
    return value + ending


def enabled(*, value=False):
    return not value


def decimal(value=1.5):
    return value * 2.0


def payload(value=b"data"):
    return value + b"!"


def absent(value=None):
    return value


def accept(*, value) -> None:
    print(value)


print(combine(3))
print(combine(3, factor=5, second=1))
print(text())
print(text(ending="?", value="hello"))
print(enabled())
print(enabled(value=True))
print(decimal())
print(decimal(-2.0))
print(payload())
print(payload(b"other"))
print(absent())
accept(value=17)
