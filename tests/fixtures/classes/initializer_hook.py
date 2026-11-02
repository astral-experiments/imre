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
    def __init__(self):
        self.reference = self._reference().copy()
        entry_key, entry_value = self.reference.popitem()
        self.last = {entry_key: entry_value}

    def _reference(self):
        return {"kind": "base", "pair": (1, 2)}


class DerivedMapping(Mapping):
    def _reference(self):
        return {"kind": "derived"}


class Pairs:
    def __init__(self):
        self.pairs = self.make().copy()

    def make(self):
        return {"a": (1, 2), "b": (3, 4)}


show(Owner("first"))
derived = Derived()
show(derived)
print(derived.extra)
show(Inherited("third"))
try:
    Reject("unused")
except ValueError as error:
    print(str(error))


print(Mapping().last["pair"])
derived_mapping = DerivedMapping()
print(derived_mapping.last["kind"])
assert derived_mapping._reference()["kind"].upper() == "DERIVED"
assert Pairs().make()["b"][0] == 3
assert Pairs().pairs["a"][1] == 2
