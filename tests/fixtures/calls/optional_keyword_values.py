"""Annotations do not check arguments; operations still raise their own errors."""
def accept(x: int) -> str:
    print("entered accept")
    return "ok"


def inspect_value(x: int) -> bool:
    return x is None


def rhs(events: list[str], stop: bool) -> int:
    events.append("right")
    if stop:
        raise ValueError("right failed")
    return 1


def arithmetic(x: int, events: list[str], stop: bool) -> int:
    events.append("entered")
    return x + rhs(events, stop)


def run(value: int | None, stop: bool) -> None:
    saved = {"x": value}
    events: list[str] = []
    print(accept(**saved))
    print(inspect_value(**saved))
    try:
        print(arithmetic(events=events, stop=stop, **saved))
    except TypeError as exc:
        print(str(exc))
    except ValueError as exc:
        print(str(exc))
    print(events)




def inline_value(x: int) -> bool:
    return x is None


def saved_literal(x: int) -> bool:
    return x is None


def test_inline(x: int | None) -> None:
    print(inline_value(**{"x": x}))


def test_literal() -> None:
    saved = {"x": None}
    print(saved_literal(**saved))


run(42, False)
run(None, False)
run(None, True)
test_inline(8)
test_inline(None)
print(inline_value(**{"x": None}))
test_literal()
