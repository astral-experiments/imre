# ruff: noqa: B012, PLW0120
# A break from finally contributes a distinct definite-assignment path.


def find(values: list[int], wanted: int) -> int:
    for value in values:
        if value == wanted:
            found = value
            break
    else:
        found = -1
    return found


def count_down(value: int) -> int:
    while value > 0:
        if value == 2:
            found = value
            break
        value -= 1
    else:
        found = -1
    return found


def finally_find(values: list[int]) -> str:
    for value in values:
        try:
            continue
        finally:
            if value == 2:
                result = "found"
                break
    else:
        result = "absent"
    return result


def finally_assigned(values: list[int]) -> int:
    for value in values:
        try:
            break
        finally:
            result = value + 1
    else:
        result = -1
    return result


def optional(value: int | None) -> int:
    while value is None:
        value = 4
    else:
        return value + 1


def fill(value: str | None) -> str:
    while not value:
        value = "filled"
    else:
        return value + "!"


print(find([], 2))
print(find([1, 2, 3], 2))
print(find([1, 2, 3], 4))
print(count_down(0))
print(count_down(1))
print(count_down(3))
print(finally_find([]))
print(finally_find([1, 2]))
print(finally_find([1, 3]))
print(finally_assigned([]))
print(finally_assigned([3]))
print(optional(None))
print(optional(0))
print(optional(7))
print(fill(None))
print(fill(""))
print(fill("existing"))
