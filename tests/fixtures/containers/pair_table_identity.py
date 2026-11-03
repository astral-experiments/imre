def main() -> None:
    table: list[list[tuple[int, int]]] = []
    table.append([])
    table.append([])
    table[0].append((10**30, 2))
    value = table[0][0]
    alias = table[0]
    table[1].append(value)
    print(alias is table[0])
    print(value == table[0][0])
    print(value == table[1][0])
    alias.append((3, 4))
    print(len(table[0]))
    print(alias.pop() == (3, 4))
    table[0] = []
    print(len(alias))
    print(len(table[0]))
    print(value == alias[0])


main()
