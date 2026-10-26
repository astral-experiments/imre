# ruff: noqa: C400
# Materialization must consume the generator's distinct lazy scope.


def show(values: list[int]) -> None:
    for value in values:
        print(value)


class ShadowedReceiver:
    value: int

    def __init__(self) -> None:
        self.value = sum(self for self in [1, 2, 3])


def narrowed(value: int | None) -> None:
    if value is not None:
        print(sum(value for value in [1, 2]))
        print(value + 1)


def scopes() -> None:
    target = "outer"
    show(list(target + 1 for target in [1, 2, 3]))
    print(target)
    source = [5, 6]
    show(list(source + 10 for source in source))
    show(source)
    repeated = 90
    show(list(repeated for repeated in [2, 3] for repeated in range(repeated)))
    print(repeated)
    show(list(same for same, same in [(1, 10), (2, 20)]))

    # A captured name is loaded when the lazy body runs, after the start argument.
    offset = 1
    print(sum((value + offset for value in [2, 3]), (offset := 10)))
    print(offset)
    values = [1, 2]
    print(sum((value + values[0] for value in values), len(values := [10])))
    print(values[0])
    offset = 10
    for offset in (offset + value for value in [1, 2, 3]):
        print(offset)
    offset = 1
    for left, right in zip(
        (offset + value for value in [0, 0]),
        ((offset := value) for value in [10, 20]),
        strict=True,
    ):
        print(left)
        print(right)
    print(offset)

    saved = -1
    print(any((saved := value) > 1 for value in range(5)))
    print(saved)
    print(all((saved := value) > 0 for value in [3, 0, 9]))
    print(saved)
    print(sum((saved := value) for value in [7, 8]))
    print(saved)
    print(any((saved := value) for value in range(0)))
    print(saved)
    print(sum((saved := inner) for outer in [1, 2] for inner in range(outer)))
    print(saved)

    nullable: int | None = 1
    try:
        for result in (nullable + item for outer in [0] if nullable is not None for item in range(2)):
            print(result)
            nullable = None
    except TypeError:
        print("TypeError")

    # Nested generators capture the active outer target, not an enclosing name.
    inner = 77
    show(
        list(
            inner + outer
            for outer in [2, 3]
            for inner in (item for item in range(outer))
        )
    )
    print(inner)
    show(list(sum(inner for inner in range(outer)) for outer in [1, 3, 5]))
    show(
        list(
            outer * inner
            for outer in [1, 2, 3]
            if outer > 1
            for inner in range(outer)
            if inner
        )
    )


scopes()
narrowed(None)
narrowed(9)
print(ShadowedReceiver().value)
