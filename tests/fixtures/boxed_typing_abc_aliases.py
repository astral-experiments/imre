from collections import abc
import typing
from typing import (
    AbstractSet, AsyncGenerator, AsyncIterable, Collection, Container, Coroutine,
    Hashable, ItemsView, KeysView, MappingView, MutableSequence, MutableSet,
    ValuesView,
)


for alias, origin, arity in (
    (AbstractSet, abc.Set, 1),
    (AsyncGenerator, abc.AsyncGenerator, 2),
    (AsyncIterable, abc.AsyncIterable, 1),
    (Collection, abc.Collection, 1),
    (Container, abc.Container, 1),
    (Coroutine, abc.Coroutine, 3),
    (Hashable, abc.Hashable, 0),
    (ItemsView, abc.ItemsView, 2),
    (KeysView, abc.KeysView, 1),
    (MappingView, abc.MappingView, 1),
    (MutableSequence, abc.MutableSequence, 1),
    (MutableSet, abc.MutableSet, 1),
    (ValuesView, abc.ValuesView, 1),
):
    assert getattr(typing, alias.__name__) is alias
    arguments = (str, int, bytes)[:arity]
    specialized = alias[arguments]
    assert typing.get_origin(alias) is origin
    assert typing.get_args(alias) == ()
    assert typing.get_origin(specialized) is origin
    assert typing.get_args(specialized) == arguments

    for base in (alias, specialized):
        class Child(base):
            pass

        assert Child.__orig_bases__ == (base,)
        assert Child.__bases__ == (origin, typing.Generic)
        if alias in (ItemsView, KeysView, MappingView, ValuesView):
            data = {"a": 1}
            view = base(data)
            assert type(view) is origin
            data["b"] = 2
            assert len(view) == 2
        else:
            try:
                base()
            except TypeError:
                pass
            else:
                raise AssertionError(f"{base} instantiated an abstract class")

    for wrong in ((), (str, int, bytes, bool)):
        if not arity and not wrong:
            continue
        try:
            alias[wrong]
        except TypeError as error:
            if not arity:
                assert str(error) == f"{alias} is not a generic class"
            else:
                expected = "at least 1" if alias is AsyncGenerator else str(arity)
                which = "many" if wrong else "few"
                assert str(error) == f"Too {which} arguments for {alias}; actual {len(wrong)}, expected {expected}"
        else:
            raise AssertionError(f"{alias} accepted {wrong}")

assert typing.get_args(AsyncGenerator[int]) == (int, type(None))
assert list(ItemsView({"a": 1})) == [("a", 1)]
assert list(KeysView[str]({"a": 1})) == ["a"]
assert list(ValuesView[int]({"a": 1})) == [1]
print("ABC aliases")
