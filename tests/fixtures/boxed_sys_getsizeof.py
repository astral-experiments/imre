import gc
import sys
import weakref
from sys import getsizeof


def current():
    class Sized:
        def __sizeof__(self):
            return self.result

    class IndexOnly:
        def __index__(self):
            raise AssertionError("getsizeof called __index__")

    class SubInt(int):
        pass

    value = Sized()
    value.__sizeof__ = lambda: 500
    value.result = 0
    overhead = getsizeof(value)
    for result in (True, SubInt(39), sys.maxsize):
        value.result = result
        assert getsizeof(object=value) == int(result) + overhead

    fallback = object()
    value.result = IndexOnly()
    assert getsizeof(value, default=fallback) is fallback
    for result, kind in ((IndexOnly(), TypeError), (-1, ValueError), (1 << 100, OverflowError)):
        value.result = result
        try:
            if kind is TypeError:
                getsizeof(value)
            else:
                getsizeof(value, fallback)
        except kind:
            pass
        else:
            raise AssertionError("accepted invalid size")

    class Fault(TypeError):
        pass

    marker = Fault("descriptor failed")

    class Descriptor:
        def __get__(self, instance, owner):
            assert instance is measured and owner is Measured
            raise marker

    class Measured:
        __sizeof__ = Descriptor()

    measured = Measured()
    assert getsizeof(measured, fallback) is fallback
    try:
        getsizeof(measured)
    except Fault as error:
        assert error is marker
    else:
        raise AssertionError("lost descriptor exception")

    target = Sized()
    proxy = weakref.proxy(target)
    size = getsizeof(proxy)  # The referent's __sizeof__ would read its missing result.
    del target
    gc.collect()
    assert getsizeof(proxy) == size and size > 0 and getsizeof(None) > 0

    from collections import OrderedDict

    plain = {}
    ordered = OrderedDict()
    plain_empty = dict.__sizeof__(plain)
    ordered_empty = ordered.__sizeof__()
    for key in range(64):
        plain[key] = ordered[key] = None
    assert dict.__sizeof__(plain) > plain_empty
    assert ordered.__sizeof__() > ordered_empty
    assert ordered.__sizeof__() > dict.__sizeof__(ordered)
    before = getsizeof(ordered)
    ordered[0] = bytearray(20000)
    assert getsizeof(ordered) == before

    class Slotted:
        __slots__ = ("member",)

    child = Slotted()
    child.member = None
    size = object.__sizeof__(child)
    child.member = bytearray(20000)
    assert object.__sizeof__(child) == size
