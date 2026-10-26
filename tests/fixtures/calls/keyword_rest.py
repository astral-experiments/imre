"""Typed keyword rest preserves evaluation order, spelling and fresh storage."""


def observed(number: int) -> int:
    print(number)
    return number


def describe(prefix: str, /, **values: int) -> str:
    result = prefix
    for key in values:
        result += ":" + key + "=" + str(values[key])
    values["local"] = 99
    return result


print(describe("empty"))
print(describe("one", first=1))
print(describe("ordered", z=observed(20), prefix=observed(4), a=observed(7)))
print(describe("fresh"))
