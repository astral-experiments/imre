import gc
import weakref
from dataclasses import FrozenInstanceError, dataclass, field


def current():
    class Original:
        number: int
        label: str = 'value'

        def owner(self):
            return __class__

    Slotted = dataclass(frozen=True, slots=True)(Original)
    assert Slotted is not Original and Slotted.__qualname__ == Original.__qualname__
    assert Slotted.__slots__ == ('number', 'label')
    obj = Slotted(4)
    assert not hasattr(obj, '__dict__') and obj.owner() is Slotted
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

    @dataclass(slots=True, init=False, repr=False, eq=False)
    class KeywordField:
        __annotations__ = {"class": str}

    instance = KeywordField()
    setattr(instance, "class", "value")
    assert getattr(instance, "class") == "value"
    assert KeywordField.__dict__["class"].__objclass__ is KeywordField

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
    print('replacement slots, class cell, state and initializer annotations')
