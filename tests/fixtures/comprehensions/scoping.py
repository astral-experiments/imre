"""Comprehension targets are private, including nested and repeated targets."""


def show(values: list[int]) -> None:
    for value in values:
        print(value)


def rows(limit: int) -> list[list[int]]:
    return [[inner + outer for inner in range(outer)] for outer in range(limit)]


offsets = [2]


def offset_values() -> list[int]:
    return [offsets[0] + value for value in range(2)]


def scoped() -> None:
    value = "enclosing string"
    numbers = [value + 10 for value in [1, 2, 3]]
    print(value)
    show(numbers)

    # The leftmost iterable sees the enclosing list before its name is shadowed.
    source = [4, 5, 6]
    copied = [source * 2 for source in source]
    show(copied)
    show(source)
    source = [source + 1 for source in source]
    show(source)

    later = "outer later"
    pairs = [(first, later) for first in range(3) for later in range(first)]
    for first, second in pairs:
        print(first)
        print(second)
    print(later)

    repeated = 80
    show([repeated for repeated in [2, 3] for repeated in range(repeated)])
    print(repeated)
    show([same for same, same in [(1, 10), (2, 20)]])

    target = 90
    empty: list[int] = [target for target in range(0)]
    print(len(empty))
    print(target)
    filtered = [target for target in range(3) if False]
    print(len(filtered))
    print(target)

    inner = "outer inner"
    nested = rows(4)
    for row in nested:
        print(len(row))
        show(row)
    print(inner)
    same_names = [[index for index in range(index)] for index in [1, 3]]
    for row in same_names:
        show(row)


def optional_addition(values: list[int | None]) -> None:
    try:
        print([value + 1 for value in values])
    except TypeError as exc:
        print(str(exc))
    try:
        print([value + 1 for value in values if value is not None for value in values])
    except TypeError as exc:
        print(str(exc))


# Entry locals must be captured by generated helpers without becoming globals.
entry = [7, 8]
entry_result = [entry + 1 for entry in entry]
show(entry_result)
show(entry)
show(offset_values())
scoped()
optional_addition([1, 2])
optional_addition([1, None])
optional_addition([None])
