"""Definition identity, executable binding and the code registry used by Jinja."""

from types import CodeType

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
    saved = view.__code__
    method = Views().get
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
