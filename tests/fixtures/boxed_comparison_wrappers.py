import types


def current():
    obj = object()
    for slot, value in ((object.__init__, obj), (object.__str__, obj),
                        (object.__lt__, obj), (int.__lt__, 42)):
        assert type(slot) is types.WrapperDescriptorType
        bound = slot.__get__(value)
        assert type(bound) is types.MethodWrapperType
        assert bound.__self__ is value
        assert getattr(value, slot.__name__) == bound

    class Owner:
        def method(self):
            pass

    def capture():
        return obj

    code_type = types.CodeType
    code_object = capture.__code__
    assert code_object.co_freevars == ("obj",)
    assert code_type.co_freevars.__get__(code_object, code_type) is code_object.co_freevars
    assert code_type.__base__ is object and code_type.__mro__ == (code_type, object)
    assert code_type.co_name.__get__(code_object, code_type) is code_object.co_name
    assert "co_flags" in dir(code_object) and dir(code_object) == dir(code_type)

    def stream():
        yield obj

    names = stream.__code__.co_freevars
    marked = types.coroutine(stream).__code__
    assert marked.co_freevars == names == ("obj",)
    assert marked.co_freevars is not names

    method = Owner().method
    for value in (method.__func__.__code__, method.__func__, method, 42, "abc"):
        assert type(value).__init__ is object.__init__
        assert type(value).__init__(value) is None
        bound = value.__init__
        assert type(bound) is types.MethodWrapperType
        assert bound.__self__ is value
        assert bound() is None
        assert bound(1, flag=True) is None
    method.__func__.__init__ = None
    assert method.__init__.__self__ is method
    del method.__func__.__init__
    for cls in (list, dict, type, types.ModuleType, Exception):
        assert cls.__init__ is not object.__init__

    class code:
        pass

    class Number(int):
        pass

    assert Number.__init__ is object.__init__
    assert object.__init__(Number(2), 3) is None
    try:
        object.__init__(code(), 3)
    except TypeError:
        pass
    else:
        raise AssertionError("a Python class name must not select a native allocator")

    assert obj.__init__() is None
    assert obj.__str__() == str(obj)
    assert object.__lt__(obj, obj) is NotImplemented
    assert obj.__lt__(1) is NotImplemented

    class WithNew:
        def __new__(cls, arg):
            return object.__new__(cls)

    assert WithNew.__init__ is object.__init__
    c = WithNew(1)
    assert c.__init__(2, allowed=True) is None

    class WithInit(WithNew):
        def __init__(self, arg):
            pass

    try:
        object.__init__(WithInit(1), 1)
    except TypeError:
        pass
    else:
        raise AssertionError('object initializer accepted subclass initializer arguments')

    class Plain:
        pass

    try:
        Plain(1)
    except TypeError:
        pass
    else:
        raise AssertionError('plain constructor accepted an argument')

    class Number(int):
        def __int__(self):
            raise AssertionError

        def __lt__(self, other):
            return 'override'

    class Unrelated:
        __class__ = int

        def __gt__(self, other):
            return 'reflected'

    large = 1 << 140
    assert int.__lt__(Number(large), large + 1) is True
    assert int.__lt__(large, Number(large)) is False
    assert int.__lt__(True, 2) is True
    assert int.__lt__(2, 3.0) is NotImplemented
    assert int.__lt__(2, Unrelated()) is NotImplemented
    assert (Number(1) < 0) == 'override'
    assert (obj < Unrelated()) == 'reflected'
    for bad in (None, 1.0, Unrelated()):
        try:
            int.__lt__(bad, 1)
        except TypeError:
            pass
        else:
            raise AssertionError('int descriptor accepted an incompatible receiver')


current()


class Verdict:
    def __bool__(self):
        return False

    def __del__(self):
        events.append("released")


class Item:
    def __eq__(self, other):
        events.append(other)
        return Verdict()


events = []
item = Item()
if item == 1 or item == 2:
    raise AssertionError("unexpected truth result")
assert events == [1, "released", 2, "released"], events
