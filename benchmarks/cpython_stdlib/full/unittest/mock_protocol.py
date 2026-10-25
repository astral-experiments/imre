"""Compare native call recording/arguments against CPython, including rejected calls."""

import unittest.mock
from unittest import mock
from unittest.mock import Mock


def exercise() -> None:
    stream = unittest.mock.Mock(name="stdin")
    stream.close.assert_not_called()
    write = stream.write
    print(write is stream.write)
    print(write is not stream.close)
    print(write == stream.write)
    print(write != stream.close)
    first = write(b"data", check=True, status=None)
    print(first is stream.write.return_value)
    print(write.call_count)
    write.assert_called_once_with(b"data", status=None, check=1)
    same = Mock()
    same(b"data", status=None, check=1)
    original = write.call_args_list[0]
    equal = same.call_args_list[0]
    print(original == equal)
    print(original is equal)
    same(b"data", status=None, check=2)
    print(original != same.call_args_list[1])
    first.close()
    stream.write.assert_called_with(b"data", check=True, status=None)
    print(stream.call_count)
    stream.close()
    stream.close.assert_called_once()
    stream.close.assert_called_with()
    stream.flush()
    stream.flush.assert_called_once_with()
    stream(b"x")
    try:
        stream.assert_not_called()
    except AssertionError as exc:
        print(str(exc))
    try:
        write.assert_called_with("data", check=True, status=None)
    except AssertionError as exc:
        print(str(exc))
    try:
        mock.Mock().assert_called()
    except AssertionError as exc:
        print(str(exc))
    counter = Mock(name="chmod", side_effect=ValueError("denied"))
    try:
        counter(7)
    except ValueError as exc:
        print(str(exc))
    counter.assert_called_once_with(7)
    try:
        counter.assert_called_once_with(False)
    except AssertionError as exc:
        print(str(exc))
    try:
        stream.assret_called()
    except AttributeError as exc:
        print(str(exc))


exercise()
