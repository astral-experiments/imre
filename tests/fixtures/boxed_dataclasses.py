from dataclasses import KW_ONLY, FrozenInstanceError, InitVar, asdict, dataclass, field, fields


@dataclass
class Entry:
    name: str
    children: list = field(default_factory=list)
    _: KW_ONLY
    options: dict = field(default_factory=dict)

    def label(self):
        return self.name.upper()


@dataclass(frozen=True)
class Key:
    part: int
    value: str = field(compare=False, repr=False, default="unused")


def current():
    class Clearing(dict):
        def __eq__(self, other):
            self.left.clear()
            self.right.clear()
            for i in range(12000):
                item = [i]
            return NotImplemented

    clearing = Clearing(code=7)
    clearing.left = [field(metadata=clearing).metadata]
    clearing.right = [field(metadata={"code": 7}).metadata]
    assert clearing.left == clearing.right

    backing = {"code": 7}
    view = field(metadata=backing).metadata
    assert view == {"code": 7} and {"code": 7} == view
    backing["code"] = 9
    assert view != {"code": 7} and view["code"] == 9
    assert InitVar[int].type is int
    a = Entry("root")
    b = Entry(name="root")
    assert a == b and not (a != b) and a.label() == "ROOT"
    assert a.children is not b.children and a.options is not b.options
    a.children.append(1)
    assert a != b and asdict(a) == {"name": "root", "children": [1], "options": {}}
    assert Entry.__match_args__ == ("name", "children")
    assert fields(Entry)[1].type is list and fields(Entry)[2].default_factory is dict
    key = Key(42)
    assert repr(key) == "Key(part=42)"
    assert key == Key(42, "other")
    assert Key.__hash__ is not None and Entry.__hash__ is None
    for bad in (lambda: Entry(), lambda: Entry("x", name="y"), lambda: Entry("x", unknown=0)):
        try:
            bad()
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError("missing TypeError")
    try:
        key.part = 1
    except FrozenInstanceError as exc:
        print(str(exc))
    try:
        del key.part
    except FrozenInstanceError as exc:
        print(str(exc))
    print("boxed dataclasses")
