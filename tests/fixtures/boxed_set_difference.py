def current():
    for kind in (set, frozenset):
        source = kind([1, 2, 3])
        other = kind([2])
        result = source.difference()
        assert result == source and result is not source and type(result) is kind
        assert source.difference(other, [3]) == {1}
        assert kind.difference(source, {2: None}, iter([3])) == {1}
        assert source == {1, 2, 3}

    class Frozen(frozenset):
        pass

    assert type(Frozen([1]).difference()) is frozenset
    events = []

    class Source:
        def __iter__(self):
            events.append("iteration")
            yield 5
            raise ValueError("stop here")

    try:
        set().difference(Source(), Source())
    except ValueError as error:
        assert str(error) == "stop here"
    else:
        raise AssertionError("iterator failure was lost")
    assert events == ["iteration"]

    class Key:
        hashable = True

        def __hash__(self):
            assert self.hashable
            return 19

    key = Key()
    source, other = {key}, {key: None}
    key.hashable = False
    assert source.difference(other) == set()

    later = []

    class Cursor:
        def __iter__(self):
            return self

        def __next__(self):
            raise StopIteration

        def __del__(self):
            later.append(1)

    class Input:
        def __iter__(self):
            return Cursor()

    assert {1}.difference(Input(), later) == set()
    assert later == [1]
