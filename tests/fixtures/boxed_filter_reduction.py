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
