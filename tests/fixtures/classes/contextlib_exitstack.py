import contextlib
from contextlib import AbstractContextManager, ExitStack


class Stack(ExitStack):
    pass


class Manager(AbstractContextManager[int]):
    def __enter__(self) -> int:
        return 42

    def __exit__(self, kind, value, traceback):
        return None


def make(stack: contextlib.ExitStack, cls: type[contextlib.ExitStack]) -> contextlib.AbstractContextManager[object]:
    assert isinstance(stack, ExitStack)
    return cls()


def local_imports() -> bool:
    from contextlib import AbstractContextManager as ManagerClass, ExitStack as StackClass
    import contextlib as ctx

    return ManagerClass is ctx.AbstractContextManager and StackClass is ctx.ExitStack


assert ExitStack is contextlib.ExitStack
assert AbstractContextManager is contextlib.AbstractContextManager
assert ExitStack.__bases__ == (contextlib._BaseExitStack, AbstractContextManager)
assert contextlib._BaseExitStack.__bases__ == (object,)
assert [base.__name__ for base in ExitStack.__mro__] == [
    "ExitStack", "_BaseExitStack", "AbstractContextManager", "ABC", "object"
]
assert isinstance(Stack(), AbstractContextManager)
assert issubclass(Stack, ExitStack)
assert isinstance(make(Stack(), Stack), Stack)
assert Manager().__enter__() == 42
assert local_imports()

events = []


def observe(kind, value, traceback):
    events.append(None if value is None else type(value).__name__)
    return True


def fail():
    events.append("callback")
    raise ValueError("cleanup")


with ExitStack() as stack:
    stack.push(observe)
    stack.callback(fail)
    raise KeyError("body")

assert events == ["callback", "ValueError"]
body = KeyError("body")
cleanup = ValueError("cleanup")


def raise_cleanup():
    raise cleanup


try:
    with ExitStack() as stack:
        stack.callback(raise_cleanup)
        raise body
except ValueError as exc:
    assert exc is cleanup and exc.__context__ is body
else:
    raise AssertionError("cleanup exception was lost")

print("ExitStack")
