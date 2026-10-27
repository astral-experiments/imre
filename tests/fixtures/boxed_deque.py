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

    left, right = deque([1, 2], 2), deque([1, 2], 5)
    assert left == right and [left] == [right] and not (left != right)
    right.append(3)
    assert left != right and not (left == right)

    class Item:
        def __eq__(self, other):
            left.append(7)
            return True

    left, right = deque([Item()]), deque([Item()])
    try:
        left == right
    except RuntimeError as error:
        assert str(error) == "deque mutated during iteration"
    else:
        raise AssertionError("changed deque during comparison")

    queue = deque([1, 2, 3, 4], maxlen=4)
    forward, backward = iter(queue), reversed(queue)
    assert (next(forward), next(backward)) == (1, 4)
    reverse = queue.reverse
    assert reverse.__self__ is queue and reverse() is None
    assert list(forward) == [3, 2, 1] and list(backward) == [2, 3, 4]
    assert queue.maxlen == 4 and list(queue) == [4, 3, 2, 1]
    cursor = iter(queue)
    queue[False] = marker
    assert next(cursor) is marker and queue.__getitem__(-1) == 1
    del queue[-1]
    try:
        next(cursor)
    except RuntimeError:
        pass
    else:
        raise AssertionError("deque deletion kept its cursor valid")
    backward = deque.__reversed__(queue)
    assert type(backward).__name__ == "_deque_reverse_iterator"
    assert iter(backward) is backward
    queue.append(0)
    try:
        next(backward)
    except RuntimeError:
        pass
    else:
        raise AssertionError("changed deque")
    assert next(backward, None) is None

    queue = deque([marker])
    push = queue.append
    remove = queue.remove
    assert push.__self__ is queue and remove.__self__ is queue
    push("tail")
    queue.extend(queue)
    remove(marker)
    assert list(queue) == ["tail", marker, "tail"]
    bounded = deque([1, 2], maxlen=3)
    assert deque.extendleft(bounded, [3, 4]) is None
    assert list(bounded) == [4, 3, 1]
    bounded.extendleft(bounded)
    assert list(bounded) == [1, 3, 4]
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

    for bound in (0, None):
        for extend in (deque.extend, deque.extendleft):
            queue = deque(maxlen=bound)
            source = Source()
            try:
                extend(queue, source)
            except ValueError as error:
                assert str(error) == "source"
            else:
                raise AssertionError("iterator error")
            assert source.count == 2
            if bound is None:
                assert queue.popleft() is marker
            assert not queue
    alias = deque([1, 2], maxlen=2)
    try:
        alias.__init__([], -1)
    except ValueError:
        pass
    assert list(alias) == [1, 2] and alias.maxlen == 2
    try:
        deque.__init__(alias, Source(), 3)
    except ValueError:
        pass
    assert list(alias) == [marker] and alias.maxlen == 3
    assert alias.clear() is None and not alias and alias.maxlen == 3
    alias.append(marker)
    assert alias.__init__(alias) is None and not alias and alias.maxlen is None
    print("deque passed")
