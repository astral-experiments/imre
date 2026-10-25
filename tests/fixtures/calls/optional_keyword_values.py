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


run(42, False)
run(None, False)
run(None, True)
