class Item:
    name: str
    value: int

    def __init__(self, name: str, value: int) -> None:
        self.name = name
        self.value = value

    def label(self) -> str:
        return self.name


def first(left: Item, right: Item) -> Item:
    return left


def main() -> None:
    items: list[Item] = [Item("original", 7)]
    alias = items
    saved = alias[0]
    repeated = items * 2
    items[0] = Item("replacement", 9)
    print(saved.label())
    repeated[1].name = "shared object"
    print(repeated[0].label())
    print(saved.label())
    print(items[0].label())
    popped = repeated.pop()
    repeated[0] = Item("new", 11)
    print(popped.value)
    print(popped.label())

    one: list[Item] = [Item("argument lifetime", 13)]
    kept = first(one[0], one.pop())
    print(kept.label())
    print(len(one))
    one.append(Item("append", 17))
    popped_again = one.pop()
    print(popped_again.value)

    nested: list[list[Item]] = [[Item("nested", 19)]]
    nested_alias = nested[0]
    nested[0] = []
    print(nested_alias[0].label())
    print(nested_alias.pop().value)


main()
