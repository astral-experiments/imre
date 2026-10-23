def first(left: str, right: str) -> str:
    return left


def change_left(items: list[list[int]]) -> list[list[int]]:
    items.append([3])
    return items


def main() -> None:
    words: list[str] = ["alpha", "beta"]
    alias = words
    kept = words[-1]
    alias[-1] = "replacement"
    print(kept)
    print(words[1])
    alias.append("last")
    print(first(words[-1], words.pop()))
    print(len(words))
    print(words.pop())
    print(words.pop())

    flags: list[bool] = [True, False]
    flags_alias = flags
    flags_alias[-1] = True
    flags.append(False)
    print(flags[-2])
    print(flags.pop())
    print(len(flags * 0))
    print(len(flags * -2))
    repeated_flags = flags * 2
    repeated_flags[0] = False
    print(flags[0])
    print(repeated_flags[0])

    row: list[str] = ["shared"]
    rows: list[list[str]] = [row] * 3
    rows[1].append("added")
    print(rows[0][-1])
    saved_row = rows[-1]
    popped_row = rows.pop()
    rows[-1] = ["new"]
    print(saved_row[0])
    print(popped_row[-1])
    rows[0][0] = "changed"
    print(saved_row[0])
    print(rows[-1][0])

    # Both operands are evaluated before the left side is copied.
    items = [[1]]
    combined = items + change_left(items)
    print(len(combined))
    print(combined[1][0])
    combined[0].append(2)
    print(combined[2][1])
    print(items[0][1])
    combined.pop()
    print(len(items))
    words = ["first"]
    renamed = words + (words := ["replacement"])
    print(renamed[0])
    print(renamed[-1])
    print(([] + words + [])[0])
    contextual: list[str] = [] + []
    print(len(contextual))
    replacement = rows.pop()
    print(replacement[0])
    print(len(rows))


main()
