from dataclasses import KW_ONLY, FrozenInstanceError, asdict, dataclass, field, fields


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
