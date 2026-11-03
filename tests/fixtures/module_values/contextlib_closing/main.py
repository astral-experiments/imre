import contextlib
import types
from contextlib import aclosing, closing


class Resource:
    def __init__(self, error=None):
        self.calls = 0
        self.error = error

    def close(self):
        self.calls += 1
        if self.error is not None:
            raise self.error
        return True


class Managed(closing):
    pass


assert closing is contextlib.closing
resource = Resource()
manager = Managed(thing=resource)
assert isinstance(manager, contextlib.AbstractContextManager)
with manager as target:
    assert target is resource
    assert resource.calls == 0
assert resource.calls == 1

body_error = RuntimeError("body")
try:
    with manager:
        raise body_error
except RuntimeError as caught:
    assert caught is body_error
else:
    raise AssertionError("closing suppressed the body exception")
assert resource.calls == 2
assert manager.__exit__(None, None, None) is None
assert resource.calls == 3

close_error = ValueError("close")
replacement = Resource(close_error)
manager.thing = replacement
try:
    with manager as target:
        assert target is replacement
        raise body_error
except ValueError as caught:
    assert caught is close_error
    assert caught.__context__ is body_error
else:
    raise AssertionError("close exception did not propagate")
assert replacement.calls == 1 and resource.calls == 3

alias = Managed[int]
assert type(alias) is types.GenericAlias
assert alias.__origin__ is Managed and alias.__args__ == (int,)
assert closing[str].__origin__ is closing

assert aclosing is contextlib.aclosing
assert aclosing[int].__origin__ is aclosing
