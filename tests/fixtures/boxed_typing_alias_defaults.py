from collections.abc import Generator as GeneratorABC
from contextlib import AbstractAsyncContextManager, AbstractContextManager
from typing import AsyncContextManager, ContextManager, Generator, get_args, get_origin


for alias, origin, defaults in (
    (Generator, GeneratorABC, (type(None), type(None))),
    (ContextManager, AbstractContextManager, (bool | None,)),
    (AsyncContextManager, AbstractAsyncContextManager, (bool | None,)),
):
    assert get_origin(alias) is origin
    assert get_origin(alias[bytes]) is origin
    assert get_args(alias[bytes]) == (bytes, *defaults)
    assert get_args(alias) == ()
    for invalid in ((), (int, str, bytes, bool)):
        try:
            alias[invalid]
        except TypeError as exc:
            assert "expected at least 1" in str(exc)
        else:
            raise AssertionError(invalid)

assert get_args(Generator[bytes, str]) == (bytes, str, type(None))
assert get_args(AsyncContextManager[bytes, None]) == (bytes, type(None))
print("alias defaults")
