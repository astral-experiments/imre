from unittest import TestCase
from unittest.mock import CallableMixin, DEFAULT, Mock, NonCallableMock

class Composed(CallableMixin, NonCallableMock):
    pass


assert Mock.__call__ is CallableMixin.__call__
assert not issubclass(NonCallableMock, CallableMixin)
composed = Composed(side_effect=[7])
assert composed(3) == 7
composed.assert_called_once_with(3)
assert isinstance(composed.child, Composed) and isinstance(composed.return_value, Composed)

check = TestCase()
error = ValueError("from iterator")
mock = Mock(return_value="fallback", side_effect=[1, DEFAULT, error, KeyError])
assert mock() == 1
assert mock() == "fallback"
with check.assertRaises(ValueError) as raised:
    mock("error")
assert raised.exception is error
mock.assert_called_with("error")
with check.assertRaises(KeyError):
    mock()
with check.assertRaises(StopIteration):
    mock(phase="done")
mock.assert_called_with(phase="done")
with check.assertRaises(AssertionError):
    mock.assert_called_with(phase="earlier")
assert mock.call_count == 5

effects = iter([error])
mock.side_effect = effects
assert mock.side_effect is effects
mock.side_effect = lambda: next(effects)
assert mock() is error
mock.side_effect = 42
assert mock.side_effect == 42
with check.assertRaises(TypeError):
    mock()

class Values:
    __call__ = None

    def __iter__(self):
        return iter(["from iterable"])


mock.side_effect = Values()
assert mock() == "from iterable"
with check.assertRaises(StopIteration):
    mock()
mock.side_effect = str.upper
assert mock("item") == "ITEM"
mock.side_effect = len
assert mock([1, 2]) == 2
items = []
mock.side_effect = items.append
assert mock("item") is None and items == ["item"]
deliveries = []

def deliver(message):
    deliveries.append(message)
    if message == "lost":
        raise error
    return "sent:" + message

spy = Mock(wraps=deliver, side_effect=[DEFAULT, "override", DEFAULT])
assert spy.return_value is DEFAULT
assert spy("first") == "sent:first"
spy.return_value = None
assert spy("second") == "override" and spy("third") is None
assert deliveries == ["first"]
spy.side_effect = None
spy.return_value = DEFAULT
with check.assertRaises(ValueError) as raised:
    spy("lost")
assert raised.exception is error and deliveries == ["first", "lost"]
spy.assert_called_with("lost")
print("side effects checked")
