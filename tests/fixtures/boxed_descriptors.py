"""Compare data descriptors, callable instances, and attribute fallback with CPython."""

from functools import update_wrapper


def current():
    class Foreign:
        __repr__ = list.__repr__
    seen = []
    try:
        Foreign().__repr__(seen.append(1))
    except TypeError:
        pass
    assert seen == [1]
    try:
        saved = Foreign().__repr__
        saved(seen.append(2))
    except TypeError:
        pass
    assert seen == [1]
    try:
        Foreign().__repr__(*(seen.append(3),))
    except TypeError:
        pass
    assert seen == [1]

    class Document:
        __slots__ = ("__doc__",)

    doc = Document()
    assert not hasattr(doc, "__doc__")
    doc.__doc__ = "instance documentation"
    assert doc.__doc__ == "instance documentation"
    del doc.__doc__
    assert not hasattr(doc, "__doc__")

    class Namespace:
        pass

    obj = Namespace()
    obj.flag = "old"
    previous = obj.__dict__
    replacement = {"flag": "new"}
    obj.__dict__ = replacement
    assert obj.__dict__ is replacement and obj.flag == "new"
    obj.flag = "updated"
    assert replacement["flag"] == "updated" and previous["flag"] == "old"
    del obj.__dict__
    assert obj.__dict__ == {} and "flag" in replacement

    class Slotted:
        __slots__ = ("field",)

    class Mask(Slotted):
        field = "class"

    masked = Mask()
    masked.field = "instance"
    assert masked.__dict__["field"] == masked.field

    order = []

    class Forward:
        def __set_name__(self, owner, name):
            self.name = name
            order.append((owner.__name__, name))

        def __get__(self, instance, owner):
            if instance is None:
                return self
            return getattr(instance.target, self.name)

    class Store:
        def __get__(self, instance, owner):
            return instance.saved

        def __set__(self, instance, value):
            instance.saved = value * 2

    class Raising:
        def __get__(self, instance, owner):
            raise AttributeError("from descriptor")

    class Target:
        def __str__(self):
            return "target"

        def read(self, x):
            return x + 1

        def __call__(self, x, *, extra):
            return x + extra

    class Proxy:
        read = Forward()
        __call__ = Forward()
        __str__ = Forward()
        write = Store()
        absent = Raising()

        def __init__(self, target):
            self.target = target

        def __getattr__(self, name):
            return "missing " + name

    proxy = Proxy(Target())
    proxy.write = 6
    print(order, Proxy.read.name, proxy.write)
    print(callable(proxy), callable(Proxy), callable(proxy.read), callable(7))
    bound = Target.read.__get__(proxy.target, Target)
    print(bound(3), Target.read.__get__(None, Target) is Target.read)
    print(proxy.read(21), proxy(4, extra=3), proxy.absent, proxy.other)
    proxy.read = 9
    proxy.__call__ = lambda x: x
    proxy.__str__ = lambda: "instance"
    print(str(proxy), proxy.__str__())
    print(proxy.read, proxy.__call__(7), proxy(7, extra=8))

    class BadString:
        def __str__(self):
            return 42

    try:
        str(BadString())
    except TypeError as error:
        print(str(error))

    class LateDescriptor:
        pass

    late = LateDescriptor()
    late.write = "instance"
    LateDescriptor.write = Store()
    late.saved = 42
    print(late.write)

    failure = ValueError("descriptor failed")

    class Broken:
        def __set_name__(self, owner, name):
            raise failure

    try:
        class Bad:
            attr = Broken()
    except ValueError as error:
        assert error is failure
        assert error.__notes__ == ["Error calling __set_name__ on 'Broken' instance 'attr' in 'Bad'"]
    else:
        raise AssertionError("descriptor exception was lost")

    class ReplaceNext:
        def __set_name__(self, owner, name):
            owner.later = None
            for i in range(2500):
                _ = [i, {}]

    class Names:
        first = ReplaceNext()
        later = Forward()

    print(Names.later is None, order[-1])

    class Qualname(str):
        def __set_name__(self, owner, name):
            raise AssertionError("type metadata received a descriptor callback")

    class Ordered:
        __qualname__ = Qualname("Ordered")
        first = Forward()
        second = Forward()
        third = Forward()
        del first
        second = Forward()
        first = Forward()

    assert order[-3:] == [("Ordered", "second"), ("Ordered", "third"), ("Ordered", "first")]

    class Missing(AttributeError, Exception):
        pass

    class Hidden:
        @property
        def field(self):
            raise Missing("absent")

    class Recover(Hidden):
        def __getattr__(self, name):
            return "fallback " + name

    hidden = Hidden()
    print(getattr(hidden, "field", 17), hasattr(hidden, "field"), Recover().field)

    def wrapper():
        pass

    wrapper.field = {"kept": 1}
    update_wrapper(wrapper, hidden, ("field",), ("field",))
    print(wrapper.field, wrapper.__wrapped__ is hidden)

    class Loop:
        pass

    callback = Loop()
    Loop.__call__ = callback

    class Recursive:
        __setitem__ = callback

    try:
        Recursive()[1] = 2
    except RecursionError:
        print("recursive setter")
    Loop.__call__ = lambda self, key, value: print("retry", key, value)
    Recursive()[3] = 4

    class Next:
        def __get__(self, receiver, owner):
            return receiver.target

    class Link:
        __call__ = Next()

    target = lambda: 73
    for _ in range(1200):
        link = Link()
        link.target = target
        target = link
    print(target())

    class Slotted:
        __slots__ = ('field',)

    item = Slotted()
    item.field = 7
    print(item.field)
    try:
        item.extra = 11
    except AttributeError as error:
        print(str(error))

    class Node:
        def __eq__(self, other):
            return isinstance(other, Node)

        __hash__ = object.__hash__

    first, second = Node(), Node()
    slot = Node.__hash__
    assert slot is object.__hash__ and type(slot).__name__ == "wrapper_descriptor"
    assert slot.__get__(None, Node) is slot
    bound = slot.__get__(first, Node)
    assert bound.__self__ is first and bound.__objclass__ is object
    assert first == second and hash(first) == slot(first) == bound()
    keys = {first: 1, second: 2}
    assert keys[first] == 1 and keys[second] == 2

    class Key(str):
        __eq__ = str.__eq__
        __hash__ = str.__hash__

    assert str.__dict__["__hash__"] is Key.__hash__ is str.__hash__
    assert str.__hash__.__objclass__ is str
    for text in ("", "Málaga", "\ud800"):
        key = Key(text)
        assert key.__hash__() == hash(key) == hash(text)
        assert {key: 1}[text] == {text: 1}[key]

    class CustomHash(Key):
        def __hash__(self):
            return 42

    key = CustomHash("zone")
    assert hash(key) == 42 and str.__hash__(key) == hash("zone")
    try:
        str.__hash__(42)
    except TypeError as error:
        assert str(error) == "descriptor '__hash__' requires a 'str' object but received a 'int'"
    else:
        raise AssertionError("str.__hash__ accepted an integer")

    class Invoice(int):
        def __hash__(self):
            return 303

    invoice = Invoice(1 << 130)
    assert hash(invoice) == invoice.__hash__() == 303
    assert int.__hash__(invoice) == hash(1 << 130)
    assert int.__dict__["__hash__"].__get__(invoice, Invoice)() == hash(1 << 130)
    try:
        int.__hash__(1.0)
    except TypeError as error:
        assert str(error) == "descriptor '__hash__' requires a 'int' object but received a 'float'"
    else:
        raise AssertionError("int hash accepted a float")

    class Number(int):
        def __radd__(self, other):
            return "reflected"

    add = int.__add__
    assert type(add) is type(object.__hash__) and add.__objclass__ is int
    assert getattr(add.__objclass__, add.__name__) is add
    assert Number.__add__ is bool.__add__ is add
    number = Number(1 << 100)
    assert add(True, number) == (1 << 100) + 1 and 1 + number == "reflected"
    bound = add.__get__(number, Number)
    assert repr(bound) == f"<method-wrapper '__add__' of Number object at {id(number):#x}>"
    assert bound(number) == 1 << 101
    assert add(number, 0.5) is NotImplemented

    class IntegerItem(int):
        __eq__ = int.__eq__
        __le__ = int.__le__

    class Reflected(IntegerItem):
        def __eq__(self, other):
            return "reflected"

    class ReverseOrder(int):
        __lt__ = int.__gt__

    assert ReverseOrder(2) < 1 and not ReverseOrder(1) < 2

    item = IntegerItem(1 << 100)
    peer = Reflected(1 << 100)
    assert (item == peer) == "reflected"
    assert (True == Reflected(1)) is True and (1.0 == Reflected(1)) is True
    assert int.__eq__(item, peer) is True
    assert int.__dict__["__eq__"] is bool.__eq__ is IntegerItem.__eq__
    assert int.__eq__.__get__(item, IntegerItem)(1.0) is NotImplemented
    assert int.__le__(True, item) is True
    assert int.__le__(item, "large") is NotImplemented

    class Declines(int):
        def __eq__(self, other):
            return NotImplemented

    left = Declines(1)
    assert (left == left) is True and (left == Declines(1)) is False

    class FloatItem(float):
        __eq__ = float.__eq__
        __le__ = float.__le__
        __lt__ = float.__lt__

    calls = []

    class ReflectedFloat(FloatItem):
        def __gt__(self, other):
            calls.append(other)
            return "reflected float"

    real = FloatItem(2**53)
    peer = ReflectedFloat(real)
    assert (real < peer) == "reflected float" and calls == [real]
    assert float.__lt__(real, peer) is False and len(calls) == 1
    assert float.__lt__(real, 2**53 + 1) is True
    assert float.__eq__(real, 2**53 + 1) is False
    assert float.__lt__(float("inf"), 1 << 4000) is False
    assert float.__eq__(-0.0, 0) is True
    assert float.__eq__(real, 2 + 0j) is NotImplemented

    boundary_calls = []

    class EdgeInteger(int):
        def __eq__(self, other):
            boundary_calls.append(("eq", type(other), other))
            return "matched"

        def __gt__(self, other):
            boundary_calls.append(("gt", type(other), other))
            return []

    edge = EdgeInteger(2**53 + 1)
    assert float.__eq__(real, edge) is True
    assert (real == edge) is True
    assert float.__le__(2**48 + 0.5, EdgeInteger(2**48)) is False
    assert float.__eq__(2**48 + 0.5, EdgeInteger(2**48)) is False
    assert boundary_calls == [("eq", int, 2**53), ("eq", int, 2**53), ("gt", int, 2**48)]

    class ForeignComparison:
        __lt__ = float.__eq__

    foreign = ForeignComparison()
    try:
        foreign < 0.0
    except TypeError as exc:
        assert str(exc) == "descriptor '__eq__' requires a 'float' object but received a 'ForeignComparison'"
    else:
        raise AssertionError("float comparison accepted a foreign receiver")

    class FloatEquality(float):
        def __eq__(self, other):
            return "float equality"

    assert (1 + 0j == FloatEquality(1)) is True
    assert (FloatEquality(1) == 1 + 0j) == "float equality"
    nan = FloatItem("nan")
    assert float.__eq__(nan, nan) is False and nan != nan
    assert float.__le__.__get__(nan, FloatItem)(nan) is False

    index = int.__index__
    assert type(index) is type(add) and index.__objclass__ is int
    assert index is Number.__index__ is bool.__index__
    exact = 1 << 100
    assert exact.__index__() is exact
    assert number.__index__() == exact and type(number.__index__()) is int
    assert True.__index__() == 1 and type(True.__index__()) is int

    class CustomIndex(int):
        def __index__(self):
            return 73

    custom = CustomIndex(4)
    assert custom.__index__() == 73 and index(custom) == 4
    try:
        index("4")
    except TypeError as error:
        assert str(error) == "descriptor '__index__' requires a 'int' object but received a 'str'"
    else:
        raise AssertionError("int index accepted a string")

    class Library:
        field = 17

    namespace = Library.__dict__
    assert namespace["field"] == 17 and namespace is not Library.__dict__
    Library.field = 19
    assert namespace["field"] == 19
    del Library.field
    assert "field" not in namespace
    try:
        Library.__dict__ = {}
    except AttributeError:
        pass
    else:
        raise AssertionError("class __dict__ is writable")
