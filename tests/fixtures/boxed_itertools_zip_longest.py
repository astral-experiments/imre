import gc
import importlib
import itertools
import sys
import weakref
from itertools import zip_longest


def current():
    assert itertools.zip_longest is zip_longest
    assert zip_longest.__module__ == 'itertools'
    assert list(zip_longest()) == []
    assert list(zip_longest([], [])) == []
    marker = []
    rows = list(zip_longest((1, 2), (), (3,), fillvalue=marker))
    assert rows == [(1, marker, 3), (2, marker, marker)]
    assert rows[0][1] is marker and rows[1][2] is marker

    events = []
    error = ValueError('iterator failure')

    class Source:
        def __init__(self, name, values):
            self.name = name
            self.values = iter(values)

        def __iter__(self):
            events.append(('iter', self.name))
            return self

        def __next__(self):
            events.append(('next', self.name))
            value = next(self.values)
            if value is error:
                raise value
            return value

    class Child(zip_longest):
        def __init__(self, *args, **kwargs):
            events.append(('init', 'child'))

        @property
        def _zip_longest_state(self):
            return 'child state'

    left = Source('left', (1, 2, 3))
    right = Source('right', (4, error, 6))
    z = Child(left, right)
    assert type(z) is Child and iter(z) is z and z._zip_longest_state == 'child state'
    assert events == [('iter', 'left'), ('iter', 'right'), ('init', 'child')]
    assert next(z) == (1, 4)
    zip_longest.__init__(z)
    try:
        next(z)
    except ValueError as exc:
        assert exc is error
    else:
        raise AssertionError('lost input exception')
    assert next(z, None) is None
    assert events[3:] == [('next', 'left'), ('next', 'right')] * 2
    assert next(left) == 3 and next(right) == 6

    events.clear()
    try:
        zip_longest(left, bogus=None)
    except TypeError:
        pass
    else:
        raise AssertionError('unexpected keyword accepted')
    assert events == []
    try:
        zip_longest(left, 42, right)
    except TypeError:
        pass
    else:
        raise AssertionError('invalid iterable accepted')
    assert events == [('iter', 'left')]

    def make_zipper():
        short = Source('short', ())
        final = Source('final', (1,))
        return zip_longest(short, final), weakref.ref(short), weakref.ref(final)

    cursor, short, final = make_zipper()
    assert next(cursor) == (None, 1)
    gc.collect()
    assert short() is None and final() is not None
    assert next(cursor, None) is None and next(cursor, None) is None
    gc.collect()
    assert final() is not None
    del cursor
    gc.collect()
    assert final() is None
    assert not hasattr(zip_longest(), '_zip_longest_state')
    cursor = zip_longest([1, 2], [])
    saved = sys.modules.pop("itertools")
    try:
        fresh = importlib.import_module("itertools").zip_longest
        for cls in (zip_longest, fresh):
            try:
                object.__new__(cls)
            except TypeError:
                pass
            else:
                raise AssertionError("unsafe zip_longest allocation")
            assert list(cls([0])) == [(0,)]
        assert list(cursor) == [(1, None), (2, None)]
    finally:
        sys.modules["itertools"] = saved
    print('zip_longest')


if __name__ == '__main__':
    current()
