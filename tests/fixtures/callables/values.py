from typing import Callable


def first(value: int) -> int:
    print("first")
    return value + 1


def second(value: int) -> int:
    print("second")
    return value * 2


def choose(flag: bool) -> Callable[[int], int]:
    return first if flag else second


def forward(fn: Callable[[int], int]) -> Callable[[int], int]:
    return fn


def apply(fn: Callable[[int], int], value: int) -> int:
    return fn(value)


def mixed(number: int, ratio: float, text: str, values: list[int], flag: bool) -> str:
    values.append(number)
    return f"{number}:{ratio}:{text}:{values[0]}:{flag}"


def optional(value: float | None) -> float | None:
    return value


def show_optional(value: float | None) -> None:
    if value is None:
        print(None)
    else:
        print(value)


def half(value: float) -> float:
    return value / 2.0


def raw(value: bytes) -> bytes:
    return value


def truth(value: int) -> bool:
    return value > 0


def union_identity(value: str | bytes) -> str | bytes:
    return value


def show_union(value: str | bytes) -> None:
    if isinstance(value, str):
        print(value)
    else:
        print(value)


def copy(values: list[str]) -> list[str]:
    return values[:]


def nothing(value: str) -> None:
    print(value)


def defaulted(value: int, extra: int = 3) -> int:
    return value + extra


def maybe(fn: Callable[[int], int] | None, value: int) -> int:
    if fn is None:
        return value
    return fn(value)


def bounded(values: list[int]) -> int:
    total = 0
    for index in range(len(values)):
        total += values[index] // 8
    return total


def inferred(value):
    return value + 3


def main() -> None:
    fn: Callable[[int], int] = first
    print(bool(fn))
    print(apply(forward(fn), 1 << 100))
    relay: Callable[[Callable[[int], int]], Callable[[int], int]] = forward
    print(relay(fn)(14))
    print(choose(False)(7))
    print((first if True else second)(8))
    # The original callee is retained before a later argument rebinds its slot.
    print(fn((fn := second)(9)))
    print(fn(10))
    print(maybe(None, 12))
    print(maybe(first, 12))
    optional_fn: Callable[[int], int] | None = second
    if optional_fn is not None:
        print(optional_fn(13))
    optional_fn = None
    print(optional_fn is None)
    square: Callable[[int], int] = lambda value: value * value
    print(apply(square, -(1 << 80)))
    named: Callable[[int], int] = lambda value: first(value)
    print(named(11))
    scalar: Callable[[], str] = lambda: "no arguments"
    print(scalar())
    packed: Callable[[int, float, str, list[int], bool], str] = mixed
    values = [1 << 100]
    print(packed(-(1 << 120), -0.5, "mixed Ω", values, True))
    print(values[1])
    nullable: Callable[[float | None], float | None] = optional
    show_optional(nullable(None))
    show_optional(nullable(-0.0))
    floating: Callable[[float], float] = half
    print(floating(-0.0))
    print(floating(3.5))
    binary: Callable[[bytes], bytes] = raw
    print(binary(b"\x00\xff"))
    boolean: Callable[[int], bool] = truth
    print(boolean(-(1 << 100)))
    print(boolean(1 << 100))
    union: Callable[[str | bytes], str | bytes] = union_identity
    show_union(union("tagged Ω"))
    show_union(union(b"tagged\x00\xff"))
    copied: Callable[[list[str]], list[str]] = copy
    print(copied(["owned"])[0])
    sink: Callable[[str], None] = nothing
    print(sink("none result"))
    complete: Callable[[int, int], int] = defaulted
    print(complete(2, 4))
    print(defaulted(extra=5, value=2))
    print(defaulted(2))
    fn = first
    print(sum(map(fn, [1, 2]), (fn := second)(0)))
    print(fn(3))
    # A small direct caller cannot constrain later callers through a value.
    print(bounded([1, 2, 3]))
    reduction: Callable[[list[int]], int] = bounded
    large = [1 << 100, 1 << 120]
    print(reduction(large))
    large[0] = -(1 << 140)
    print(reduction(large))
    contextual: Callable[[int], int] = inferred
    print(contextual(5))
    shadow: int | None = 7
    if shadow is not None:
        print(apply(lambda shadow: (shadow := shadow + 1), 2))
        print(shadow + 1)


main()
