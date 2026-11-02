from unittest import TestCase
from unittest.mock import MagicMock, Mock, call, mock_open, seal

check = TestCase()
names = ["close", "assert_worker", "_private", "__enter__", "__exit__", "__iter__", "__next__"]
handle = MagicMock(spec=names)
assert not hasattr(handle, "write") and not hasattr(handle, "__int__")
handle.assert_worker()
handle._private()
handle.extra = 42
assert handle.extra == 42
other = MagicMock(spec_set=["close"])
with check.assertRaises(AttributeError):
    other.extra = 42
assert not hasattr(other, "__enter__") and not hasattr(other, "__iter__")
with check.assertRaises(TypeError):
    iter(other)
assert handle.__enter__ is handle.__enter__
handle.__enter__.return_value = handle
with handle as entered:
    assert entered is handle
assert list(iter(handle)) == []
handle.__iter__.return_value = (1, 2)
assert list(iter(handle)) == list(iter(handle)) == [1, 2]
assert handle.__exit__.return_value is False
assert handle.method_calls == [call.assert_worker(), call._private()]
assert handle.mock_calls[:4] == [call.assert_worker(), call._private(), call.__enter__(), call.__exit__(None, None, None)]

plain = Mock(["__len__", "__custom__"])
assert not hasattr(plain, "__len__")
plain.__custom__(1)
plain.__custom__.assert_called_once_with(1)

locked = MagicMock(spec=["__iter__"])
seal(locked)
with check.assertRaises(AttributeError):
    locked.__iter__
replacement = Mock(name="replacement")
other = MagicMock(spec=["__enter__", "__iter__"])
other.__enter__ = replacement
other.__enter__()
other.reset_mock()
replacement.assert_not_called()
other.__enter__ = lambda self: self
assert other.__enter__ is other.__enter__ and other.__enter__() is other
del other.__iter__
assert not hasattr(other, "__iter__")
with check.assertRaises(TypeError):
    iter(other)

for cls in (Mock, MagicMock):
    restricted = cls(spec=["close"])
    with check.assertRaises(AttributeError):
        restricted.__len__ = lambda self: 3

plain = Mock(spec=["__enter__", "__exit__"])
sibling = Mock(spec=["__enter__", "__exit__"])
plain.__enter__ = lambda self: self
plain.__exit__ = lambda self, *exc: False
with plain as entered:
    assert entered is plain
assert not hasattr(sibling, "__enter__")

context = MagicMock(spec=["__enter__", "__exit__"])
for attempt in range(2):
    with check.assertRaises(KeyError):
        with context:
            raise KeyError(attempt)
    context.reset_mock(return_value=True)
    assert context.__exit__.return_value is False

manual = MagicMock(return_value=1)
context.__enter__ = manual
context.__exit__.side_effect = lambda *args: True
context.reset_mock(return_value=True, side_effect=True)
assert isinstance(manual.return_value, MagicMock)
assert context.__exit__.side_effect is None

for data in ("first\nsecond\n", b"first\nsecond\n"):
    opener = mock_open(read_data=data)
    with opener("file") as fp:
        assert fp is opener.return_value
        assert fp.readline() == data[:6]
        assert list(fp) == [data[6:]]
        assert fp.read() == data[:0]
    fp.close.assert_called_once_with()
    assert not hasattr(fp, "made_up") and not hasattr(opener, "__enter__")
    assert opener("another") is fp
    assert next(fp) == data[:6]
    assert fp.readlines() == [data[6:]]
    with check.assertRaises(StopIteration):
        next(fp)
    fp.read.return_value = "replacement"
    assert opener().read() == "replacement"
    fp.readline.side_effect = iter(["custom"])
    assert opener().readline() == "custom"

custom = Mock()
assert mock_open(custom, "content") is custom
with check.assertRaises(KeyError):
    with custom("failed") as fp:
        raise KeyError("failed")
fp.close.assert_called_once_with()
print("mock_open checked")
