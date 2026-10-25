"""An exhaustive dispatch can bind a function, its options, and its keyword shape."""


def witness(n: int) -> int:
    print(n)
    return n


def label(first: int, second: int, *, suffix: str = "") -> str:
    return f"{first}:{second}{suffix}"


def fixed_tuple() -> None:
    values = (witness(1), witness(2))
    suffix = "!"
    keywords = {"suffix": suffix}
    suffix = "?"
    print(label(*values, **keywords))
    print(label(*(witness(3), witness(4)), **{"suffix": suffix}))


class Command:
    def __init__(self) -> None:
        pass

    def invoke(self, prefix: str, *args: str, suffix: str = "") -> str:
        return prefix + ",".join([*args]) + suffix


def number(x: int) -> str:
    return str(x + 1)


def text(x: str, suffix: str) -> str:
    return x + suffix


def dispatch(mode: int) -> str:
    match mode:
        case 1:
            operation, arg, options = number, 5, {}
        case 2:
            operation, arg, options = text, "present", {"suffix": "!"}
        case _:
            return "no operation"
    print("selected")
    return operation(arg, **options)


def optional(n: int | None) -> str:
    keywords = {"x": n}
    try:
        return number(**keywords)
    except TypeError:
        return "TypeError"


def optional_parameter(x: int | None) -> bool:
    return x is None


def accepts_nullable(x: int | None) -> bool:
    keywords = {"x": x}
    return optional_parameter(**keywords)


fixed_tuple()
print(Command().invoke("/"))
print(Command().invoke("/", "a", "b", suffix="."))
print(dispatch(1))
print(dispatch(2))
print(dispatch(3))
print(optional(9))
print(optional(None))
print(accepts_nullable(1))
print(accepts_nullable(None))
