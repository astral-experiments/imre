import _io
import inspect
import io
import os
import sys
import tempfile
from unittest.mock import Mock


def expect_error(error, method, *args):
    try:
        method(*args)
    except error:
        return
    raise AssertionError(method.__name__)


descriptor = _io._IOBase.__dict__["__dict__"]
assert descriptor.__get__(None, io.StringIO) is descriptor


def check_stream(stream):
    namespace = descriptor.__get__(stream, type(stream))
    assert namespace is stream.__dict__
    namespace["descriptor_probe_marker"] = 17
    assert stream.descriptor_probe_marker == 17
    assert inspect.getattr_static(stream, "descriptor_probe_marker") == 17
    assert inspect.getattr_static(stream, "__dict__") is descriptor
    assert inspect.getattr_static(type(stream), "__dict__") is descriptor
    del stream.descriptor_probe_marker
    assert "descriptor_probe_marker" not in namespace
    expect_error(AttributeError, descriptor.__set__, stream, {})
    expect_error(AttributeError, descriptor.__delete__, stream)
    expect_error(AttributeError, setattr, stream, "__dict__", {})
    expect_error(AttributeError, delattr, stream, "__dict__")


with io.StringIO() as stream:
    check_stream(stream)
with open(os.devnull) as stream:
    check_stream(stream)
    reader = Mock(spec=type(stream))
    reader.read.return_value = "reply"
    assert reader.read() == "reply"
    reader.read.assert_called_once_with()
    writer = Mock(spec=stream)
    writer.write("message")
    writer.write.assert_called_once_with("message")
with open(os.devnull, "rb") as stream:
    check_stream(stream)
with tempfile.TemporaryFile("w+b") as stream:
    check_stream(stream)
check_stream(sys.stdout)
check_stream(sys.stdout.buffer)
expect_error(TypeError, descriptor.__get__, object(), object)
expect_error(TypeError, descriptor.__set__, object(), {})
expect_error(TypeError, descriptor.__delete__, object())
