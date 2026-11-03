def fresh() -> list[list[tuple[int, int]]]:
    values: list[list[tuple[int, int]]] = []
    values.append([])
    values.append([])
    return values


def grow(values: list[list[tuple[int, int]]]) -> int:
    values.append([])
    return 17


def mark(log: list[int], value: int) -> int:
    log.append(value)
    return value


def fail(log: list[int]) -> int:
    log.append(43)
    raise ValueError("field")


def show(values: list[list[tuple[int, int]]], at: int) -> None:
    for left, right in values[at]:
        print(f"{left} {right}")


def main() -> None:
    values = fresh()
    log: list[int] = []
    values[-1].append((grow(values), 19))
    values[-1].append((10**40, -(10**50)))
    for boundary in [-1073741825, -1073741824, 1073741823, 1073741824]:
        values[-1].append((boundary, -boundary))
    for i in range(8):
        values[0].append((i, -i))
    try:
        values[mark(log, 8)].append((mark(log, 41), mark(log, 42)))
    except IndexError:
        print(str(log))
    try:
        values[1].append((mark(log, 42), fail(log)))
    except ValueError:
        print(str(log))
    for row in values:
        print(len(row))
    show(values, 0)
    show(values, 1)
    show(values, -1)
    saved = values[0]
    try:
        saved = values[10**50]
    except IndexError as error:
        print(str(error))
        print(len(saved))
    values = fresh()
    print(f"{len(saved)} {len(values)}")


main()
