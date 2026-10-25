from annotationlib import Format, ForwardRef, get_annotations
from functools import cache, lru_cache


def documented(x: int) -> str: ...


def outer() -> None:
    def local(x: int) -> Missing: ...

    first = lru_cache(1)(local)
    second = cache(first)
    print(first.__wrapped__ is local)
    print(second.__wrapped__ is first)
    print(first(7) is None)
    for i in range(2):
        refs = get_annotations(second, format=Format.FORWARDREF)
        value = refs["return"]
        print(refs["x"] is int)
        if isinstance(value, ForwardRef):
            print(value.__forward_arg__)
            print(value.__owner__ is second)
            print(value.__owner__ is first)
    try:
        get_annotations(first, format=Format.VALUE)
    except NameError as exc:
        print(str(exc))
    print(get_annotations(first, format=Format.STRING))
    a = get_annotations(cache(documented))
    b = {"return": str, "x": int}
    print(a == b)
    b["x"] = str
    print(a != b)
    del a["return"]
    print(a != b)
    print(get_annotations(documented))


outer()
