from unittest import TestCase
from unittest.mock import InvalidSpecError, MagicMock, Mock, PropertyMock, call, mock_open, patch, seal

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

class Readable:
    def read(self): pass


class Reader(Readable):
    def __iter__(self): return self
    def __next__(self): raise StopIteration


original = Readable.read
property_patch = patch("__main__.Readable.read", new_callable=PropertyMock, return_value=7)
recorded = property_patch.start()
try:
    reader = Readable()
    assert reader.read == 7
    reader.read = 9
    assert recorded.mock_calls == [call(), call(9)]
finally:
    property_patch.stop()
assert Readable.read is original and property_patch.stop() is None

factory_patch = patch.object(Readable, "read", new_callable=dict, contents=7)
with factory_patch as replacement:
    assert replacement == {"contents": 7}
    assert factory_patch.stop() is None and Readable.read is replacement
assert Readable.read is original
with check.assertRaises(ValueError):
    patch.object(Readable, "read", None, new_callable=PropertyMock)

specified = MagicMock(Reader)
assert specified.__class__ is Reader
assert isinstance(specified, Reader) and isinstance(specified, Readable)
assert not hasattr(specified, "__enter__") and not hasattr(specified, "write")
specified.extra = 42
specified.read(3)
specified.read.assert_called_once_with(3)
specified.__iter__ = lambda self: self
specified.__next__.return_value = 7
assert iter(specified) is specified and next(specified) == 7
specified.__next__.assert_called_once_with()

strict = Mock(spec_set=Reader)
assert strict.__class__ is Reader
strict.read = 3
with check.assertRaises(AttributeError):
    strict.write = 4
assert handle.__class__ is type(handle)

inner = Mock()
inner.__class__ = type
with check.assertRaises(InvalidSpecError):
    Mock(spec=inner)
with check.assertRaises(InvalidSpecError):
    MagicMock(spec_set=inner)


class ReportedMock:
    @property
    def __class__(self): return Mock


reported = ReportedMock()
specified.return_value = reported
specified.reset_mock()
assert specified.return_value is reported

specified.__init__(["read"])
assert specified.__class__ is type(specified)
assert not hasattr(specified, "__iter__") and not hasattr(specified, "__next__")
with check.assertRaises(TypeError):
    iter(specified)

plain = Mock(["__len__", "__custom__"])
assert not hasattr(plain, "__len__")
plain.__custom__(1)
plain.__custom__.assert_called_once_with(1)

items = MagicMock()
items["key"].value = 42
seal(items)
assert items[1:3].value == 42
items.__getitem__.assert_called_with(slice(1, 3))
assert not hasattr(items["key"], "missing")
assert not hasattr(other, "__getitem__")

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
