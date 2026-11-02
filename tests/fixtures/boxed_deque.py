import gc
import weakref
from collections import deque
from types import MappingProxyType


def check_weakrefs():
    class Slotted(deque):
        __slots__ = ()

    for cls in (deque, Slotted):
        target = cls([1])
        holder = deque([target])
        callbacks = []
        watch = weakref.ref(target, callbacks.append)
        proxy = weakref.proxy(target)
        assert watch() is target and str(proxy) == str(target)
        assert not hasattr(target, "__weakref__")
        del target
        gc.collect()
        assert watch() is holder[0] and not callbacks
        holder.clear()
        gc.collect()
        assert watch() is None and len(callbacks) == 1 and callbacks[0] is watch
        try:
            str(proxy)
        except ReferenceError:
            pass
        else:
            raise AssertionError("dead deque proxy still forwards")

    for obj in ([], iter(deque()), reversed(deque())):
        try:
            weakref.ref(obj)
        except TypeError:
            pass
        else:
            raise AssertionError("unexpected weak reference support")


def current():
    check_weakrefs()
    marker = []
    queue = deque((item for item in (marker, "middle", 3)))
    assert type(queue) is deque and isinstance(queue, deque)
    assert deque.__module__ == "collections"
    assert deque.__reduce_ex__ is object.__reduce_ex__
    for limit, expected_args in ((None, ()), (0, ((), 0)), (2, ((), 2))):
        original = deque((None, 3), maxlen=limit)
        for reduce in (original.__reduce__, lambda: original.__reduce_ex__(5)):
            cls, args, state, items = reduce()
            assert cls is deque and args == expected_args and state is None
            if original:
                original[0] = marker
            assert list(items) == list(original)
    namespace = deque.__dict__
    assert type(namespace) is MappingProxyType and namespace is not deque.__dict__
    assert "__name__" not in namespace and "__str__" not in namespace
    for name in ("__repr__", "__contains__", "append", "__module__"):
        assert namespace[name] is getattr(deque, name)
    assert namespace["__repr__"].__objclass__ is deque
    assert namespace["__contains__"](queue, marker)
    for name in ("__len__", "append", "__copy__", "__reversed__"):
        descriptor = namespace[name]
        assert descriptor.__objclass__ is deque and descriptor.__qualname__ == "deque." + name
        assert descriptor.__get__(None, deque) is descriptor
        bound = descriptor.__get__(queue)
        assert bound.__self__ is queue and bound.__qualname__ == descriptor.__qualname__
        if name == "__len__":
            assert type(descriptor).__name__ == "wrapper_descriptor"
            assert type(bound).__name__ == "method-wrapper" and bound() == 3
        else:
            assert type(descriptor).__name__ == "method_descriptor"
            assert type(bound).__name__ == "builtin_function_or_method"
        try:
            descriptor.__get__([])
        except TypeError as error:
            assert name in str(error)
        else:
            raise AssertionError("bound deque descriptor to a list")
    try:
        namespace["append"] = None
    except TypeError:
        assert namespace["append"] is deque.append
    else:
        raise AssertionError("writable deque namespace")
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

    class Recorded(deque):
        def __new__(cls, *args, **kwargs):
            result = super().__new__(cls, *args, **kwargs)
            result.allocated = len(args)
            return result

        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self.source = args[0]

    derived = Recorded([marker, 1, 2], maxlen=2)
    derived.extra = 1
    duplicate = derived.copy()
    assert type(duplicate) is Recorded and duplicate is not derived
    assert derived.allocated == 1 and duplicate.allocated == 2
    assert duplicate.source is derived and not hasattr(duplicate, "extra")
    assert derived.maxlen == duplicate.maxlen == 2 and list(duplicate) == [1, 2]
    assert derived == duplicate == deque([1, 2]) and not (duplicate != derived)
    assert deque.__eq__(derived, [1, 2]) is NotImplemented
    assert Recorded.__hash__ is None
    assert deque.pop.__get__(derived).__self__ is derived
    assert deque.__iadd__(derived, derived) is derived and list(derived) == [1, 2]
    try:
        derived.maxlen = 20
    except AttributeError:
        assert derived.maxlen == 2
    else:
        raise AssertionError("writable deque maxlen")

    class EmptyView(deque):
        def __iter__(self):
            return iter(())

    derived = EmptyView([1, 2])
    assert list(derived) == [] and list(deque.__iter__(derived)) == [1, 2]
    assert type(derived.copy()) is EmptyView and len(derived.copy()) == 0
    assert repr(derived) == "EmptyView([])"
    assert derived == EmptyView([3, 4]) and derived != deque([1, 2])
    assert derived != EmptyView([1])

    print("deque passed")
