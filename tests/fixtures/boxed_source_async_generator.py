import gc
import sys
import warnings
import weakref


def finish(operation, value=None):
    try:
        operation.send(value)
    except StopIteration as done:
        return done.value
    raise AssertionError("operation suspended")


def current():
    scope = {"events": []}
    exec("""\
class Gate:
    def __await__(self):
        reply = yield "suspended"
        return reply

class End(StopAsyncIteration):
    pass

class Stop(StopIteration):
    pass

async def called(target):
    value = target("left", await Gate())
    yield value

async def consume(factory):
    async for value in factory():
        return value

async def stream(required, /, *, option=2):
    events.append((required, option))
    try:
        sent = yield required
        reply = await Gate()
        yield (sent, reply)
    finally:
        events.append("closed")

async def thrower(suspend=False):
    try:
        yield "ready"
    except KeyError as exc:
        if suspend:
            await Gate()
        yield exc
    yield "after handler"

async def reentrant(holder):
    gen = holder[0]
    for op in (gen.aclose(), gen.athrow(KeyError()), gen.asend(None)):
        try:
            op.send(None)
        except RuntimeError as exc:
            assert "already running" in str(exc)
        else:
            raise AssertionError("another operation entered the executing frame")
    yield "still running"

async def cleanup(invalid):
    try:
        yield "open"
    finally:
        if invalid:
            yield "unexpected item"
        else:
            events.append("closing")
            reply = await Gate()
            events.append(reply)

async def broken(error):
    yield None
    raise error
""", scope)
    def unavailable():
        raise LookupError("missing iterable")

    try:
        scope["consume"](unavailable).send(None)
    except LookupError as exc:
        assert exc.args == ("missing iterable",)
    else:
        raise AssertionError("async for did not evaluate its operand")
    gen = scope["called"](lambda left, right: (left, right))
    assert type(gen).__aiter__(gen) is gen
    for method, args in ((type(gen).__anext__, ()), (type(gen).asend, (gen,)),
                         (type(gen).aclose, (None,))):
        try:
            method(*args)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid async-generator descriptor arguments accepted")
    operation = type(gen).__anext__(gen)
    assert type(operation).send(operation, None) == "suspended"
    assert finish(operation, "arrived") == ("left", "arrived")
    assert finish(gen.aclose()) is None

    stream = scope["stream"]
    events = scope["events"]
    try:
        stream(required=1)
    except TypeError:
        pass
    else:
        raise AssertionError("positional-only parameter accepted as keyword")

    gen = stream("first")
    assert type(gen).__name__ == "async_generator" and gen.__aiter__() is gen
    assert events == []
    first = gen.__anext__()
    assert finish(first) == "first"
    assert events == [("first", 2)]
    try:
        first.send(None)
    except RuntimeError as exc:
        assert "cannot reuse" in str(exc)
    else:
        raise AssertionError("used __anext__ operation could be reused")

    send = gen.asend("sent")
    assert send.send(None) == "suspended"
    try:
        gen.__anext__().send(None)
    except RuntimeError as exc:
        assert "already running" in str(exc)
    else:
        raise AssertionError("overlapping __anext__ operation advanced generator")
    assert finish(send, "reply") == ("sent", "reply")
    assert finish(gen.aclose()) is None and events == [("first", 2), "closed"]
    try:
        gen.__anext__().send(None)
    except StopAsyncIteration:
        pass
    else:
        raise AssertionError("closed generator produced an item")

    gen = scope["thrower"]()
    assert finish(gen.__anext__()) == "ready"
    thrown = KeyError("injected")
    assert finish(gen.athrow(thrown)) is thrown
    assert finish(gen.aclose()) is None

    holder = []
    gen = scope["reentrant"](holder)
    holder.append(gen)
    assert finish(gen.__anext__()) == "still running"
    assert finish(gen.aclose()) is None

    # An exception injected before the first instruction escapes as the same instance.
    for kind in (StopIteration, scope["Stop"]):
        raised = kind("before body")
        for stored in (True, False):
            gen = stream("never entered")
            try:
                if stored:
                    gen.athrow(raised).send(None)
                else:
                    gen.__anext__().throw(raised)
            except StopIteration as exc:
                assert exc is raised
            else:
                raise AssertionError("initial exception was lost")
            assert finish(gen.aclose()) is None
    assert events == [("first", 2), "closed"]

    gen = scope["thrower"]()
    assert finish(gen.__anext__()) == "ready"
    assert finish(gen.__anext__()) == "after handler"
    try:
        gen.__anext__().send(None)
    except StopAsyncIteration:
        pass
    else:
        raise AssertionError("expected natural exhaustion")
    try:
        gen.athrow(raised).send(None)
    except StopIteration as exc:
        assert type(exc) is StopIteration and exc.args == ()
    else:
        raise AssertionError("athrow on an exhausted generator resumed")

    gen = scope["thrower"](True)
    assert finish(gen.__anext__()) == "ready"
    throw = gen.athrow(thrown)
    assert throw.send(None) == "suspended"
    assert finish(throw) is thrown
    # When athrow resumes a suspended await, its send delegates to the generator again.
    assert finish(throw) == "after handler"
    try:
        throw.send(None)
    except StopAsyncIteration:
        pass
    else:
        raise AssertionError("resumed athrow missed natural exhaustion")
    assert finish(gen.aclose()) is None

    for args, message in (
        ((), "athrow expected at least 1 argument, got 0"),
        ((KeyError, None, None, None), "athrow expected at most 3 arguments, got 4"),
    ):
        gen = scope["thrower"]()
        assert finish(gen.__anext__()) == "ready"
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            throw = gen.athrow(*args)
        try:
            throw.send(None)
        except TypeError as exc:
            assert exc.args == (message,)
        else:
            raise AssertionError("invalid athrow arguments accepted")
        try:
            gen.__anext__().send(None)
        except RuntimeError as exc:
            assert "already running" in str(exc)
        else:
            raise AssertionError("another read took ownership after invalid athrow")
        assert finish(throw) == "after handler"
        assert finish(gen.aclose()) is None

    gen = scope["cleanup"](False)
    assert finish(gen.__anext__()) == "open"
    close = gen.aclose()
    assert close.send(None) == "suspended"
    assert events[-1] == "closing"
    try:
        gen.__anext__().send(None)
    except RuntimeError as exc:
        assert "already running" in str(exc)
    else:
        raise AssertionError("read accepted while aclose was awaiting cleanup")
    assert finish(close, "cleanup resumed") is None
    assert events[-2:] == ["closing", "cleanup resumed"]

    gen = scope["cleanup"](True)
    assert finish(gen.__anext__()) == "open"
    try:
        gen.aclose().send(None)
    except RuntimeError as exc:
        assert exc.args == ("async generator ignored GeneratorExit",)
    else:
        raise AssertionError("aclose accepted an item yielded during cleanup")
    try:
        gen.__anext__().send(None)
    except GeneratorExit:
        pass
    else:
        raise AssertionError("pending GeneratorExit lost after yield in cleanup")
    assert finish(gen.aclose()) is None

    for name, parent in (("Stop", "StopIteration"), ("End", "StopAsyncIteration")):
        raised = scope[name]("inner")
        gen = scope["broken"](raised)
        assert finish(gen.__anext__()) is None
        try:
            gen.__anext__().send(None)
        except RuntimeError as exc:
            assert exc.args == ("async generator raised " + parent,)
            assert exc.__cause__ is exc.__context__ is raised
        else:
            raise AssertionError("user exception mistaken for normal exhaustion")

    hooks = []
    def firstiter(gen):
        hooks.append("first")

    def finalizer(gen):
        hooks.append("final")
        assert finish(gen.aclose()) is None

    old_hooks = sys.get_asyncgen_hooks()
    try:
        sys.set_asyncgen_hooks(firstiter, finalizer)
        gen = stream("hooked")
        with warnings.catch_warnings():
            warnings.simplefilter("error", DeprecationWarning)
            try:
                gen.athrow(KeyError, None)
            except DeprecationWarning as exc:
                assert "the (type, exc, tb) signature of athrow() is deprecated" in str(exc)
            else:
                raise AssertionError("athrow suppressed its deprecation warning")
        assert hooks == []
        ref = weakref.ref(gen)
        operation = gen.__anext__()
        try:
            weakref.ref(operation)
        except TypeError:
            pass
        else:
            raise AssertionError("asend supported weak references")
        assert hooks == ["first"]
        assert finish(operation) == "hooked"
        sys.set_asyncgen_hooks(None, None)
        del gen
        assert ref() is not None
        del operation
        gc.collect()
        assert ref() is None
        assert hooks == ["first", "final"] and events[-1] == "closed"
    finally:
        sys.set_asyncgen_hooks(*old_hooks)
    print("source async generator protocol")
