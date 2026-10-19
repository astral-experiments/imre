# ruff: noqa: SIM222, SIM223
# Constant operands must still exercise short-circuit evaluation and truth contexts.


def integer(log: list[int], mark: int, value: int) -> int:
    log.append(mark)
    return value


def fail() -> int:
    raise ValueError("must be skipped")


def exercise(value: int) -> None:
    log: list[int] = []
    print(integer(log, 1, value) and integer(log, 2, 0) and integer(log, 3, 9))
    print(integer(log, 4, value) or integer(log, 5, 0) or integer(log, 6, 9))
    for mark in log:
        print(mark)
    saved = 4
    print(value and (saved := 8))
    print(saved)
    print(value or (saved := 12))
    print(saved)
    if value and (bound := 7):
        print(bound)
    if value or (other := 0):
        print("true")
    else:
        print(other)
    if not (value or (last := 0)):
        print(last)


exercise(0)
exercise(2)
print(0 and fail())
print(1 or fail())
groups: list[list[int]] = [[], [1]]
for items in groups:
    if items and len(items):
        print("nonempty")
    print(not (items and 3))
    print(bool(items and 3))
    print(bool(items or b"fallback"))
    print(bool((items and "text") or 0))
    assert items or "fallback"
    print(5 if items and 1 else 6)
numbers = [value for value in range(4) if value]
print(len(numbers))
print(numbers[0])
mapping = {value: value + 1 for value in range(4) if value}
print(len(mapping))
unique = {value for value in range(4) if value}
print(len(unique))
