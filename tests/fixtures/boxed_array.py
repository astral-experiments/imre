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

    assert list(array('i',[Number()])) == [17]
    assert list(array('f',[Distance()])) == [5.5]
    assert list(array('d',(float(i) for i in range(3)))) == [0.0,1.0,2.0]
    class EmptyList(list):
        def __iter__(self):
            raise RuntimeError("list initialization uses indexing")

    assert list(array("i", EmptyList([4,5]))) == [4,5]
