from annotationlib import Format, ForwardRef, get_annotations
from collections.abc import Callable


def make() -> Callable[[int], int]:
    def identity(x: int) -> int:
        return x
    return identity


def declared(value: int) -> unseen: ...


def alias() -> Callable[[int], None]:
    return declared


def main() -> None:
    previous = ForwardRef("missing")
    for i in range(3):
        fresh = ForwardRef("missing")
        print(previous is not fresh)
        previous = fresh
    def original(value: int) -> missing: ...

    annotations = get_annotations(original, format=Format.FORWARDREF)
    print(annotations["value"] is int)
    reference = annotations["return"]
    print(isinstance(reference, ForwardRef))
    if isinstance(reference, ForwardRef):
        print(reference.__forward_arg__)
        print(reference.__forward_module__ is None)
        print(reference.__forward_is_class__)
        print(reference.__forward_is_argument__)
        print(reference.__owner__ is original)
    print(original(1) is None)
    try:
        annotations["value"].__forward_arg__
    except AttributeError as exc:
        print(str(exc))
    del annotations["value"]
    again = get_annotations(original, format=Format.FORWARDREF)
    print("value" in again)
    print(again["return"] is not reference)
    try:
        get_annotations(original, format=Format.VALUE)
    except NameError as exc:
        print(str(exc))
    first = make()
    second = make()
    print(first is not second)
    print(first(42))
    print(alias() is declared)
    outside = get_annotations(declared, format=Format.FORWARDREF)
    print(outside["return"].__owner__ is alias())


main()
