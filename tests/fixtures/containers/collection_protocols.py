from collections import ChainMap, OrderedDict
from collections.abc import (
    AsyncGenerator, AsyncIterable, AsyncIterator, Awaitable, Buffer, Coroutine,
    Generator, ItemsView, KeysView, Reversible, ValuesView,
)


class Minimal(Coroutine):
    def __await__(self):
        yield

    def send(self, value):
        return value

    def throw(self, *args):
        return super().throw(*args)


class IgnoresClose(Minimal):
    def throw(self, typ):
        pass


class Stream(AsyncIterator):
    async def __anext__(self):
        raise StopAsyncIteration


class Exports:
    def __buffer__(self, flags):
        return memoryview(b"x")


class NoBuffer(Exports):
    __buffer__ = None


class Virtual:
    pass


mapping = ChainMap({"a": 1})
for view, cls, builtin in ((mapping.keys(), KeysView, {}.keys()),
                           (mapping.values(), ValuesView, {}.values()),
                           (mapping.items(), ItemsView, {}.items())):
    assert type(view) is cls and isinstance(builtin, cls)
    assert list(view) == list(cls(mapping))

keys = mapping.keys()
old = keys | {"c"}
mapping["b"] = 2
assert len(keys) == 2 and old == {"a", "c"}
assert mapping.items() & {("a", 1)} == {("a", 1)}
assert isinstance(OrderedDict().values(), Reversible)
assert not isinstance(iter([]), Reversible)

assert Coroutine.__bases__ == (Awaitable,)
assert Coroutine.__abstractmethods__ == frozenset({"__await__", "send", "throw"})
assert Coroutine.register(Virtual) is Virtual and isinstance(Virtual(), Awaitable)
assert Minimal().close() is None
try:
    IgnoresClose().close()
except RuntimeError as exc:
    assert str(exc) == "coroutine ignored GeneratorExit"
else:
    raise AssertionError("close() returned after throw() ignored GeneratorExit")
assert isinstance(Stream(), AsyncIterable)
assert Stream.__abstractmethods__ == frozenset()
assert AsyncGenerator.__abstractmethods__ == frozenset({"asend", "athrow"})
stream = Stream()
assert stream.__aiter__() is stream
assert isinstance((i for i in range(1)), Generator)
assert Buffer.__abstractmethods__ == frozenset({"__buffer__"})
assert isinstance(b"x", Buffer) and isinstance(Exports(), Buffer)
assert not issubclass(NoBuffer, Buffer)
print("ok")
