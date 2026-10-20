"""Each pulled element survives later alias mutation in the same zip row."""


class Row:
    number: int
    text: str
    values: list[int]

    def __init__(self, index: int) -> None:
        number = (1 << 100) + index
        self.number = number
        self.text = "row " + str(index)
        self.values = [number]


def mutate(rows: list[Row], mode: int) -> int:
    if mode == 0:
        rows.pop()
    elif mode == 1:
        rows[0] = Row(99)
    else:
        for index in range(128):
            rows.append(Row(index + 100))
    return mode


def selected_before_mutation(mode: int) -> None:
    rows = [Row(mode)]
    for row, marker in zip(
        rows, (mutate(rows, mode) for index in range(1)), strict=True
    ):
        print(marker)
        print(row.number)
        print(row.text)
        print(row.values[0])
        # Growing the first source must not trigger a strict probe after break.
        break
    print(len(rows))


def erase_row(rows: list[Row | None], index: int) -> int:
    rows[index] = None
    return index


def erase_float(values: list[float], index: int) -> int:
    values[index] = 0.0
    return index


def erase_optional_float(values: list[float | None], index: int) -> int:
    values[index] = None
    return index


def nullable_values() -> None:
    rows: list[Row | None] = [None, Row(7)]
    for row, index in zip(
        rows, (erase_row(rows, index) for index in range(2)), strict=True
    ):
        if row is None:
            print("absent row")
        else:
            print(row.text)
            print(row.number)
        print(rows[index] is None)

    numbers = [0.0, -0.0, float("nan"), float("inf"), -float("inf")]
    for number, index in zip(
        numbers, (erase_float(numbers, index) for index in range(5)), strict=True
    ):
        print(number)
        print(numbers[index])

    optional: list[float | None] = [None, -0.0, 1.5, float("nan")]
    for optional_number, index in zip(
        optional,
        (erase_optional_float(optional, index) for index in range(4)),
        strict=True,
    ):
        if optional_number is None:
            print("absent float")
        else:
            print(optional_number)
        print(optional[index] is None)

    nothing: list[None] = [None, None]
    for value in reversed(nothing):
        print(value is None)


def change_and_reject(values: list[str], grow: bool) -> bool:
    if grow:
        values.append("new")
    else:
        values.pop()
    return False


def strict_probe(reverse: bool, grow: bool) -> None:
    empty: list[int] = []
    values: list[str] = [] if grow else ["old"]
    print(f"probe {reverse} {grow}")
    try:
        if reverse:
            rows = list(
                zip(
                    empty,
                    (0 for index in range(1) if change_and_reject(values, grow)),
                    reversed(values),
                    strict=True,
                )
            )
        else:
            rows = list(
                zip(
                    empty,
                    (0 for index in range(1) if change_and_reject(values, grow)),
                    values,
                    strict=True,
                )
            )
        print(len(rows))
    except ValueError as error:
        print(error)
    print(len(values))


for mode in range(3):
    selected_before_mutation(mode)
nullable_values()
for reverse in [False, True]:
    for grow in [False, True]:
        strict_probe(reverse, grow)

# Conversion and later pulls remain lazy on the generic string-list route.
print(any(map(int, ["1", "bad"])))
print(all(map(int, ["0", "bad"])))
