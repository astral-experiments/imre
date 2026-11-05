from functools import partial
from types import ClassMethodDescriptorType, GenericAlias


def current():
    class Callback(partial):
        pass

    class Override(Callback):
        def __class_getitem__(cls, key):
            return cls, key

    raw = vars(partial)["__class_getitem__"]
    assert type(raw) is ClassMethodDescriptorType
    assert raw.__doc__ == "See PEP 585" and raw.__text_signature__ == "($type, object, /)"
    for cls in (partial, Callback):
        method = cls.__class_getitem__
        assert method.__self__ is cls
        assert cls(int).__class_getitem__.__self__ is cls
        for alias in (cls[int], method(int), raw(cls, int), raw.__get__(None, cls)(int)):
            assert type(alias) is GenericAlias
            assert alias.__origin__ is cls and alias.__args__ == (int,)
            assert alias.__parameters__ == ()
    assert Override[str] == (Override, str)
    for args in ((), (dict, int), (42, int), (partial,), (partial, int, str)):
        try:
            raw(*args)
        except TypeError:
            pass
        else:
            raise AssertionError(args)
    try:
        partial.__class_getitem__(object=int)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a keyword argument")
    print("partial class subscription")
