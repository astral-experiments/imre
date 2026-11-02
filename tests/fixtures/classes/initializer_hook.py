def value(label: str, size: int) -> list[int]:
    print(label)
    return [size]


class Owner:
    name: str
    items: list[int]

    def __init__(self, name: str) -> None:
        self.name = name
        self.initialize()

    def initialize(self) -> None:
        self.items = value("base", 1)


class Derived(Owner):
    extra: str

    def __init__(self) -> None:
        super().__init__("derived")
        self.extra = "after"

    def initialize(self) -> None:
        self.items = value("override", 2)


class Inherited(Owner):
    def initialize(self) -> None:
        self.items = value("inherited", 3)
        print(self.items)


def show(owner: Owner) -> None:
    print(owner.name)
    print(owner.items)


def reject() -> list[int]:
    raise ValueError("initializer failed")


class Reject(Owner):
    def initialize(self) -> None:
        self.items = reject()


class Mapping:
    reference: dict[str, str]

    def __init__(self):
        self.reference = self._reference().copy()

    def _reference(self):
        return {"kind": "base"}


class DerivedMapping(Mapping):
    def _reference(self):
        return {"kind": "derived"}


show(Owner("first"))
derived = Derived()
show(derived)
print(derived.extra)
show(Inherited("third"))
try:
    Reject("unused")
except ValueError as error:
    print(str(error))


assert Mapping().reference["kind"] == "base"
assert DerivedMapping().reference["kind"] == "derived"
