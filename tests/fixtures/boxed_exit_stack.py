from contextlib import AbstractContextManager, ExitStack, nullcontext
from types import MethodType

events = []

with nullcontext(enter_result=events) as value:
    assert value is events
with nullcontext() as value:
    assert value is None


class Exit:
    def __call__(self, resource, kind, exc, tb):
        events.append((resource.name, None if exc is None else kind.__name__))
        return resource.suppress


class Resource:
    __exit__ = Exit()

    def __init__(self, name, suppress):
        self.name = name
        self.suppress = suppress

    def __enter__(self):
        return self


def cleanup(value):
    events.append(value)


stack = ExitStack()
first = Resource("first", False)
second = Resource("second", True)
assert stack.enter_context(first) is first
bound = stack._exit_callbacks[-1][1]
assert type(bound) is MethodType and bound.__self__ is first and bound.__func__ is Resource.__exit__
assert stack.callback(cleanup, "between") is cleanup
assert stack.enter_context(second) is second
with stack.pop_all():
    raise KeyError("body")
stack.close()
assert events == [("second", "KeyError"), "between", ("first", None)]

try:
    AbstractContextManager()
except TypeError:
    pass
else:
    raise AssertionError("abstract base instantiated")

class Child(AbstractContextManager):
    def __exit__(self, kind, exc, tb):
        return None

child = Child()
assert child.__enter__() is child
assert issubclass(Resource, AbstractContextManager)

class Disabled(Resource):
    __exit__ = None

assert not issubclass(Disabled, AbstractContextManager)

inner = ValueError("cleanup")
outer = KeyError("body")

def raise_inner():
    raise inner

try:
    with ExitStack() as stack:
        stack.callback(raise_inner)
        raise outer
except ValueError as exc:
    assert exc is inner and exc.__context__ is outer
print(events)
