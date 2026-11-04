from unittest import TestCase, mock
from unittest.mock import ANY, Mock, call

assert mock.call is call
assert call._format("payload") == ("_format", ("payload",), {})
assert repr(call.fetch()._format("payload")) == "call.fetch()._format('payload')"

parent = Mock()
child = Mock(name="worker", parent=parent, spec=["send"])
children = child._mock_children
child(1)
child.send(2)
assert parent.mock_calls == [call(1), call.send(2)]
assert parent.method_calls == [call.worker(1), call.worker.send(2)]
assert "worker" not in parent._mock_children
child.reset_mock()
assert child._mock_parent is parent
assert child._mock_children is children and children["send"] is child.send
assert parent.mock_calls == [call(1), call.send(2)]

client = Mock()
client.fetch(1).read(size=2)
client.fetch(3)
expected = call.fetch(1).read(size=ANY)
assert client.mock_calls == [*expected.call_list(), call.fetch(3)]
assert client.method_calls == [call.fetch(1), call.fetch(3)]
assert client.fetch.call_args_list == [call(1), call(3)]
assert client.fetch.return_value.method_calls == [call.read(size=2)]
assert client.mock_calls[1].args == ()
assert client.mock_calls[1].kwargs == {"size": 2}
assert repr(client.mock_calls[1]) == "call.fetch().read(size=2)"

client.assert_has_calls(expected.call_list())
client.assert_has_calls([call.fetch(3), call.fetch(1)], any_order=True)
for sequence, unordered in (
    ([call.fetch(1), call.fetch(3)], False),
    ([call.fetch(1), call.fetch(1)], True),
    ([call.fetch().write(size=2)], False),
    ([call.fetch().read(size=1)], False),
):
    try:
        client.assert_has_calls(sequence, any_order=unordered)
    except AssertionError:
        pass
    else:
        raise AssertionError(f"wrong call sequence matched: {sequence!r}")

assert call.fetch(1).read() != call.fetch(2).read()
unusual = Mock()
getattr(unusual.branch, "").leaf(1)
getattr(unusual.branch, "()")(2)
unusual.branch()(3)
assert [item[0] for item in unusual.mock_calls] == ["branch.leaf", "branch()", "branch", "branch()"]
assert [item[0] for item in unusual.method_calls] == ["branch..leaf", "branch.()", "branch"]

history = client.mock_calls
client.reset_mock()
assert len(history) == 3 and client.mock_calls == []


class HintedHistory:
    def __init__(self, items, on_length):
        self.items = items
        self.on_length = on_length

    def __iter__(self):
        return iter(self.items)

    def __len__(self):
        self.on_length()
        return len(self.items)


unused = Mock()
check = TestCase()
with check.assertRaises(AssertionError):
    unused.assert_has_calls(HintedHistory([call(7)], lambda: unused(7)))

history_items = []
unused.call_args_list = HintedHistory(history_items, lambda: history_items.append(((7,), {})))
with check.assertRaises(AssertionError):
    unused.assert_any_call(7)

integer = mock.MagicMock()
assert int(integer) == 1
integer.__int__.return_value = 7
assert int(integer) == 7
assert integer.mock_calls == [call.__int__(), call.__int__()]
assert integer.method_calls == []
integer.reset_mock(return_value=True)
assert integer.mock_calls == []
assert int(integer) == 7
with check.assertRaises(TypeError):
    int(mock.MagicMock(spec_set=object))

key = mock.MagicMock()
assert hash(key) == object.__hash__(key)
key.__hash__.return_value = 17
assert hash(key) == 17
assert key.mock_calls == [call.__hash__(), call.__hash__()]
assert key.method_calls == []
key.__hash__.return_value = "invalid"
with check.assertRaises(TypeError):
    hash(key)
assert key.__hash__.call_count == 3
