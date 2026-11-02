import _collections
import collections
import operator
import pickle


def current():
    deque = collections.deque
    assert deque is vars(_collections)["deque"]
    assert collections._deque_iterator is _collections._deque_iterator
    for make, reverse in ((_collections._deque_iterator, False),
                          (_collections._deque_reverse_iterator, True)):
        d = deque([10, 20])

        class Index:
            def __index__(self):
                d.append(30)
                return 1

        it = make(d, Index())
        assert type(it) is type(reversed(d) if reverse else iter(d))
        assert operator.length_hint(it) == 2
        factory, args = it.__reduce_ex__(4)
        assert factory is make and args[0] is d and args[1] == 1
        assert list(factory(*args)) == ([20, 10] if reverse else [20, 30])
        assert list(make(d, 100)) == []
        d.clear()
        assert it.__reduce__()[1][1] == -2
        try:
            next(it)
        except RuntimeError:
            pass
        else:
            raise AssertionError("iterator missed mutation")
        assert it.__reduce__()[1][1] == 0

    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        owner = deque([None, "tail"], maxlen=2)
        owner[0] = owner
        cursor = iter(owner)
        assert next(cursor) is owner
        restored_iter, restored = pickle.loads(pickle.dumps((cursor, owner), protocol))
        assert restored is not owner and restored[0] is restored and restored.maxlen == 2
        marker = object()
        restored[1] = marker
        assert next(restored_iter) is marker and list(restored_iter) == []

    # The public collections module exports only the forward factory for pickle.
    assert pickle.loads(pickle.dumps(collections._deque_iterator)) is collections._deque_iterator
    factory = collections._deque_iterator
    del collections._deque_iterator
    try:
        assert not hasattr(collections, "_deque_iterator")
    finally:
        collections._deque_iterator = factory
    try:
        pickle.dumps(reversed(deque()))
    except pickle.PicklingError:
        pass
    else:
        raise AssertionError("unexpected reverse iterator global")
