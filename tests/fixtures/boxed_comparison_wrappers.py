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

    assert obj.__init__() is None
    assert obj.__str__() == str(obj)
    assert object.__lt__(obj, obj) is NotImplemented
    assert obj.__lt__(1) is NotImplemented

    class WithNew:
        def __new__(cls, arg):
            return object.__new__(cls)

    assert WithNew.__init__ is object.__init__
    assert WithNew(1).__init__() is None

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
