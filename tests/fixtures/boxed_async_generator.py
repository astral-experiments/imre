from types import AsyncGeneratorType
from collections.abc import AsyncGenerator, Awaitable

class Pause:
    def __await__(self):
        value = yield "paused"
        return value

async def source(log):
    log.append("open")
    try:
        sent = yield ("first", 1)
        log.append(sent)
        awaited = await Pause()
        yield awaited
        try:
            yield 3
        except LookupError as exc:
            yield exc.args
    finally:
        log.append("closing")
        await Pause()
        log.append("closed")

def raised(kind, call, *args):
    try:
        call(*args)
    except kind as exc:
        return exc
    raise AssertionError((kind, args))

async def invalid(kind):
    yield 1
    raise kind("escaped")


def current():
    log = []
    g = source(log)
    assert type(g) is AsyncGeneratorType and isinstance(g, AsyncGenerator)
    assert not isinstance(g, Awaitable)
    assert g.__aiter__() is g and not g.ag_running and log == []
    assert raised(TypeError, iter, g)
    assert g.__name__ == "source"
    g.__qualname__ = "renamed"
    assert g.__qualname__ == "renamed" and g.ag_code.co_name == "source"
    frame = g.ag_frame
    assert frame.f_code is g.ag_code and g.ag_await is None and not g.ag_suspended
    first = g.__anext__()
    assert iter(first) is first and first.__await__() is first
    assert raised(StopIteration, next, first).value == ("first", 1)
    assert raised(RuntimeError, next, first)
    step = g.asend("sent")
    assert next(step) == "paused" and g.ag_running
    assert g.ag_frame is frame and g.ag_await is not None and g.ag_suspended
    assert log == ["open", "sent"]
    for pending in (g.__anext__(), g.athrow(ValueError()), g.aclose()):
        assert "already running" in str(raised(RuntimeError, next, pending))
    assert raised(StopIteration, step.send, 2).value == 2
    assert not g.ag_running and raised(StopIteration, next, anext(g)).value == 3
    assert raised(StopIteration, next, g.athrow(LookupError("caught"))).value == ("caught",)
    closed = g.aclose()
    assert next(closed) == "paused" and g.ag_running and log[-1] == "closing"
    assert raised(StopIteration, closed.send, None).value is None
    assert not g.ag_running and log[-1] == "closed"
    assert g.ag_frame is None and g.ag_await is None and not g.ag_suspended
    assert raised(StopAsyncIteration, next, g.__anext__())
    assert raised(StopIteration, next, g.athrow(ValueError())).value is None
    assert raised(StopIteration, next, g.aclose()).value is None

    for kind in (StopIteration, StopAsyncIteration):
        cold = invalid(kind)
        injected = kind("cold")
        assert raised(kind, next, cold.athrow(injected)) is injected
        g = invalid(kind)
        assert raised(TypeError, next, g.asend("cold"))
        assert raised(StopIteration, next, g.__anext__()).value == 1
        exc = raised(RuntimeError, next, g.__anext__())
        assert type(exc.__cause__) is kind
        assert str(exc) == "async generator raised " + kind.__name__

    check_await_delegate()
    check_awaited_argument()
    check_hooks()
    check_unawaited()
    print("async-generator protocol passed")


def check_hooks():
    import gc
    import sys

    events = []
    saved = []

    def first(gen):
        events.append(("first", gen.ag_code.co_name))

    def final(gen):
        events.append(("final", gen.ag_code.co_name))
        saved.append(gen)

    async def ticker():
        try:
            yield 1
        finally:
            events.append("closed")

    previous = sys.get_asyncgen_hooks()
    try:
        sys.set_asyncgen_hooks(firstiter=first, finalizer=final)
        gen = ticker()
        assert events == []
        operation = gen.__anext__()
        assert events == [("first", "ticker")]
        assert raised(StopIteration, next, operation).value == 1
        sys.set_asyncgen_hooks(firstiter=None, finalizer=None)
        del operation, gen
        gc.collect()
        assert events == [("first", "ticker"), ("final", "ticker")]
        assert len(saved) == 1
        assert raised(StopIteration, next, saved.pop().aclose()).value is None
        assert events[-1] == "closed"
    finally:
        sys.set_asyncgen_hooks(*previous)


def check_await_delegate():
    async def inner():
        await Pause()

    class Bridge:
        def __init__(self, coroutine):
            self.it = coroutine.__await__()

        def __await__(self):
            return self.it

    async def outer(awaitable):
        await awaitable
        yield 1

    nested = inner()
    explicit = Bridge(inner())
    for item, delegate in ((nested, nested), (explicit, explicit.it)):
        gen = outer(item)
        op = gen.__anext__()
        assert next(op) == "paused" and gen.ag_await is delegate
        assert op.close() is None


def check_unawaited():
    import gc
    import warnings

    gen = invalid(StopIteration)
    gen.__qualname__ = "probe"
    with warnings.catch_warnings(record=True) as records:
        warnings.simplefilter("always")
        operations = ((gen.__anext__, ()), (gen.asend, (None,)),
                      (gen.athrow, (ValueError(),)), (gen.aclose, ()))
        for method, args in operations:
            operation = method(*args)
            del operation
            gc.collect()
        assert len(records) == 4
        names = ("asend", "asend", "athrow", "aclose")
        for record, name in zip(records, names):
            assert record.category is RuntimeWarning and record.source is gen
            assert str(record.message) == "coroutine method '" + name + "' of 'probe' was never awaited"
    assert raised(StopIteration, next, gen.aclose()).value is None


def check_awaited_argument():
    async def double(value):
        return value * 2

    async def stream():
        result = await double(await Pause())
        yield result

    gen = stream()
    op = gen.__anext__()
    assert next(op) == "paused"
    assert raised(StopIteration, op.send, 21).value == 42
    assert raised(StopAsyncIteration, next, gen.__anext__())
