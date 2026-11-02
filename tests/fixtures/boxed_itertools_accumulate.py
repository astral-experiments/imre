import gc
import itertools
import weakref
from itertools import accumulate


def current():
    assert accumulate is itertools.accumulate
    assert list(accumulate(iterable=(), initial=0)) == [0]
    assert list(accumulate(iterable=(1, 2), func=None, initial=None)) == [1, 3]
    assert list(accumulate(['a', 'b', 'c'])) == ['a', 'ab', 'abc']
    assert list(accumulate((2, 3, 4), lambda a, b: a * b)) == [2, 6, 24]

    events = []
    failure = ValueError('source or callback')

    class Source:
        def __init__(self, items):
            self.items = iter(items)

        def __iter__(self):
            events.append('iter')
            return self

        def __next__(self):
            events.append('next')
            item = next(self.items)
            if item is failure:
                raise failure
            return item

    class Child(accumulate):
        def __init__(self, *args, **kwargs):
            events.append('init')

        @property
        def _accumulate_state(self):
            return 'child state'

    def combine(total, value):
        if value == 3:
            raise failure
        return (total or 0) + value

    it = Child(Source((None, failure, 2, 3, 4)), combine)
    assert it._accumulate_state == 'child state' and type(it) is Child and iter(it) is it
    assert events == ['iter', 'init']
    assert next(it) is None
    for expected in (2, 6):
        try:
            next(it)
        except ValueError as exc:
            assert exc is failure
        else:
            raise AssertionError('suppressed iterator exception')
        assert next(it) == expected
    assert next(it, None) is None

    class Marker:
        pass

    class Replace:
        def __call__(self, old, new):
            return new

    def make():
        source = Source((1,))
        initial = Marker()
        func = Replace()
        cursor = accumulate(source, func, initial=initial)
        return cursor, weakref.ref(source), weakref.ref(func), weakref.ref(initial)

    cursor, source, func, initial = make()
    assert initial() is not None and next(cursor) is initial()
    assert next(cursor) == 1 and next(cursor, None) is None
    gc.collect()
    assert initial() is None and source() is not None and func() is not None
    del cursor
    gc.collect()
    assert source() is None and func() is None

    it = accumulate.__new__(Child, (1, 2), initial=10)
    assert type(it) is Child and next(it) == 10
    accumulate.__init__(it)
    assert list(it) == [11, 13]
    assert not hasattr(accumulate(()), '_accumulate_state')
    for protocol in (0, 1, 2, 5):
        cursor = accumulate(())
        for reducer in (cursor.__reduce_ex__, lambda p: object.__reduce_ex__(cursor, p)):
            try:
                reducer(protocol)
            except TypeError as exc:
                name = 'accumulate' if protocol < 2 else 'itertools.accumulate'
                assert str(exc) == f"cannot pickle '{name}' object"
            else:
                raise AssertionError('exposed an incomplete reconstruction tuple')

    class Bare(accumulate):
        pass

    child = Bare([2, 3])
    rebuild, (cls, base, state) = object.__reduce__(child)
    assert rebuild.__name__ == '_reconstructor' and cls is Bare and base is accumulate
    assert type(state) is accumulate and list(state) == [2, 7]
    class Own(accumulate):
        def __new__(cls, iterable):
            return super().__new__(cls, iterable)

    for cls, typename, owner in ((accumulate, 'itertools.accumulate', 'itertools.accumulate'),
                                 (Bare, 'Bare', 'Bare'), (Own, 'Own', 'itertools.accumulate')):
        try:
            object.__new__(cls)
        except TypeError as exc:
            assert str(exc) == f'object.__new__({typename}) is not safe, use {owner}.__new__()'
        else:
            raise AssertionError('created an uninitialized accumulate subclass')
    try:
        object.__reduce_ex__(Bare([]), 2)
    except TypeError:
        pass
    else:
        raise AssertionError('lost native cursor state')

    class WithArgs(accumulate):
        def __getnewargs_ex__(self):
            return (([2, 3],), {'initial': 7})

    rebuild, args, state, items, pairs = object.__reduce_ex__(WithArgs([]), 5)
    assert rebuild.__name__ == '__newobj_ex__'
    assert args == (WithArgs, ([2, 3],), {'initial': 7})
    assert state is None and items is None and pairs is None

    print('accumulate')


if __name__ == '__main__':
    current()
