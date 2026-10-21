"""Private iterator resets preserve arbitrary-precision owners and cursors."""


def edges(start: int, stop: int, step: int) -> None:
    for value in (item for item in range(start, stop, step)):
        print(value)
    print("reverse")
    for index, value in zip(range(4), reversed(range(start, stop, step))):
        print(index)
        print(value)
    print("sum")
    print(sum(item for item in range(start, stop, step)))


boundary = 1 << 62
for start, stop, step in [
    (boundary - 2, boundary + 2, 1),
    (-boundary + 1, -boundary - 3, -1),
    (boundary - 2, boundary - 1, 2),
    (-boundary + 1, -boundary, -2),
    (-boundary, -boundary + 1, 1),
    (boundary, boundary, -1),
]:
    edges(start, stop, step)

# Constant arguments permit analysis; the final increment still needs BigInt.
for value in (item for item in range(4611686018427387902, 4611686018427387903, 2)):
    print(value)
for value in reversed(range(-4611686018427387904, -4611686018427387903)):
    print(value)

# The same inner pipeline alternates heap integers and immediate integers.
huge = 1 << 100
print(sum(value for start in [huge, 0, -huge] for value in range(start, start + 2)))
print(
    sum(
        value
        for start in [huge, 0, -huge]
        for value in reversed(range(start, start + 2))
    )
)

# Clearing a private yielded value does not clear the enclosing loop target.
last = huge
for last in (value for value in range(0)):
    print("unexpected empty range")
print(last)
for last in (value for value in range(huge, huge + 2)):
    pass
print(last)
