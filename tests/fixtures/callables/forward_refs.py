from annotationlib import ForwardRef


def owner(x: int) -> int:
    return x + 1


def main() -> None:
    f = ForwardRef("T")
    named = ForwardRef("T", module="typing", is_argument=False, is_class=True)
    print(repr(f))
    print(repr(named))
    print(f.__forward_arg__)
    print(named.__forward_module__ == "typing")
    print(f.__forward_module__ is None)
    print(named.__forward_is_argument__)
    print(named.__forward_is_class__)
    print(f.__owner__ is None)
    previous = f
    for _ in range(2):
        current = ForwardRef("T")
        print(previous is not current)
        previous = current
    print(isinstance(f, ForwardRef))
    print(isinstance("T", ForwardRef))
    for kind in [bool, int, str]:
        print(issubclass(kind, int))
        print(issubclass(kind, object))
    try:
        isinstance(42, f)
    except TypeError as exc:
        print(str(exc))
    try:
        issubclass(int, f)
    except TypeError as exc:
        print(str(exc))
    print(f == ForwardRef("T", is_argument=False))
    print(f != named)
    print(ForwardRef("T", owner=int).__owner__ is int)
    print(repr(ForwardRef("T", owner=int)))
    print(repr(ForwardRef("T", owner=False)))
    print(repr(ForwardRef("T", owner=0.0)))
    print(repr({"type": named}))
    p = owner
    retained = ForwardRef("arg", owner=p).__owner__
    print(retained is p)
    print(retained(41))
    try:
        ForwardRef(42)
    except TypeError as exc:
        print(str(exc))


main()
