import typing
from abc import ABC, ABCMeta, abstractmethod
from collections.abc import Mapping, Sized
from typing import Protocol, get_protocol_members, is_protocol


class HasHTML(Protocol):
    def __html__(self):
        ...


class Concrete(HasHTML):
    def __html__(self):
        return "trusted"


class WithInit(Protocol):
    def __init__(self, value):
        self.value = value


class Abstract(Protocol):
    @abstractmethod
    def operation(self):
        ...


def current():
    print(typing.TYPE_CHECKING is False, typing.Protocol is Protocol)
    print(Protocol.__bases__ == (typing.Generic,), HasHTML.__mro__[1:3] == (Protocol, typing.Generic))
    assert is_protocol(HasHTML) and typing.is_protocol(Abstract)
    for arg in (int, typing.TypeVar("T")):
        assert typing.get_args(typing.SupportsAbs[arg]) == (arg,)
    # The base ABC check can recognize a declared non-runtime protocol;
    # the public isinstance operation below still enforces @runtime_checkable.
    assert ABCMeta.__subclasscheck__(HasHTML, Concrete)
    assert ABCMeta.__instancecheck__(HasHTML, Concrete())

    for rejected in (Protocol, typing.Generic, Concrete, Concrete(), int, 1):
        assert not is_protocol(rejected)
    class FalsyMarker:
        _is_protocol = None
    assert is_protocol(FalsyMarker) is None
    print(HasHTML._is_protocol, Concrete._is_protocol, Concrete().__html__())
    print(WithInit("explicit initializer").value)
    print(Protocol.__abstractmethods__ == frozenset(), Abstract.__abstractmethods__ == {"operation"})
    class Forward(Protocol):
        item: NotInScope
        def method(self): ...

    assert get_protocol_members(Forward) == frozenset({'item', 'method'})
    del Forward.method
    class Inherited(Forward, Protocol):
        other: int
    assert get_protocol_members(Inherited) == {'item', 'other'}
    assert get_protocol_members(Forward) == {'item', 'method'}

    def broken(format):
        raise RuntimeError('custom provider')
    Forward.__annotate__ = broken
    try:
        class Broken(Forward, Protocol): pass
    except RuntimeError as error:
        assert str(error) == 'custom provider'
    else:
        raise AssertionError('suppressed a custom annotation provider error')

    parameter = typing.TypeVar("T")
    alias = Protocol[parameter]
    print(alias.__origin__ is Protocol, alias.__args__[0] is parameter)
    for check in (
        lambda: HasHTML(),
        lambda: Abstract(),
        lambda: isinstance("text", (HasHTML, str)),
        lambda: issubclass(Concrete, HasHTML),
    ):
        try:
            check()
        except TypeError as error:
            print(str(error))
    print(isinstance("text", (str, HasHTML)), isinstance(Concrete(), Concrete))

    class Explicit(HasHTML, Sized, Protocol, object):
        pass

    class GenericProtocol(Protocol, typing.Generic[parameter], object):
        pass

    class Ordinary(Explicit, Concrete):
        pass

    print(Explicit._is_protocol, GenericProtocol._is_protocol, Ordinary._is_protocol)
    class Mixed(Abstract, ABC):
        @classmethod
        def __subclasshook__(cls, candidate):
            return True if candidate is str else NotImplemented

    class MixedConcrete(Mixed):
        def operation(self):
            return 42

    assert type(Mixed) is type(Abstract) and not is_protocol(Mixed)
    assert Mixed.__abstractmethods__ == {"operation"} and MixedConcrete().operation() == 42
    assert Mixed.register(Concrete) is Concrete and isinstance(Concrete(), Mixed)
    assert issubclass(str, Mixed)

    for base in (Concrete, Mapping, int):
        try:
            class Invalid(base, Protocol):
                pass
        except TypeError as error:
            assert str(error) == f"Protocols can only inherit from other protocols, got {base!r}"
        else:
            raise AssertionError(f"unexpected protocol base: {base!r}")

    @typing.runtime_checkable
    class HasLower(Protocol):
        def lower(self): ...

    assert issubclass(str, HasLower) and isinstance("text", HasLower)
    assert not issubclass(object, HasLower)

    class Candidate: pass
    assert not issubclass(Candidate, HasLower)
    Candidate.lower = str.lower
    assert not issubclass(Candidate, HasLower)
    assert isinstance(Candidate(), HasLower)
    HasLower._abc_caches_clear()
    assert issubclass(Candidate, HasLower)
    Candidate.lower = None
    assert issubclass(Candidate, HasLower) and isinstance(Candidate(), HasLower)
    HasLower._abc_caches_clear()
    assert not isinstance(Candidate(), HasLower)

    class Registered: pass
    assert not issubclass(Registered, HasLower)
    HasLower.register(Registered)
    assert issubclass(Registered, HasLower)

    class Proxy:
        @property
        def __class__(self): return Registered

    assert isinstance(Proxy(), HasLower)

    @typing.runtime_checkable
    class HasAddNote(Protocol):
        def add_note(self, note): ...

    assert issubclass(ValueError, HasAddNote)
