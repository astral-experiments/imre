import collections.abc
import typing
import warnings
from typing import ByteString


def current():
    assert ByteString is typing.ByteString
    assert ByteString.__origin__ is collections.abc.ByteString
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert isinstance(b"", ByteString)
        assert not isinstance("text", ByteString)
        assert issubclass(bytes, ByteString)
    message = "'typing.ByteString' is deprecated and slated for removal in Python 3.17"
    assert [(item.category, str(item.message)) for item in caught] == [(DeprecationWarning, message)] * 2

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")

        class Both(ByteString, typing.Awaitable):
            pass

    assert Both.__orig_bases__ == (ByteString, typing.Awaitable)
    assert Both.__bases__ == (collections.abc.ByteString, collections.abc.Awaitable, typing.Generic)
    message = "'collections.abc.ByteString' is deprecated and slated for removal in Python 3.17"
    assert [(item.category, str(item.message)) for item in caught] == [(DeprecationWarning, message)]
