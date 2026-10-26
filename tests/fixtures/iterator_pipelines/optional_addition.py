def observed(value: int | None) -> int | None:
    if value is None:
        print("pull None")
    else:
        print(f"pull {value}")
    return value


def total(values: list[int | None]) -> int:
    return sum(value + 1 for value in values)


def visit(values: list[int | None], first_only: bool) -> None:
    print("start")
    try:
        for value in (observed(item) + 1 for item in values):
            print(f"yield {value}")
            if first_only:
                break
    except TypeError as error:
        print("TypeError: " + str(error))
    print("end")


def guarded(value: int | None) -> None:
    if any(value is not None for index in range(1)):
        print(value + 1)


guarded(None)
guarded(2)

print(total([1, 2]))
visit([1, 2], False)
visit([1, None, 9], True)
visit([1, None, 9], False)
visit([None], False)
