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
    print('accumulate')


if __name__ == '__main__':
    current()
