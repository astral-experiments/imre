from collections import deque


def current():
    marker = []
    queue = deque((item for item in (marker, "middle", 3)))
    assert type(queue) is deque and isinstance(queue, deque)
    assert deque.__module__ == "collections"
    assert queue and len(queue) == 3
    assert queue.popleft() is marker and queue.pop() == 3
    assert tuple(queue) == ("middle",) and list(queue) == ["middle"]
    assert deque.pop(queue) == "middle" and not queue
    for take in (queue.pop, queue.popleft):
        try:
            take()
        except IndexError as error:
            assert str(error) == "pop from an empty deque"
        else:
            raise AssertionError("empty deque")

    queue = deque(iterable=range(5), maxlen=2)
    queue.appendleft(marker)
    assert queue.maxlen == 2 and list(queue) == [marker, 3]
    cursor = iter(queue)
    assert iter(cursor) is cursor and next(cursor) is marker
    queue.append("last")
    try:
        next(cursor)
    except RuntimeError as error:
        assert str(error) == "deque mutated during iteration"
    else:
        raise AssertionError("changed deque")
    assert list(queue) == [3, "last"]

    queue = deque([marker])
    push = queue.append
    remove = queue.remove
    assert push.__self__ is queue and remove.__self__ is queue
    push("tail")
    queue.extend(queue)
    remove(marker)
    assert list(queue) == ["tail", marker, "tail"]
    class Source:
        def __init__(self):
            self.count = 0

        def __iter__(self):
            return self

        def __next__(self):
            self.count += 1
            if self.count == 1:
                return marker
            raise ValueError("source")

    for queue in (deque(maxlen=0), deque()):
        source = Source()
        try:
            queue.extend(source)
        except ValueError as error:
            assert str(error) == "source"
        else:
            raise AssertionError("iterator error")
        assert source.count == 2
        if queue.maxlen is None:
            assert queue.popleft() is marker
        assert not queue
    print("deque passed")
