class Item:
    names: list[str]

    def __init__(self, names: list[str]) -> None:
        self.names = names


def empty_names() -> list[str]:
    return []


def size(names: list[str]) -> int:
    return len(names)


def empty_nested() -> list[list[str]]:
    return [[], []]


def empty_pair() -> tuple[list[str], list[int]]:
    return ([], [])


def main() -> None:
    names: list[str] = []
    print(size([]))
    names = empty_names()
    names.append("typed return")
    print(names[0])
    nested: list[list[str]] = [[], ["context"]]
    nested[0].append("typed nested")
    print(nested[0][0])
    print(nested[-1][0])
    nested = empty_nested()
    nested[0].append("independent")
    print(len(nested[1]))
    pair = empty_pair()
    pair[0].append("tuple context")
    pair[-1].append(23)
    print(pair[0][0])
    print(pair[1][0])
    item = Item([])
    item.names.append("constructor context")
    print(item.names[0])
    objects: list[Item] = []
    objects.append(Item([]))
    print(len(objects[0].names))


main()
