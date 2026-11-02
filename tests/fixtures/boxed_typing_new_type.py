import _typing
from typing import NewType


class Owner:
    Token = NewType('Owner.Token', int)


def current():
    token = Owner.Token
    value = []
    assert token.__name__ == 'Token'
    assert token.__qualname__ == 'Owner.Token'
    assert token.__module__ == __name__
    assert token.__supertype__ is int
    assert token.__call__ is _typing._idfunc
    assert token(value) is value

    try:
        token(x=value)
    except TypeError as exc:
        assert str(exc) == '_typing._idfunc() takes no keyword arguments'
    else:
        raise AssertionError('NewType accepted a keyword argument')
