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
    copied = queue.copy()
    assert copied is not queue and copied.maxlen == 2
    assert copied.popleft() is marker and list(queue) == [marker, 3]
    assert deque.__copy__(queue).popleft() is marker
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
    alias = bounded
    bounded += "abc"
    assert bounded is alias and list(alias) == list("abc")
    bounded += alias
    assert list(bounded) == list("abc")
    assert bounded.__iadd__(range(2)) is alias and list(alias) == ["c", 0, 1]

    class Reflected:
        def __radd__(self, other):
            return marker

    bounded += Reflected()
    assert bounded is marker

    class Match:
        def __init__(self, result):
            self.result = result

        def __eq__(self, other):
            bounded.clear()
            return self.result

    bounded = deque([0, Match(True)])
    contains = bounded.__contains__
    assert type(deque.__contains__).__name__ == "wrapper_descriptor"
    for method in (deque.__contains__, contains):
        assert method.__doc__ == "Return bool(key in self)."
        assert method.__text_signature__ == "($self, key, /)"
    assert contains(0) is True and len(bounded) == 2
    assert deque.__contains__(bounded, 99) is True and not bounded
    assert contains(99) is False
    bounded.append(Match(False))
    try:
        contains(99)
    except RuntimeError as error:
        assert str(error) == "deque mutated during iteration"
    else:
        raise AssertionError("changed deque during membership test")
    try:
        contains()
    except TypeError:
        pass
    else:
        raise AssertionError("missing membership operand")

    class StopCompares:
        def __eq__(self, other):
            raise LookupError("comparison reached")

    stop = StopCompares()
    assert deque([stop]).__contains__(stop)
    assert deque([1, stop]).__contains__(1)
    try:
        deque([stop, 1]).__contains__(1)
    except LookupError as error:
        assert str(error) == "comparison reached"
    else:
        raise AssertionError("membership skipped a comparison error")

    bounded = deque([Match(True)])
    assert bounded.index(99) == 0
    bounded.append(Match(False))
    try:
        bounded.index(99)
    except RuntimeError as error:
        assert str(error) == "deque mutated during iteration"
    else:
        raise AssertionError("changed deque during search")

    class Bound:
        def __index__(self):
            bounded.extend([1, 2, 1])
            return -2

    assert bounded.index(1, Bound(), 10**50) == 2

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
        for extend in (deque.extend, deque.extendleft, deque.__iadd__):
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
    describe = deque.__repr__
    assert type(describe).__name__ == "wrapper_descriptor" and describe.__objclass__ is deque
    assert describe.__doc__ == "Return repr(self)." and describe.__text_signature__ == "($self, /)"
    assert describe(deque()) == "deque([])"
    recursive = deque(["tag"], maxlen=2)
    recursive.append(recursive)
    assert repr(recursive) == recursive.__repr__() == describe(recursive) == "deque(['tag', [...]], maxlen=2)"

    class ClearsOnRepr:
        def __repr__(self):
            recursive.clear()
            return "cleared"

    recursive.extend([ClearsOnRepr(), "last"])
    assert describe(recursive) == "deque([cleared, 'last'], maxlen=2)"
    assert not recursive
    try:
        describe([])
    except TypeError as error:
        assert str(error) == "descriptor '__repr__' requires a 'collections.deque' object but received a 'list'"
    else:
        raise AssertionError("deque repr accepted a list")

    print("deque passed")
