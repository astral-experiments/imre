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

    class ReplaceNext:
        def __set_name__(self, owner, name):
            owner.later = None
            for i in range(2500):
                _ = [i, {}]

    class Names:
        first = ReplaceNext()
        later = Forward()

    print(Names.later is None, order[-1])

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
