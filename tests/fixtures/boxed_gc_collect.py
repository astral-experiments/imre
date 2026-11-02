import gc
from weakref import ref


class Node:
    pass


def current():
    callbacks = []
    node = Node()
    node.cycle = node
    handle = ref(node, callbacks.append)
    assert gc.collect() >= 0
    assert handle() is node and not callbacks

    roots = [node]
    del node
    # Keep these in one expression, so no intervening statement collects the cycle.
    observed = roots.clear(), len(callbacks), gc.collect(generation=2), len(callbacks)
    assert observed == (None, 0, 1, 1)
    assert handle() is None and callbacks == [handle]

    # An unreachable cycle with no weak references still needs collection.
    cycle = []
    cycle.append(cycle)
    del cycle
    assert gc.collect() == 1
    assert gc.collect() == 0

    # A tracked child held only by the cycle is also collected. Atomic
    # values and a list that never joins the cycle must not change the count.
    cycle = []
    cycle.extend((cycle, [], {}, 123, "atomic"))
    acyclic = [123]
    del acyclic
    del cycle
    assert gc.collect(2) == 3
    assert gc.collect() == 0
    try:
        gc.collect(-1)
    except ValueError as error:
        assert str(error) == "invalid generation"
    else:
        raise AssertionError("accepted an invalid generation")

    def tuple_count(kind, held):
        gc.collect()
        x = len(kind)
        if kind == "packed":
            value = (x, x)
        elif kind == "list":
            value = tuple([x, x])
        elif kind == "iterator":
            value = tuple(iter([x, x]))
        elif kind == "concat":
            value = (x,) + (x,)
        elif kind == "repeat":
            value = (x,) * 2
        elif kind == "nested":
            value = (((x,),),)
        elif kind == "static-type":
            value = tuple([int])
        elif kind == "function":
            value = tuple([len])
        elif kind == "object":
            value = (object(),)
        else:
            value = ([],)
        cycle = [value]
        cycle.append(cycle)
        del value
        for _ in range(held):
            gc.collect()
        del cycle
        return gc.collect()

    for kind, counts in [
        ("packed", [1, 1, 1]), ("list", [1, 1, 1]),
        ("iterator", [2, 1, 1]), ("concat", [2, 1, 1]), ("repeat", [2, 1, 1]),
        ("nested", [3, 2, 1]), ("mutable", [3, 3, 3]),
        ("static-type", [2, 1, 1]), ("function", [2, 2, 2]), ("object", [1, 1, 1]),
    ]:
        assert [tuple_count(kind, held) for held in range(3)] == counts

    # Keeping the middle tuple directly reachable lets the same collection
    # untrack it before checking its parent.
    x = len(counts)
    middle = ((x,),)
    outer = (middle,)
    cycle = [outer]
    cycle.append(cycle)
    gc.collect()
    del middle, outer, cycle
    assert gc.collect() == 1

    def container_count(kind):
        x = len(kind)
        middle = ((x,),)
        outer = (middle,)
        if kind == "duplicate":
            cycle = [middle, outer, middle]
            cycle.append(cycle)
        else:
            cycle = {}
            if kind == "middle-first":
                cycle["middle"], cycle["outer"] = middle, outer
            else:
                cycle["outer"], cycle["middle"] = outer, middle
            cycle["self"] = cycle
        del middle, outer
        gc.collect()
        del cycle
        return gc.collect()

    assert container_count("middle-first") == 1
    assert container_count("outer-first") == 2
    assert container_count("duplicate") == 1

    # A reentrant collection cannot collect the isolated graph twice or claim
    # new garbage produced by a weakref callback.
    nested_counts = []
    def nested_collect(wr):
        assert wr() is None
        created = []
        created.append(created)
        del created
        nested_counts.append(gc.collect(2))

    node = Node()
    node.cycle = node
    nested_ref = ref(node, nested_collect)
    del node
    assert gc.collect(2) == 1
    assert nested_ref() is None and nested_counts == [0]
    assert gc.collect(2) == 1
    assert gc.collect(2) == 0

    # CPython includes a weakref destroyed when its callback drops the last owner.
    refs = []
    def drop_ref(wr):
        assert wr() is None
        refs.clear()

    node = Node()
    node.cycle = node
    refs.append(ref(node, drop_ref))
    del node
    assert gc.collect(2) == 2 and not refs
    assert gc.collect(2) == 0

    # An isolated weak wrapper keeps its callback's strong edge during GC.
    # With a bound method, this edge can be what holds the cycle together.
    class Owner:
        def observe(self, wr):
            raise AssertionError("unreachable weakref callback ran")

    seen = []
    def witness(wr):
        seen.append(wr.__callback__ is witness)

    owner = Owner()
    watched = ref(owner, witness)
    wrapper = ref(owner, owner.observe)
    owner.wrapper = wrapper
    del owner, wrapper
    assert gc.collect(2) == 3
    assert watched() is None and watched.__callback__ is witness and seen == [True]
    owner = Owner()
    watched = ref(owner, witness)
    del owner
    assert watched() is None and watched.__callback__ is None and seen == [True, False]

    class Key:
        owners = {}

    Key.owners[Key] = None
    held = Key.owners
    callbacks = []
    key = ref(Key, callbacks.append)
    del Key
    gc.collect()
    assert key() in held and not callbacks
    del held
    gc.collect()
    assert key() is None and callbacks == [key]
