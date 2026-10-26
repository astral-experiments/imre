def add_first(values: list[int | None]) -> int:
    first, = values
    return first + 1


print(add_first([0]))
print(add_first([2]))
try:
    print(add_first([None]))
except TypeError as exc:
    print(str(exc))
