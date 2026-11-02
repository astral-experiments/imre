def current():
    data = {'here', frozenset({1, 2})}
    remove = data.remove
    assert remove('here') is None
    mutable = {2, 1}
    assert set.remove(data, mutable) is None and not data
    data.add('retained')
    for key in ('absent', (3,), mutable):
        try:
            remove(key)
        except KeyError as exc:
            assert len(exc.args) == 1 and exc.args[0] is key
        else:
            raise AssertionError('missing key did not raise')
    assert data == {'retained'}

    class HashableSet(set):
        def __hash__(self):
            return 71

    hashable = HashableSet({1, 2})
    bucket = {hashable}
    assert bucket.remove(hashable) is None and not bucket

    for arguments in ((), (1, 2)):
        try:
            remove(*arguments)
        except TypeError as exc:
            assert str(exc) == f'set.remove() takes exactly one argument ({len(arguments)} given)'
        else:
            raise AssertionError('invalid argument count accepted')

    try:
        set.remove(frozenset(), 1)
    except TypeError as exc:
        assert str(exc) == "descriptor 'remove' for 'set' objects doesn't apply to a 'frozenset' object"
    else:
        raise AssertionError('frozenset receiver accepted')

    try:
        remove([])
    except TypeError:
        pass
    else:
        raise AssertionError('list accepted')
    assert data.discard(mutable) is None

    class HashError:
        def __hash__(self):
            raise RuntimeError('hash failed')

    try:
        remove(HashError())
    except RuntimeError as exc:
        assert str(exc) == 'hash failed'
    else:
        raise AssertionError('hash error lost')


if __name__ == "__main__":
    current()
