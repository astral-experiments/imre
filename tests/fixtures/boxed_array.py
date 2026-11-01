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

    assert list(array('i',[Number()])) == [17]
    assert list(array('f',[Distance()])) == [5.5]
    assert list(array('d',(float(i) for i in range(3)))) == [0.0,1.0,2.0]
    class EmptyList(list):
        def __iter__(self):
            raise RuntimeError("list initialization uses indexing")

    assert list(array("i", EmptyList([4,5]))) == [4,5]
