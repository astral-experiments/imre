def first(left: str, right: str) -> str:
    return left


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
    replacement = rows.pop()
    print(replacement[0])
    print(len(rows))


main()
