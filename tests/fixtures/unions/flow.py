def show(value: str | bytes) -> str:
    if isinstance(value, bytes):
        return value.decode()
    return value


def pair(first: str | bytes, second: str | bytes) -> str:
    return show(first) + ":" + show(second)


def run(value: str | bytes) -> None:
    print(pair(value, (value := b"replaced")))
    print(show(value))
    if isinstance(value, str) or not isinstance(value, bytes):
        print("unreachable")
    if not isinstance(value, str) and isinstance(value, bytes):
        print(value.decode())
    values = [value.decode() for index in range(2) if isinstance(value, bytes)]
    for item in values:
        print(item)
    values = [show(value := "text" if index == 0 else b"bytes") for index in range(2)]
    for item in values:
        print(item)
    print(show(value))


def guarded(value: str | bytes | None) -> str:
    if value is not None and isinstance(value, str):
        return value + " guarded"
    if isinstance(value, bytes):
        return value.decode()
    return "none"


def optional_binding(value: str | bytes) -> str:
    holder: str | bytes | None = None
    result = (holder := value)
    print(show(result))
    if holder is not None:
        return show(holder)
    return "none"


run("original")
run(b"initial bytes")
print(guarded("text"))
print(guarded(b"bytes"))
print(guarded(None))
print(optional_binding("optional text"))
print(optional_binding(b"optional bytes"))
