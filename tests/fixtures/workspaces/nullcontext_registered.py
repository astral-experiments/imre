import contextlib
from contextlib import AbstractAsyncContextManager, AbstractContextManager, nullcontext
from tempfile import TemporaryDirectory


class Child(nullcontext[int]):
    pass


def local_class():
    from contextlib import nullcontext as Local
    return Local


def typed(manager: nullcontext[int], cls: type[nullcontext[int]]) -> int:
    with cls(manager.enter_result) as entered:
        return entered


assert nullcontext is contextlib.nullcontext is local_class()
assert nullcontext.__bases__ == (AbstractContextManager, AbstractAsyncContextManager)
assert issubclass(Child, nullcontext)
assert isinstance(Child(1), AbstractContextManager)
assert isinstance(Child(1), AbstractAsyncContextManager)
assert typed(Child(42), Child) == 42
try:
    nullcontext(1, 2)
except TypeError:
    pass
else:
    raise AssertionError("nullcontext accepted two arguments")

payload = object()
with nullcontext(enter_result=payload) as entered:
    assert entered is payload
try:
    with nullcontext(1):
        raise ValueError("body")
except ValueError as exc:
    assert str(exc) == "body"
else:
    raise AssertionError("nullcontext suppressed the body error")


async def asynchronous():
    async with nullcontext(23) as entered:
        return entered


try:
    asynchronous().send(None)
except StopIteration as result:
    assert result.value == 23
else:
    raise AssertionError("coroutine did not finish")


def choose(cached: str | None):
    manager = TemporaryDirectory(dir="scratch") if cached is None else nullcontext(cached)
    with manager as name:
        assert isinstance(name, str)
        return name


assert choose("cached") == "cached"
assert choose(None) != "cached"
print("nullcontext")
