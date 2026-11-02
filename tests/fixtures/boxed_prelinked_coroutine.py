from __future__ import annotations

import inspect
from abc import get_cache_token
from collections.abc import Awaitable, Coroutine, Generator
from types import CoroutineType


class Manager:
    def __init__(self):
        self.events = []

    async def __aenter__(self) -> int:
        def answer():
            return 42

        assert answer.__qualname__ == "Manager.__aenter__.<locals>.answer"
        self.events.append("enter")
        return answer()

    async def __aexit__(self, etype, value, tb):
        assert etype is value is tb is None
        self.events.append("exit")


async def read_value(manager) -> int:
    def identity(value: Missing):
        return value

    assert identity.__annotations__ == {"value": "Missing"}
    async with manager as value:
        return identity(value)


manager = Manager()
pending = read_value(manager)
assert inspect.iscoroutinefunction(read_value)
assert type(pending) is CoroutineType
assert Awaitable.__subclasshook__(CoroutineType) is True
assert Coroutine.__subclasshook__(CoroutineType) is True
assert isinstance(pending, Coroutine) and isinstance(pending, Awaitable)
token = get_cache_token()
assert Coroutine.register(CoroutineType) is CoroutineType
assert get_cache_token() == token

wrapper = pending.__await__()
assert isinstance(wrapper, Generator)
assert not isinstance(wrapper, Coroutine) and not isinstance(wrapper, Awaitable)
assert pending.cr_code is read_value.__code__
assert manager.events == []
try:
    pending.send(None)
except StopIteration as done:
    assert done.value == 42
else:
    raise AssertionError("async with did not complete on the first send")
assert manager.events == ["enter", "exit"]
