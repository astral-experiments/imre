import typing
from typing import overload, get_overloads
def run() -> None:
    def f() -> None: pass
    def other() -> None: pass
    print(f.__name__ is f.__name__)
    print(f.__qualname__ is f.__qualname__)
    print(f.__module__ is f.__module__)
    print(len(typing._overload_registry))
    f.__module__ = "first"
    f.__qualname__ = "same"
    f.__deprecated__ = "marker"
    overload(f)
    print(len(typing._overload_registry))
    other.__module__ = "first"
    other.__qualname__ = "same"
    result = get_overloads(other)
    print(len(result))
    print(result[0] is f)
    print(result[0].__deprecated__)
    f.__qualname__ = "second"
    print(len(get_overloads(other)))
    print(len(get_overloads(f)))
    result[0].__name__ = "renamed"
    print(f.__name__)
    try:
        print(other.__deprecated__)
    except AttributeError as e:
        print(str(e))
run()
