import functools
import inspect
import types
from inspect import isdatadescriptor


def function(value):
    return value + 1


async def deferred():
    raise AssertionError("inspection awaited the coroutine")


class Callable:
    def __call__(self):
        return 3

    @functools.singledispatchmethod
    def render(self, value):
        return "default", self

    @render.register(int)
    def as_int(self, value):
        return value + 1


class Descriptor:
    def __get__(self, instance, owner):
        return instance


class DataDescriptor(Descriptor):
    def __delete__(self, instance):
        pass


class SetOnly:
    def __set__(self, instance, value):
        pass


def current():
    obj = Callable()
    bound = function.__call__
    assert type(bound) is types.MethodWrapperType
    assert inspect.ismethodwrapper(bound) and bound(6) == 7
    assert inspect.isbuiltin([].append)
    for routine in (function, bound, type(function).__call__, list.append, [].append,
                    Callable.__call__, obj.__call__, Callable.render, obj.render, Descriptor()):
        assert inspect.isroutine(routine)
    for value in (inspect, Callable, obj, DataDescriptor(), None):
        assert not inspect.isroutine(value)
    partial = functools.partial(pow, 2)
    get = type(partial).__get__
    assert inspect.isroutine(partial) and inspect.ismethoddescriptor(partial)
    assert get.__objclass__ is functools.partial
    assert get(partial, None, Callable) is partial
    bound = partial.__get__(3)
    assert bound.__func__ is partial and bound.__self__ == 3 and bound() == 8
    try:
        get(function, obj)
    except TypeError:
        pass
    else:
        raise AssertionError("partial descriptor accepted a function")

    assert isdatadescriptor is inspect.isdatadescriptor
    assert all(isdatadescriptor(value) for value in (DataDescriptor(), SetOnly(), property(), int.numerator))
    instance_only = Descriptor()
    instance_only.__set__ = True
    assert not any(isdatadescriptor(value) for value in (instance_only, Descriptor(), DataDescriptor,
                                                         function, obj.__call__))
    assert obj.render("text") == ("default", obj)
    assert obj.render(6) == 7
    assert obj.render.__wrapped__ is Callable.__dict__["render"].func
    obj.__name__ = "deferred"
    obj.__code__ = deferred.__code__
    obj.__defaults__ = ()
    obj.__kwdefaults__ = None
    assert inspect.iscoroutinefunction(obj)
    obj.__defaults__ = []
    assert not inspect.iscoroutinefunction(obj)
    obj.__defaults__ = None
    obj.__code__ = function.__code__
    assert not inspect.iscoroutinefunction(obj)
    print("routine predicates and bound dispatch")
