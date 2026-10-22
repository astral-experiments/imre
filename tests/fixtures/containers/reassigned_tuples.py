def fail() -> str:
    raise ValueError("later field")


def main() -> None:
    keys: dict[tuple[str, str], int] = {}
    saved: list[tuple[str, str]] = []
    pair = ("before", "loop")
    for index in range(4):
        pair = (f"left-{index}", f"right-{index}")
        if index == 0:
            keys[pair] = index
        elif index == 1:
            saved.append(pair)
        elif index == 2:
            try:
                pair = (pair[1], fail())
            except ValueError:
                print(repr(pair))
        else:
            pair = (pair[1], pair[0])
    print(keys[("left-0", "right-0")])
    print(repr(saved[0]))
    print(repr(pair))

    replacement = ("new", "owner")
    for index in range(2):
        pair = (pair[1], (pair := replacement)[0])
        print(repr(pair))
    print(repr(replacement))


main()
