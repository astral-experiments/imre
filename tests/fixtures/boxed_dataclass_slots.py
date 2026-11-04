import gc
import weakref
from dataclasses import FrozenInstanceError, dataclass, field
from inspect import getattr_static


def current():
    class Original:
        number: int
        label: str = 'value'

        def owner(self) -> type:
            return __class__

    old_namespace = Original.__dict__
    dict_descriptor = old_namespace['__dict__']
    old_instance = Original()
    old_instance.note = 'kept'
    old_ref = weakref.ref(old_instance)
    Slotted = dataclass(frozen=True, slots=True)(Original)
    assert '__dict__' not in old_namespace and '__weakref__' not in old_namespace
    assert not hasattr(old_instance, '__dict__') and not hasattr(old_instance, '__weakref__')
    names = dir(old_instance)
    assert '__dict__' not in names and 'note' not in names
    assert getattr_static(old_instance, '__dict__', None) is None
    assert getattr_static(old_instance, 'note', None) is None
    assert old_ref() is old_instance
    assert dict_descriptor.__get__(old_instance, Original)['note'] == old_instance.note == 'kept'
    assert Slotted is not Original and Slotted.__qualname__ == Original.__qualname__
    assert Slotted.__slots__ == ('number', 'label')
    obj = Slotted(4)
    assert not hasattr(obj, '__dict__') and obj.owner() is Slotted
    assert Slotted.owner.__annotate__(2) == {"return": type}
    assert Slotted.label.__get__(obj, Slotted) == 'value'
    assert Slotted.number.__objclass__ is Slotted
    assert obj.__getstate__() == [4, 'value']
    obj.__setstate__([6, 'next'])
    assert obj.number == 6 and obj.label == 'next'
    for name, error in (('number', FrozenInstanceError), ('other', TypeError)):
        try:
            setattr(obj, name, 2)
        except Exception as exc:
            assert type(exc) is error
        else:
            raise AssertionError('exact frozen class accepted assignment')
    Slotted.__annotations__ = {'number': float, 'label': bytes}
    assert Slotted.__init__.__annotate__(1) == {'number': float, 'label': bytes, 'return': None}
    clone = object.__new__(Slotted)
    clone.__setstate__(obj.__getstate__())
    assert clone == obj

    class Marker:
        pass
    Marker.__wrapped__ = Marker

    @dataclass(slots=True)
    class WithFactory:
        marker = Marker
        xs: list = field(init=False, default_factory=list)
        count: int = field(init=False, default=2)

    left, right = WithFactory(), WithFactory()
    assert WithFactory.marker is Marker
    assert left.count == right.count == 2
    left.xs.append("left")
    assert right.xs == []
    def discarded():
        class Temporary:
            x: int
        replacement = dataclass(frozen=True, slots=True)(Temporary)
        return weakref.ref(Temporary), weakref.ref(replacement)

    original_ref, replacement_ref = discarded()
    gc.collect()
    assert original_ref() is None and replacement_ref() is None
    @dataclass(slots=True, weakref_slot=True)
    class Parent:
        x: int

    @dataclass(slots=True, weakref_slot=True)
    class Child(Parent):
        x: int
        y: int

    child = Child(2, 3)
    assert Child.__slots__ == ('y',)
    assert Child.x is Parent.x and 'x' not in Child.__dict__
    assert Parent.x.__get__(child) == 2 and child.y == 3
    assert not hasattr(child, '__dict__') and weakref.ref(child)() is child

    class IteratorSlots:
        __slots__ = iter(('used',))

    class Bad(IteratorSlots):
        y: list = []

    bad_namespace = Bad.__dict__
    try:
        dataclass(Bad, slots=True)
    except ValueError as exc:
        assert 'mutable default' in str(exc)
        assert '__dict__' in bad_namespace and '__weakref__' in bad_namespace
    else:
        raise AssertionError('slot inspection preceded field validation')

    class Existing:
        __slots__ = ()
        y: list = []

    try:
        dataclass(Existing, slots=True)
    except ValueError as exc:
        assert 'mutable default' in str(exc)
    else:
        raise AssertionError('own slots preceded field validation')

    class Late(IteratorSlots):
        y: int

    late_namespace = Late.__dict__
    try:
        dataclass(Late, slots=True)
    except TypeError as exc:
        assert str(exc) == "Slots of 'IteratorSlots' cannot be determined"
        assert '__init__' in Late.__dict__
        assert '__dict__' not in late_namespace and '__weakref__' not in late_namespace
    else:
        raise AssertionError('accepted an iterator as an inherited slot declaration')

    print('replacement slots, class cell, state and initializer annotations')
