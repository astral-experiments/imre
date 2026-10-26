"""Read derived fields only on branches that proved the object's layout."""
class Root:
    pass


class Left(Root):
    label: str

    def __init__(self, label: str) -> None:
        self.label = label


class Leaf(Left):
    pass


class Right(Root):
    pass


def observe(value: Root) -> None:
    if isinstance(value, Left):
        print(value.label)
        value.label = value.label + "!"
    if not isinstance(value, Left):
        print("other")
        return
    print(value.label)


def forget(value: Root, replacement: Root) -> None:
    if isinstance(value, Left):
        print(value.label)
        value = replacement
        if isinstance(value, Left):
            print(value.label)
        else:
            print("replaced")


observe(Root())
observe(Left("parent"))
leaf = Leaf("leaf")
observe(leaf)
observe(Right())
forget(leaf, Right())
forget(leaf, Left("different"))
