from contextlib import AbstractAsyncContextManager, AsyncExitStack, ExitStack
from types import MethodType


class Pause:
    def __await__(self):
        return (yield "paused")


class Manager:
    async def __aenter__(self):
        return "resource"

    async def __aexit__(self, kind, error, tb):
        assert kind is ValueError and error is failure and tb is not None
        events.append("exit")
        return True


class Stack(AsyncExitStack):
    pass


events = []
failure = ValueError("close")


async def close(label, *, extra):
    events.append(label)
    resumed = await Pause()
    assert resumed == extra
    raise failure


async def verify():
    stack = Stack()
    assert isinstance(stack, AbstractAsyncContextManager)
    assert not hasattr(stack, "__enter__")
    assert AsyncExitStack.__module__ == "contextlib"
    entered = await stack.__aenter__()
    assert entered is stack
    manager = Manager()
    manager.__aenter__ = None
    manager.__aexit__ = None
    resource = await stack.enter_async_context(manager)
    assert resource == "resource"
    is_sync, bound = stack._exit_callbacks[-1]
    assert is_sync is False and type(bound) is MethodType
    assert bound.__self__ is manager and bound.__func__ is Manager.__aexit__
    assert stack.push_async_callback(close, "async", extra=17) is close
    wrapper = stack._exit_callbacks[-1][1]
    assert wrapper.__wrapped__ is close and wrapper.__name__ == "_exit_wrapper"
    stack.callback(events.append, "sync")
    moved = stack.pop_all()
    assert type(moved) is Stack and not stack._exit_callbacks
    await stack.aclose()
    async with moved:
        raise RuntimeError("body")
    assert events == ["sync", "async", "exit"]
    assert isinstance(failure.__context__, RuntimeError)

    async with Stack() as empty:
        try:
            await empty.enter_async_context(ExitStack())
        except TypeError as error:
            assert "asynchronous context manager" in str(error)
        else:
            raise AssertionError("accepted a synchronous manager")
        assert not empty._exit_callbacks


coro = verify()
assert coro.send(None) == "paused"
try:
    coro.send(17)
except StopIteration as done:
    assert done.value is None
else:
    raise AssertionError("cleanup did not finish")
