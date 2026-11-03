from array import array
from collections.abc import MutableSequence


def current():
    for code, items in [('i', [-2147483648,0,2147483647]), ('f', [0.1,1.1,-0.0]), ('d',[0.1,1e300]),('w','😀ab')]:
        a = array(code, items)
        assert type(a) is array and isinstance(a, MutableSequence)
        assert type(iter(a)) is type(iter(array(code)))
        b = array(code, a.tobytes())
        assert b.tolist() == list(a) and a[-1] == list(a)[-1]
        print(a.typecode, a.itemsize, len(a), a.tobytes(), list(a))
        it = iter(a)
        assert next(it) == a[0]
        before = array(code, a)
        a.append(items[-1])
        assert list(it) == list(a)[1:]
        assert len(before) + 1 == len(a)
    for code, items in [('i', [2147483648]), ('i', [-2147483649]), ('f', ['0.1']), ('w',['xx']), ('w',[7]), ('i', '123'), ('i', b'123')]:
        try:
            array(code, items)
        except (OverflowError, TypeError, ValueError) as err:
            print(type(err).__name__, str(err))
        else:
            raise AssertionError("invalid array initialization did not raise")

    import sys
    import warnings

    for code in ("h", "H", "I", "l", "L", "q"):
        a = array(code)
        bits = a.itemsize * 8
        signed = code.islower()
        lo = -(1 << (bits - 1)) if signed else 0
        hi = (1 << (bits - int(signed))) - 1
        a.append(lo)
        a.append(hi)
        assert a.typecode == code and list(a) == [lo, hi]
        raw = lo.to_bytes(a.itemsize, sys.byteorder, signed=signed)
        raw += hi.to_bytes(a.itemsize, sys.byteorder, signed=signed)
        assert a.tobytes() == raw and array(code, raw).tolist() == [lo, hi]
        for bad in (lo - 1, hi + 1):
            try:
                a.append(bad)
            except OverflowError:
                pass
            else:
                raise AssertionError("out-of-range array element accepted")
    transitions = array("q", array("i", [-42, 0, 42]))
    view = memoryview(transitions)
    assert view.format == "q" and view.itemsize == 8
    view[0] = -(1 << 63)
    assert transitions.tolist() == [-(1 << 63), 0, 42]

    class OffsetOverflow(OverflowError):
        pass

    class BadOffset:
        def __index__(self):
            raise OffsetOverflow("offset")

    try:
        view[0] = BadOffset()
    except ValueError as error:
        assert type(error) is ValueError
        assert str(error) == "memoryview: invalid value for format 'q'"
    else:
        raise AssertionError("out-of-range offset accepted")
    assert transitions[0] == -(1 << 63)

    if array("L").itemsize == 8:
        n = 1 << 53
        assert memoryview(array("L", [n+1])) != memoryview(array("L", [n]))
        assert memoryview(array("L", [n+1])) != memoryview(array("d", [float(n)]))
        assert memoryview(array("L", [n])) == memoryview(array("d", [float(n)]))

    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always", DeprecationWarning)
        a = array("u", "A😀")
        assert a.typecode == "u" and list(a) == ["A", "😀"]
        assert array("w", a).tobytes() == a.tobytes()
        assert len(seen) == 1 and seen[0].category is DeprecationWarning

    offsets = array("i", [10, 20])
    copies = []

    class Offset:
        def __index__(self):
            offsets.append(30)
            copies.append(array("i", offsets))
            return -7

    offsets[-1] = Offset()
    assert offsets.tolist() == [10, -7, 30] and copies[0].tolist() == [10, 20, 30]
    view = memoryview(offsets)
    offsets[0] += 2
    assert view[0] == 12
    view.release()

    for code in ("b", "B", "i", "f", "d"):
        owner = array(code, [65, 66])
        view = memoryview(owner)
        assert view.obj is owner and not view.readonly
        assert view.format == code and view.itemsize == owner.itemsize
        assert len(view) == 2 and view.shape == (2,) and view.nbytes == 2 * owner.itemsize
        assert bytes(view) == owner.tobytes()
        assert view == b"AB" and view == memoryview(array("i", [65, 66]))
        try:
            hash(view)
        except ValueError:
            pass
        else:
            raise AssertionError("writable view was hashable")
        other = memoryview(view)
        view.release()
        try:
            owner.append(67)
        except BufferError:
            pass
        else:
            raise AssertionError("resized while the derived view remained live")
        other.release()
        owner.append(67)
        temporary = memoryview(owner)
        del temporary
        owner.append(68)
        assert len(owner) == 4

    for code in ('f', 'd'):
        view = memoryview(array(code, [float('nan')]))
        assert view != view and not (view == view)
        assert [view] == [view]
        view.release()
        assert view == view

    import gc
    gc.disable()
    try:
        cycle = []
        cycle.extend([cycle, memoryview(owner)])
        del cycle
        try:
            owner.append(69)
        except BufferError:
            pass
        else:
            raise AssertionError("resized before collecting an unreachable view cycle")
        gc.collect()
        owner.append(69)
    finally:
        gc.enable()

    import weakref

    payload = array("B", b"abc")
    callbacks = []
    handle = weakref.ref(payload, callbacks.append)
    access = weakref.proxy(payload)
    view = memoryview(payload)
    del payload
    gc.collect()
    assert handle() is view.obj and access.tobytes() == b"abc"
    assert not callbacks
    view.release()
    gc.collect()
    assert handle() is None and len(callbacks) == 1 and callbacks[0] is handle
    try:
        access.tobytes()
    except ReferenceError:
        pass
    else:
        raise AssertionError("proxy survived its array")

    a = array('i', [0])
    views = []
    class ExportDuringAppend:
        def __index__(self):
            if not views:
                a.append(17)
                views.append(memoryview(a))
            return 42
    a.append(ExportDuringAppend())
    assert a.tolist() == [0, 42] and bytes(views[0]) == a.tobytes()
    views[0].release()

    class Number:
        def __index__(self):
            return 17

    class Distance:
        def __float__(self):
            return 5.5

    for code, low, high in (('b', -128, 127), ('B', 0, 255)):
        a = array(code, [low, Number(), high])
        assert a.typecode == code and a.itemsize == 1
        assert a.tolist() == [low, 17, high]
        assert array(code, a.tobytes()).tolist() == list(a)
        assert list(array(code, range(10))) == list(range(10))
        before = a.tobytes()
        for invalid in (low - 1, high + 1, -32769, 32768, 1 << 80, 1.0):
            try:
                a.append(invalid)
            except (OverflowError, TypeError) as error:
                print(code, type(error).__name__, str(error))
            else:
                raise AssertionError("invalid byte append succeeded")
        assert a.tobytes() == before

    class Counter:
        def __init__(self, fail_at=0):
            self.calls = 0
            self.fail_at = fail_at

        def __index__(self):
            self.calls += 1
            if self.calls == self.fail_at:
                raise RuntimeError("conversion failed")
            return self.calls

    for code in ('b', 'B', 'i'):
        item = Counter()
        assert list(array(code, [item])) == [1] and item.calls == 1
        item = Counter()
        assert list(array(code, (item,))) == [1] and item.calls == 1
        item = Counter()
        assert list(array(code, iter([item]))) == [2] and item.calls == 2
        for fail_at in (1, 2):
            a = array(code, [3])
            item = Counter(fail_at)
            try:
                a.append(item)
            except RuntimeError as error:
                assert str(error) == "conversion failed"
            else:
                raise AssertionError("append ignored conversion error")
            assert item.calls == fail_at and len(a) == fail_at and a[0] == 3

    class DuringAppend:
        def __init__(self, receiver):
            self.receiver = receiver
            self.lengths = []

        def __index__(self):
            a = self.receiver
            self.lengths.append(len(a))
            if len(self.lengths) == 1:
                a.append(90)
                a.append(91)
                return 7
            self.snapshot = array(a.typecode, a)
            a.append(92)
            return 8

    a = array('b', [3])
    item = DuringAppend(a)
    a.append(item)
    assert item.lengths == [1, 2] and list(a) == [3, 8, 92]
    assert list(item.snapshot) == [3, 90]

    indexed = array('l', [10, 20])
    assert indexed.__getitem__(-1) == array.__getitem__(indexed, 1) == 20
    assert array.__setitem__(indexed, -1, -7) is None
    assert indexed.__setitem__(0, 8) is None and list(indexed) == [8, -7]

    assert list(array('i',[Number()])) == [17]
    assert list(array('f',[Distance()])) == [5.5]
    assert list(array('d',(float(i) for i in range(3)))) == [0.0,1.0,2.0]
    class EmptyList(list):
        def __iter__(self):
            raise RuntimeError("list initialization uses indexing")

    assert list(array("i", EmptyList([4,5]))) == [4,5]

    from io import BytesIO
    sample = array('i', [0x12345678])
    loaded = array('i', [-9])
    assert array.frombytes(loaded, memoryview(sample).cast('B')) is None
    with memoryview(loaded) as view:
        assert view.format == 'i' and view.itemsize == sample.itemsize
        assert loaded.frombytes(b'') is None
        for buf, kind in ((sample, TypeError), (b'x', ValueError),
                          (sample.tobytes(), BufferError), (memoryview(b'abc')[::2], BufferError)):
            try:
                loaded.frombytes(buf)
            except kind:
                pass
            else:
                raise AssertionError("invalid array buffer was accepted")
        assert view.tobytes() == array('i', [-9, 0x12345678]).tobytes()
    loaded.frombytes(bytearray(sample.tobytes()))

    backing = bytearray(b"prefix-data-suffix")
    sliced = memoryview(backing)[7:11]
    copied = array('B')
    copied.frombytes(sliced)
    backing[7] = 0
    assert copied.tobytes() == b"data"
    cast_slice = memoryview(sample).cast('B')[1:-1]
    copied.frombytes(cast_slice)
    assert copied.tobytes() == b"data" + sample.tobytes()[1:-1]
    assert loaded.tolist() == [-9, 0x12345678, 0x12345678]
    one_byte = array('B', [1])
    try:
        one_byte.frombytes(one_byte)
    except BufferError:
        pass
    else:
        raise AssertionError("array grew during a self-export")
    assert one_byte.tolist() == [1]

    source = BytesIO(sample.tobytes())
    appended = array('i')
    try:
        appended.fromfile(source, 2)
    except EOFError:
        pass
    else:
        raise AssertionError("expected a short array read")
    assert appended.tolist() == sample.tolist()
    with memoryview(appended) as view:
        assert array.byteswap(appended) is None
        assert view.tobytes() == sample.tobytes()[::-1]
