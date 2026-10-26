def choose(value: int | None, case: int) -> int:
    if case == 0:
        if not value:
            return value + 1
        return 0
    if case == 1:
        if value:
            print(value + 1)
        return value + 1
    if case == 2:
        if value or (value := None) is None:
            return value + 1
        return 0
    while value:
        value = None
    return value + 1


def suffix(value: str | None) -> str:
    if value:
        return value
    return value + "!"


numbers: list[int | None] = [0, 2, None]
for case in range(4):
    for number in numbers:
        try:
            print(choose(number, case))
        except TypeError as exc:
            print(str(exc))

texts: list[str | None] = ["x", "", None]
for text in texts:
    try:
        print(suffix(text))
    except TypeError as exc:
        print(str(exc))
