import gc
from weakref import ref


def check_generator_loops():
    closed = []

    def suspended(label, retained=None, on_close=None):
        try:
            yield 1
            yield 2
        finally:
            if retained is not None:
                assert retained
            closed.append(label)
            if on_close is not None:
                on_close()

    def start(label, keep=None, on_close=None):
        generator = suspended(label, on_close=on_close)
        for item in generator:
            assert item == 1
            break
        if keep is not None:
            return keep(generator)

    start("unowned")
    assert closed == ["unowned"]

    returned = start("returned", lambda generator: generator)
    assert next(returned) == 2
    returned.close()

    error = SyntaxError()

    def attach(generator):
        error.end_lineno = generator

    start("error attribute", attach)
    assert next(error.end_lineno) == 2
    error.end_lineno = None
    assert closed[-1] == "error attribute"

    watched = []

    def watch(generator):
        watched.append(ref(generator, lambda reference: closed.append("weak callback")))

    start("watched", watch)
    assert watched[0]() is None
    assert sorted(closed[-2:]) == ["watched", "weak callback"]

    others = []
    start("other", others.append)
    start("outer", on_close=others.clear)
    assert closed[-2:] == ["outer", "other"]

    def delegate(values):
        yield from values

    inner = suspended("delegated generator")
    outer = delegate(inner)
    assert next(outer) == 1
    del outer
    assert closed[-1] == "delegated generator"

    def guarded():
        try:
            yield from [1, 2]
        finally:
            closed.append("outer finally")

    outer = guarded()
    assert next(outer) == 1
    del outer
    assert closed[-1] == "outer finally"

    def leave_cycle():
        back = []
        generator = suspended("cycle", back)
        back.append(generator)
        for _ in generator:
            break

    enabled = gc.isenabled()
    gc.disable()
    try:
        leave_cycle()
        assert "cycle" not in closed
        gc.collect()
        assert closed[-1] == "cycle"
    finally:
        if enabled:
            gc.enable()


check_generator_loops()
