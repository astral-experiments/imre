def number(value: int | None) -> int:
    if value:
        return value + 1
    return -1


def fallback(value: str | None) -> str:
    return value or "fallback"


def conjunction(value: int | None) -> int | None:
    return value and 7


def show(value: int | None) -> None:
    if value is None:
        print("none")
    else:
        print(value)


def guarded(value: str | None) -> str:
    if not value:
        return "empty"
    return value + "!"


def matching(value: str | None) -> str:
    match value:
        case captured if captured:
            return captured + "?"
        case _:
            return "empty"


numbers: list[int | None] = [None, 0, 4]
for value in numbers:
    print(number(value))
    show(conjunction(value))
    show(value or None)
texts: list[str | None] = [None, "", "present"]
for text in texts:
    print(fallback(text))
    print(guarded(text))
    print(matching(text))
    if saved := text:
        print(saved + ":bound")
values: list[str | None] = [None, "", "a", "b"]
words = [value + "!" for value in values if value]
print(len(words))
print(words[0])
print(words[1])
optional: float | None = -0.0
print(bool(optional))
optional = float("nan")
print(bool(optional))
if optional:
    print(optional + 1.0)
optional = None
print(bool(optional))
flag: bool | None = False
print(bool(flag))
flag = True
print(bool(flag))
