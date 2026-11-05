from unittest import TestCase
from unittest.mock import Mock, call, create_autospec


def send(self, payload=b"default", *, urgent=False):
    pass


def old_spec(payload):
    pass


def rejected_spec(new_name):
    pass


class BrokenWrapper:
    @property
    def __wrapped__(self):
        raise RuntimeError("cannot unwrap")


def codec(data):
    return bytes(data)


def make_encoder():
    def register_codec():
        encode.codec = codec
        return bytes

    def encode(payload: register_codec()):
        pass

    return encode


check = TestCase()
child = Mock(spec_set=send, parent=Mock())
child()
child.assert_called_with()
child.assert_called_once_with()
child.assert_any_call()
with check.assertRaises(AssertionError):
    child.assert_called_with(payload=b"default")
child(b"data", urgent=True)
child.assert_called_with(payload=b"data", urgent=True)
child.assert_has_calls([call(), call(payload=b"data", urgent=True)])

writer = Mock(spec=old_spec)
alias = writer
data = object()
writer(data)
history = alias.call_args_list
rejected_spec.wrapper = BrokenWrapper()
with check.assertRaisesRegex(RuntimeError, "cannot unwrap"):
    alias.mock_add_spec(rejected_spec)
writer.assert_called_once_with(payload=data)
assert writer.call_args_list is history
assert history[0].args[0] is data
alias(payload=data)
writer.assert_any_call(data)

encode = make_encoder()
endpoint = create_autospec(encode, spec_set=True)
assert encode.codec is codec
assert not hasattr(endpoint, "codec")
assert getattr(endpoint, "codec", bytes)(b"archive") == b"archive"

encode = make_encoder()
encoder = Mock(spec_set=encode)
assert encode.codec is codec
assert encoder.codec is encoder.codec
