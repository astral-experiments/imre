from types import MethodType


def current():
    class Receiver:
        def method(self):
            pass

        @classmethod
        def family(cls):
            pass

        def __repr__(self):
            return 'live receiver'

    receiver = Receiver()
    assert repr(Receiver.family) == f'<bound method {Receiver.family.__qualname__} of {Receiver!r}>'
    saved = receiver.method
    expected = f'<bound method {Receiver.method.__qualname__} of live receiver>'
    assert repr(saved) == str(saved) == expected
    Receiver.method.__qualname__ = 'renamed'
    assert repr([saved]) == '[<bound method renamed of live receiver>]'

    class Broken(Receiver):
        def __repr__(self):
            raise ValueError('receiver repr')

    try:
        repr(Broken().method)
    except ValueError as exc:
        assert str(exc) == 'receiver repr'
    else:
        raise AssertionError('receiver repr did not run')

    class Callback:
        def __call__(self):
            pass

    callback = Callback()
    bound = MethodType(callback, receiver)
    assert repr(bound) == '<bound method ? of live receiver>'
    callback.__name__ = 'fallback'
    assert repr(bound) == '<bound method fallback of live receiver>'
    callback.__qualname__ = 42
    assert repr(bound) == '<bound method ? of live receiver>'

    class LookupError(Callback):
        def __getattr__(self, name):
            raise ValueError(name)

    try:
        repr(MethodType(LookupError(), receiver))
    except ValueError as exc:
        assert str(exc) == '__qualname__'
    else:
        raise AssertionError('metadata exception was swallowed')

    wrapper = (1).__repr__
    assert repr(wrapper).startswith("<method-wrapper '__repr__' of int object at 0x")
    assert wrapper() == '1'
    return 0
