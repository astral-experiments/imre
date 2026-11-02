from _weakref import ref as raw_ref
from _weakrefset import WeakSet as raw_set
from weakref import WeakKeyDictionary, WeakMethod, WeakSet, ref


class Key:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        return 7

    def __eq__(self, other):
        return isinstance(other, Key) and self.name == other.name


def current():
    print(ref is raw_ref, WeakSet is raw_set, WeakMethod.__bases__ == (ref,))
    first, equal, collision = Key("first"), Key("first"), Key("second")
    caches = [WeakKeyDictionary(), WeakKeyDictionary(), WeakKeyDictionary()]
    caches[0][first] = "value"
    caches[0][collision] = "other"
    print([len(cache) for cache in caches], caches[0].get(equal), caches[0][collision])
    print([] in caches[0])
    observed = ref(first)
    del first
    print(observed() is None, equal in caches[0], caches[0][collision])
    generic_alias_weakref()


def generic_alias_weakref():
    from collections.abc import Callable
    from gc import collect
    from types import GenericAlias

    for alias in (list[int], Callable[[int], str]):
        handle = ref(alias)
        print(handle() is alias, ref(alias) is handle)

    owner = Key("origin")
    alias = GenericAlias(owner, int)
    owner.alias = alias
    callbacks = []
    handle = ref(alias, callbacks.append)
    del owner, alias
    collect()
    print(handle() is None, len(callbacks) == 1 and callbacks[0] is handle)
