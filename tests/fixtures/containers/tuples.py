class Item:
    name: str

    def __init__(self, name: str) -> None:
        self.name = name

    def details(self) -> tuple[str, int, bool]:
        return (self.name, len(self.name), True)


def make_pair() -> tuple[list[str], Item]:
    return (["retained"], Item("object"))


def main() -> None:
    value: tuple[str, int, bool, None] = (
        "mixed",
        10000000000000000000000000000000000000000,
        False,
        None,
    )
    print(value[0])
    print(value[1])
    print(value[-2])
    print(value[-1])
    print(value[-4])

    pair = make_pair()
    names = pair[0]
    names.append("alias")
    print(pair[0][-1])
    info = pair[-1].details()
    pair[-1].name = "changed"
    print(info[0])
    print(info[1])
    print(info[-1])

    pairs: list[tuple[list[str], Item]] = [make_pair()]
    retained = pairs[0]
    pairs[0] = (["replacement"], Item("new object"))
    print(retained[0][0])
    print(retained[1].name)
    popped = pairs.pop()
    print(popped[0][0])
    print(popped[-1].name)
    print(len(pairs))


main()
