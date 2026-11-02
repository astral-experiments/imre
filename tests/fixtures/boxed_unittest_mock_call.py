from unittest import mock
from unittest.mock import ANY, Mock, call

assert mock.call is call

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
history = client.mock_calls
client.reset_mock()
assert len(history) == 3 and client.mock_calls == []
