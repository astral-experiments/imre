import gc
import weakref
from collections.abc import AsyncGenerator, AsyncIterator, Awaitable, Iterator
from inspect import (
    AGEN_CLOSED,
    AGEN_CREATED,
    AGEN_RUNNING,
    AGEN_SUSPENDED,
    getasyncgenlocals,
    getasyncgenstate,
)


class Pause:
    def __await__(self):
        return (yield "paused")


def make_stream(events):
    async def stream(value):
        events.append("started")
        try:
            sent = yield value
            reply = await Pause()
            yield sent, reply
        finally:
            events.append("closed")
    return stream


def finish(operation, value=None):
    try:
        operation.send(value)
    except StopIteration as done:
        return done.value
    raise AssertionError("operation did not complete")


def current():
    class Structural:
        def __aiter__(self): return self
        async def __anext__(self): raise StopAsyncIteration
        async def asend(self, value): raise StopAsyncIteration
        async def athrow(self, error): raise error
        async def aclose(self): pass

    class MissingSend(Structural):
        asend = None

    assert issubclass(Structural, AsyncGenerator)
    assert not issubclass(MissingSend, AsyncGenerator)
    events = []
    stream = make_stream(events)
    gen = stream("first")
    assert getasyncgenstate(gen) == AGEN_CREATED
    frame = gen.ag_frame
    assert frame.f_code is gen.ag_code
    bindings = getasyncgenlocals(gen)
    assert bindings == {"value": "first", "events": events}
    bindings["value"] = "changed"
    assert isinstance(gen, AsyncIterator) and issubclass(type(gen), AsyncGenerator)
    assert isinstance(gen, AsyncGenerator) and not isinstance(gen, (Awaitable, Iterator))
    assert gen.__aiter__() is gen and events == []
    assert finish(anext(gen)) == "changed" and events == ["started"]
    assert getasyncgenstate(gen) == AGEN_SUSPENDED and gen.ag_frame is frame
    pending = gen.asend("sent")
    assert isinstance(pending, Awaitable) and pending.send(None) == "paused"
    assert gen.ag_suspended and getasyncgenstate(gen) == AGEN_RUNNING
    try:
        gen.aclose().send(None)
    except RuntimeError as exc:
        assert "already running" in str(exc)
    else:
        raise AssertionError("another operation entered the suspended generator")
    assert bindings["sent"] == "sent" and "reply" not in bindings
    bindings["sent"] = "edited"
    assert finish(pending, "reply") == ("edited", "reply")

    owner = weakref.ref(gen)
    close = gen.aclose()
    del gen, pending
    gc.collect()
    assert owner() is not None
    assert finish(close) is None and events == ["started", "closed"]
    assert owner().ag_frame is None and getasyncgenstate(owner()) == AGEN_CLOSED
    assert getasyncgenlocals(owner()) == {} and bindings["reply"] == "reply"
    del close
    gc.collect()
    assert owner() is None
    print("linked async generator protocol")
