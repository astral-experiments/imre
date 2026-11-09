import copy
import pickle


def current():
    source = iter(range(1, 10, 2))
    jobs = filter(None, source)
    assert next(jobs) == 1
    assert jobs.__reduce__() == (filter, (None, source))
    shallow = copy.copy(jobs)
    deep = copy.deepcopy(jobs)
    assert next(shallow) == 3
    assert next(jobs) == 5
    assert list(deep) == [3, 5, 7, 9]

    for values in (range(1, 10, 3), reversed(range(1, 10, 3)),
                   range(2**90, 2**90 - 7, -2)):
        source = iter(values)
        next(source)
        jobs = filter(bool, source)
        expected = list(copy.copy(source))
        for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
            restored = pickle.loads(pickle.dumps(jobs, protocol))
            assert type(restored) is filter
            assert list(restored) == expected
        assert list(jobs) == expected
        assert list(copy.deepcopy(jobs)) == []

    # Older pickles provide the consumed count as state for the range iterator.
    legacy = b'c__builtin__\niter\n(c__builtin__\nxrange\n(I10\nI20\nI2\ntRtRI2\nb.'
    cursor = pickle.loads(legacy)
    sibling = copy.copy(cursor)

    class Offset:
        def __index__(self):
            assert next(cursor) == 14
            return 1

    cursor.__setstate__(Offset())
    assert list(cursor) == [18]
    assert list(sibling) == [14, 16, 18]

    cursor = iter(range(-(2**100), 6, 2))
    try:
        cursor.__setstate__(True)
    except TypeError:
        pass
    else:
        raise AssertionError("long range iterator accepted a bool state")
    cursor.__setstate__(2**99 + 1)
    assert list(cursor) == [2, 4]

    class Jobs(filter):
        pass

    source = iter(range(1, 7))
    jobs = Jobs(None, source)
    assert jobs.__reduce__() == (Jobs, (None, source))
    shallow = copy.copy(jobs)
    assert type(shallow) is Jobs
    assert next(shallow) == 1
    assert next(jobs) == 2

    class Tagged(Jobs):
        def __init__(self, predicate, source, *, tag):
            self.tag = tag

        def __next__(self):
            return super().__next__() * 10

    jobs = Tagged(None, [0, 3, 4], tag="ready")
    assert jobs.tag == "ready" and iter(jobs) is jobs
    assert next(jobs) == 30
    assert filter.__next__(jobs) == 4

    class Named(Jobs):
        def __new__(cls, source, *, name):
            jobs = super().__new__(cls, None, source)
            jobs.name = name
            return jobs

        def __reduce__(self):
            return list, ((self.name,),)

    named = Named([0, 5], name="pending")
    assert next(named) == 5
    assert copy.copy(named) == ["pending"]
