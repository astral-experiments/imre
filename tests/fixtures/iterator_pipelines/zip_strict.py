def probe(log: list[str], label: str, value: int) -> int:
    log.append(label + str(value))
    return value


def clear_strict(flags: list[int], value: int) -> int:
    if flags:
        flags.pop()
    return value


def triple(first: int, second: int, third: int, strict: bool) -> None:
    log: list[str] = []
    try:
        rows = list(
            zip(
                (probe(log, "a", value) for value in range(first)),
                (probe(log, "b", value) for value in range(second)),
                (probe(log, "c", value) for value in range(third)),
                strict=strict,
            )
        )
        print(len(rows))
    except ValueError as error:
        print(error)
    for event in log:
        print(event)


for first, second, third in [
    (0, 0, 0),
    (1, 1, 1),
    (2, 1, 2),
    (2, 2, 1),
    (1, 2, 1),
    (1, 1, 2),
    (0, 1, 0),
    (0, 0, 1),
]:
    triple(first, second, third, True)
    triple(first, second, third, False)

try:
    print(len(list(zip([1], [2, 3], strict=True))))
except ValueError as error:
    print(error)
try:
    print(len(list(zip([1, 2], [3], strict=True))))
except ValueError as error:
    print(error)
try:
    print(len(list(zip([1], [2], [3], [4, 5], strict=True))))
except ValueError as error:
    print(error)
try:
    print(len(list(zip([1, 2], [3, 4], [5, 6], [7], strict=True))))
except ValueError as error:
    print(error)

# strict is a truth test, not an exact-bool annotation or an eager length check.
print(len(list(zip([1], [2, 3], strict=0))))
print(len(list(zip([1], [2, 3], strict=""))))
print(len(list(zip([1], [2, 3], strict=None))))
flags: list[int] = []
print(len(list(zip([1], [2, 3], strict=flags))))
flags.append(1)
try:
    print(len(list(zip([1], [2, 3], strict=flags))))
except ValueError as error:
    print(error)

# strict's truth value is captured before the first lazy source can mutate it.
flags = [1]
try:
    print(
        len(
            list(
                zip(
                    (clear_strict(flags, value) for value in [1]),
                    [2, 3],
                    strict=flags,
                )
            )
        )
    )
except ValueError as error:
    print(error)
print(len(flags))

# A failing strict probe propagates its original exception, not a length error.
try:
    print(len(list(zip([], map(int, "x"), strict=True))))
except ValueError as error:
    print(error)
try:
    print(len(list(zip([1], map(int, "x"), strict=True))))
except ValueError as error:
    print(error)

# Once a shorter middle source is found, the later source must not be probed.
log: list[str] = []
try:
    print(
        len(
            list(
                zip(
                    (probe(log, "a", value) for value in [1]),
                    [],
                    map(int, "x"),
                    strict=True,
                )
            )
        )
    )
except ValueError as error:
    print(error)
for event in log:
    print(event)
