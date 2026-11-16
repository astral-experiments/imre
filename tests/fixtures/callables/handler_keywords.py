def double(n: int) -> int:
    return n * 2


def triple(value: int) -> int:
    return value * 3


def positional(n: int, /) -> int:
    return n


def argument() -> int:
    print("argument")
    return 21


handlers = {"double": double, "triple": triple, "positional": positional}
for name in ("double", "triple", "positional"):
    try:
        print(handlers[name](n=argument()))
    except TypeError:
        print("unexpected keyword")
