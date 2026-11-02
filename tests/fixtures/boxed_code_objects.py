"""Definition identity, executable binding and the code registry used by Jinja."""

import sys
from types import CodeType

assert sys._getframe().f_code.co_qualname == "<module>"
assert sys._getframe().f_code.co_argcount == 0

registered = set()


def register(function):
    registered.add(function.__code__)
    return function


@register
def view(value=41):
    return value + 1


def replacement(value=9):
    return value - 1


class Views:
    @register
    def get(self):
        return 7


def factory():
    return (lambda: 3, lambda: 3)


def current():
    from types import FunctionType, GeneratorType, coroutine

    def values():
        yield 1

    before = values()
    original = values.__code__
    assert original.co_qualname == "current.<locals>.values"
    assert coroutine(values) is values
    after = values()
    assert type(after) is GeneratorType
    assert before.gi_code is original and after.gi_code is values.__code__
    assert after.gi_code.co_flags == original.co_flags | 0x100
    assert coroutine(values).__code__ is after.gi_code
    assert next(before) == next(after) == 1

    def mixed(first, /, second, optional=3, *args, required, **kwargs):
        pass

    def variadic(*args, **kwargs):
        pass

    saved = view.__code__
    method = Views().get
    assert mixed.__code__.co_argcount == 3 and variadic.__code__.co_argcount == 0
    assert saved.co_argcount == 1
    assert method.__code__.co_argcount == Views.get.__code__.co_argcount == 1
    assert CodeType.co_argcount.__get__(mixed.__code__, CodeType) == 3
    try:
        CodeType.co_argcount.__get__(object(), object)
    except TypeError:
        pass
    else:
        raise AssertionError("argument-count descriptor accepted a non-code receiver")
    rebuilt = FunctionType(saved, globals())
    assert rebuilt.__code__.co_argcount == 1 and rebuilt(41) == 42
    compiled_gen = (x for x in range(2))
    loaded_gen = eval("(x for x in range(2))")
    for gen in (compiled_gen, loaded_gen):
        assert gen.gi_code.co_argcount == 1
        assert next(gen) == 0 and next(gen) == 1
    for change in (lambda: setattr(saved, "co_argcount", 7), lambda: delattr(saved, "co_argcount")):
        try:
            change()
        except AttributeError as error:
            assert str(error) == "readonly attribute"
        else:
            raise AssertionError("code argument count is writable")
    assert saved.co_qualname == "view" and method.__code__.co_qualname == "Views.get"
    view.__qualname__ = "renamed"
    assert saved.co_qualname == "view"
    view.__qualname__ = saved.co_qualname
    for action in (lambda: setattr(saved, "co_qualname", "wrong"), lambda: delattr(saved, "co_qualname")):
        try:
            action()
        except AttributeError as error:
            assert str(error) == "readonly attribute"
        else:
            raise AssertionError("code qualified name is writable")
    print(view.__code__ is saved, method.__code__ is Views.get.__code__)
    print(len(registered), saved in registered, method.__code__ in registered)
    print(replacement.__code__ in registered, view(), method())
    print(saved.co_name, saved.co_filename == __file__, saved.co_firstlineno)
    print(type(saved).__name__, type(saved) is CodeType, type(CodeType) is type, isinstance(method.__code__, CodeType))
    print(hasattr(len, "__code__"), callable(saved))
    one, two = factory()
    again, other = factory()
    print(one is not again, one.__code__ is again.__code__, two.__code__ is other.__code__)
    print(one.__code__ is not two.__code__, one())
    view.__name__ = "renamed"
    print(saved.co_name, view.__name__)
    view.__code__ = replacement.__code__
    print(view(), view.__code__ is replacement.__code__, saved in registered)
    view.__code__ = saved
    print(view())
    for action in (lambda: setattr(saved, "co_name", "wrong"), lambda: setattr(view, "__code__", 42), lambda: saved()):
        try:
            action()
        except (TypeError, AttributeError) as error:
            print(type(error).__name__, str(error))


if __name__ == "__main__":
    current()
