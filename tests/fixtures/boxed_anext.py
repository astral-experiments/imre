import gc
import weakref


class Source:
    def __init__(self, result):
        self.result = result

    def __anext__(self):
        return self.result


class Exhausted(StopAsyncIteration):
    pass


class Cursor:
    def __init__(self):
        self.lookups = 0

    def __await__(self):
        self.lookups += 1
        return self

    def __iter__(self):
        return self

    def __next__(self):
        raise Exhausted()

    def send(self, *values):
        return values

    def throw(self, *args):
        return args

    def close(self):
        raise Exhausted()


class Pause:
    def __await__(self):
        return (yield "paused")


class AsyncSource:
    async def __anext__(self):
        return await Pause()


def completion(method, *args):
    try:
        method(*args)
    except StopIteration as exc:
        return exc
    raise AssertionError("expected StopIteration")


def owned_cycle():
    cursor = Cursor()
    fallback = Cursor()
    pending = anext(Source(cursor), fallback)
    cursor.pending = fallback.pending = pending
    return pending, weakref.ref(cursor), weakref.ref(fallback)


def current():
    cursor = Cursor()
    default = ("default", "tuple")
    source = Source(cursor)
    source.__anext__ = lambda: None
    assert anext(source) is cursor
    assert anext(Source(default)) is default
    pending = anext(source, default)
    owner = type(pending)
    assert owner.__name__ == "anext_awaitable"
    assert type(owner.send) is type(list.append)
    assert type(owner.__await__) is type(object.__repr__)
    assert owner.send.__objclass__ is owner
    assert owner.__await__.__get__(pending, owner)() is pending
    for invalid in (lambda: owner(), lambda: owner.__next__({})):
        try:
            invalid()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid awaitable constructor or descriptor receiver")
    assert iter(pending) is pending and pending.__await__() is pending
    assert cursor.lookups == 0
    done = completion(next, pending)
    assert done.value is default and done.args == (default,)
    value = object()
    assert pending.send(value)[0] is value
    assert pending.send((value, default)) == (value, default)
    error = ValueError("forwarded")
    assert pending.throw(error) == (error,)
    assert completion(pending.close).value is default
    assert cursor.lookups == 5

    class Raises:
        def __anext__(self):
            raise error

    class BadAwait:
        def __await__(self):
            raise exhausted

    exhausted = Exhausted()
    try:
        anext(Raises(), default)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("synchronous exception was lost")
    try:
        next(anext(Source(BadAwait()), default))
    except Exhausted as caught:
        assert caught is exhausted
    else:
        raise AssertionError("exception fetching __await__ was lost")

    suspended = anext(AsyncSource(), default)
    assert next(suspended) == "paused"
    assert completion(suspended.send, value).value is value
    pending, inner, fallback = owned_cycle()
    gc.collect()
    assert inner() is not None and fallback() is not None
    del pending
    gc.collect()
    assert inner() is None and fallback() is None
