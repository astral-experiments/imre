def rows(count: int) -> None:
    table: list[list[tuple[int, int]]] = []
    table.append([])
    last = -15
    for last in range(count):
        table.append([])
    else:
        print(f"{last} {len(table)}")
    for i in range(9):
        table[-1].append((2**75 if i == 3 else i, -i))
    for left, right in table[-1]:
        print(f"{left} {right}")


for stop in [-5, 0, 1, 11]:
    rows(stop)
