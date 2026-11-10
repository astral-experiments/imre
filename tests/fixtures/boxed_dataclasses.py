from dataclasses import KW_ONLY, FrozenInstanceError, InitVar, asdict, astuple, dataclass, field, fields


@dataclass
class Entry:
    name: str
    children: list = field(default_factory=list)
    _: KW_ONLY
    options: dict = field(default_factory=dict)

    def label(self):
        return self.name.upper()


@dataclass(frozen=True, order=True)
class Key:
    part: int
    value: str = field(compare=False, repr=False, default="unused")


def new_values() -> list[int]:
    return [1]


@dataclass
class CustomFactory:
    values: list[int] = field(default_factory=new_values)


@dataclass
class CalledFactory:
    values: list[int] = field(default_factory=list())  # noqa: C408


def default_factories():
    first = CustomFactory().values
    second = CustomFactory().values
    assert first == second == [1] and first is not second

    supplied = [2]
    assert CalledFactory(supplied).values is supplied
    try:
        CalledFactory()
    except TypeError as exc:
        print(str(exc))
    else:
        raise AssertionError("missing TypeError for a non-callable default factory")


def replacement():
    from dataclasses import replace

    @dataclass(frozen=True)
    class Model:
        name: str
        items: list = field(default_factory=list)
        cache: list = field(default_factory=list, init=False)

    model = Model("old")
    model.cache.append(1)
    changed = replace(model, name="new")
    assert model.name == "old" and changed.name == "new"
    assert changed.items is model.items
    assert changed.cache == [] and model.cache == [1]
    assert model.__replace__(name="new") == changed
    try:
        replace(model, cache=[])
    except TypeError as exc:
        assert str(exc) == "field cache is declared with init=False, it cannot be specified with replace()"
    else:
        raise AssertionError("replaced an init=False field")

    @dataclass
    class Custom:
        name: str

        def __replace__(self, **changes):
            return "custom"

    assert Custom("old").__replace__() == "custom"
    assert replace(Custom("old"), name="new").name == "new"

    @dataclass
    class Product:
        quantity: int
        factor: InitVar[int]

        def __post_init__(self, factor):
            self.quantity *= factor

    source = Product(2, 3)
    assert replace(source, factor=2).quantity == 12
    try:
        replace(source)
    except TypeError as exc:
        assert str(exc) == "InitVar 'factor' must be specified with replace()"
    else:
        raise AssertionError("accepted a missing InitVar")


def mapping_conversion():
    from collections import defaultdict

    @dataclass
    class Row:
        value: int

    @dataclass
    class Table:
        groups: dict

    class Mapping(dict):
        def __deepcopy__(self, memo):
            return "copied mapping"

    groups = defaultdict(list)
    groups['a'].append(Row(3))
    table = Table(Mapping([('group', groups)]))
    tuple_groups = astuple(table)[0]
    dict_groups = asdict(table)['groups']
    assert type(tuple_groups) is type(dict_groups) is Mapping
    tuple_group, dict_group = tuple_groups['group'], dict_groups['group']
    assert type(tuple_group) is type(dict_group) is defaultdict
    assert tuple_group.default_factory is dict_group.default_factory is list
    assert tuple_group == {'a': [(3,)]}
    assert dict_group == {'a': [{'value': 3}]}
    assert tuple_group['a'] is not groups['a'] and dict_group['a'] is not groups['a']

    import collections
    import weakref

    mapping = Mapping()
    assert asdict(Table(weakref.proxy(mapping))) == {"groups": "copied mapping"}
    try:
        collections.defaultdict = tuple
        assert asdict(Table(groups))["groups"].default_factory is list
    finally:
        collections.defaultdict = defaultdict


def signature_docstrings():
    from inspect import Signature

    class Unrenderable(Signature):
        def __str__(self):
            raise ValueError("cannot render")

    class Text(str):
        def replace(self, old, new):
            raise ValueError("cannot strip")

    class Unstrippable(Signature):
        def __str__(self):
            return Text("(value: int) -> None")

    for signature in (Unrenderable(), Unstrippable()):
        @dataclass
        class Request:
            __signature__ = signature
            value: int

        assert Request.__doc__ == "Request"
        assert Request(4).value == 4


def abstract_refresh():
    import abc
    import inspect

    class Ordered(abc.ABC):
        @abc.abstractmethod
        def __lt__(self, other): pass

    class Required(abc.ABC):
        @abc.abstractmethod
        def action(self): pass

    @dataclass(order=True)
    class Item(Ordered):
        number: int

    @dataclass(order=True)
    class Pending(Ordered, Required):
        number: int

    assert not inspect.isabstract(Item) and Item(1) < Item(2)
    assert inspect.isabstract(Pending) and Pending.__abstractmethods__ == {'action'}
    Pending.action = lambda self: self.number
    assert abc.update_abstractmethods(Pending) is Pending and Pending(3).action() == 3

    class Plain: pass
    assert abc.update_abstractmethods(Plain) is Plain
    assert not hasattr(Plain, '__abstractmethods__')

    class Broken:
        def __get__(self, instance, owner):
            raise LookupError('binding failed')

    before = Item.__abstractmethods__
    Item.__lt__ = Broken()
    try:
        abc.update_abstractmethods(Item)
    except LookupError as exc:
        assert exc.args == ('binding failed',)
    else:
        raise AssertionError('ignored descriptor exception')
    assert Item.__abstractmethods__ is before


def current():
    abstract_refresh()
    @dataclass
    class Base:
        count: int
        seed: InitVar[int]

    @dataclass(order=True)
    class Model(Base):
        bonus: InitVar[int] = field(kw_only=True, default=1)

        def __post_init__(self, seed, bonus):
            self.count += seed + bonus

    first = Model(2, 3, bonus=4)
    assert first.__dict__ == {"count": 9} and first == Model(4, 4)
    assert first < Model(5, 4) and asdict(first) == {"count": 9}
    assert repr(first).endswith("Model(count=9)") and Model.__match_args__ == ("count", "seed")
    try:
        @dataclass
        class Invalid:
            value: InitVar[int] = field(default_factory=int)
    except TypeError as exc:
        assert str(exc) == "field value cannot have a default factory"
    else:
        raise AssertionError("InitVar accepted a default factory")

    class Clearing(dict):
        def __eq__(self, other):
            self.left.clear()
            self.right.clear()
            for i in range(12000):
                item = [i]
            return NotImplemented

    class Metadata:
        def __init__(self):
            self.values = {}

        def __getitem__(self, key):
            return self.values[key]

        def __len__(self):
            return len(self.values)

        def __bool__(self):
            raise AssertionError("proxy truth must use the mapping length")

    metadata = Metadata()
    view = field(metadata=metadata).metadata
    assert not view
    metadata.values["code"] = 7
    assert view and len(view) == 1 and view["code"] == 7

    clearing = Clearing(code=7)
    clearing.left = [field(metadata=clearing).metadata]
    clearing.right = [field(metadata={"code": 7}).metadata]
    assert clearing.left == clearing.right

    backing = {"code": 7}
    view = field(metadata=backing).metadata
    from types import MappingProxyType
    assert type(view) is MappingProxyType
    assert MappingProxyType(mapping=backing) == view
    import re
    assert MappingProxyType(re.match('x', 'x'))[0] == 'x'
    assert view == {"code": 7} and {"code": 7} == view
    backing["code"] = 9
    assert view != {"code": 7} and view["code"] == 9
    nested = field(metadata=view).metadata
    assert list(nested) == ["code"] and list(nested.items()) == [("code", 9)]
    assert nested["code"] == 9 and len(nested) == 1
    assert str(nested) == "{'code': 9}"
    assert repr(nested) == "mappingproxy(mappingproxy({'code': 9}))"
    backing["view"] = view
    assert repr(view) == "mappingproxy({'code': 9, 'view': mappingproxy({...})})"
    assert InitVar[int].type is int
    assert repr(InitVar[int]) == "dataclasses.InitVar[int]"
    assert repr(InitVar[list[int]]) == "dataclasses.InitVar[list[int]]"
    assert repr(InitVar[int | str]) == "dataclasses.InitVar[int | str]"
    assert Entry.__init__.__globals__ is globals()
    assert repr(Entry.__init__.__defaults__[0]) == "<factory>"
    marker = Entry.__init__.__kwdefaults__["options"]
    Entry.__init__.__kwdefaults__["options"] = "custom"
    try:
        assert Entry("changed").options == "custom"
    finally:
        Entry.__init__.__kwdefaults__["options"] = marker
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
    assert key < Key(43) and key <= Key(42, "other")
    assert key > Key(41) and key >= Key(42, "other")
    assert key.__lt__(a) is NotImplemented
    assert Key.__hash__ is not None and Entry.__hash__ is None

    @dataclass(frozen=True)
    class CustomEquality:
        part: int

        def __eq__(self, other):
            return self is other

    assert hash(CustomEquality(42)) == hash((42,))
    assert CustomEquality.__hash__.__qualname__ == CustomEquality.__qualname__ + ".__hash__"

    @dataclass(frozen=True)
    class ExplicitUnhashable:
        __hash__ = None

    assert ExplicitUnhashable.__hash__ is None

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

    @dataclass(order=True)
    class Item:
        value: int

    left, right = Item(1), Item(2)

    def stop(self):
        raise StopIteration("reading value")

    Item.value = property(stop)
    try:
        _ = left < right
    except StopIteration as exc:
        assert str(exc) == "reading value"
    else:
        raise AssertionError("comparison swallowed the field lookup error")

    default_factories()
    replacement()
    mapping_conversion()
    signature_docstrings()
    print("boxed dataclasses")
