"""Bounded inputs for the typed list slice-sum rewrite. Run as ordinary Python."""


def stride(mode: int, trace: list[int]) -> int:
    stream = open("bound.tmp", "w", encoding="utf-8")
    try:
        trace.append(10)
        if mode == 0:
            return 1
        if mode == 1:
            return -2
        if mode == 2:
            return 0
        if mode == 3:
            raise TypeError("bound")
        return 18446744073709551616
    finally:
        stream.close()
        trace.append(11 if stream.closed else 19)


def start(ys: list[int], mode: int, trace: list[int]) -> int:
    stream = open("start.tmp", "w", encoding="utf-8")
    try:
        trace.append(20)
        if mode == 1 or mode == 2:
            ys.append(2)
        if mode == 2:
            raise ValueError("start")
        if mode == 3 and len(ys) > 0:
            ys[0] = 99
        return 0
    finally:
        stream.close()
        trace.append(21 if stream.closed else 29)


def show(name: str, values: list[int]) -> None:
    text = name
    for value in values:
        text += f" {value}"
    print(text)


# Keep the two rewrite sites in single-return functions so the validator can
# inspect their typed expressions before any later specialization.
def full(xs: list[int], ys: list[int], effect: int, trace: list[int]) -> int:
    return sum(xs[:], start(ys, effect, trace))


def stepped(
    xs: list[int], ys: list[int], effect: int, bound: int, trace: list[int]
) -> int:
    return sum(xs[:: stride(bound, trace)], start(ys, effect, trace))


def one(
    index: int, initial: list[int], origin: int, shape: int, effect: int, bound: int
) -> None:
    xs = initial[:]
    ys = xs if origin == 0 else (initial[:] if origin == 1 else [])
    trace: list[int] = []
    print(f"C {index} {origin} {shape} {effect} {bound}")
    try:
        if shape == 0:
            value = sum(xs, start(ys, effect, trace))
        elif shape == 1:
            value = full(xs, ys, effect, trace)
        else:
            value = stepped(xs, ys, effect, bound, trace)
        print(f"R {value}")
    except ValueError as exc:
        print(f"V {str(exc)}")
    except TypeError as exc:
        print(f"T {str(exc)}")
    finally:
        trace.append(30)
    show("X", xs)
    show("Y", ys)
    show("E", trace)


def main() -> None:
    heaps: list[list[int]] = [[], [1], [0], [-1]]
    for a in range(-1, 2):
        for b in range(-1, 2):
            heaps.append([a, b])
    heaps.append([1, 2, 3])
    heaps.append([4611686018427387903, 1])
    heaps.append([-4611686018427387904, -1])
    heaps.append([4611686018427387904])
    heaps.append([-4611686018427387905])
    for index in range(len(heaps)):
        for origin in range(3):
            for shape in range(3):
                for effect in range(4):
                    for bound in range(5 if shape == 2 else 1):
                        one(index, heaps[index], origin, shape, effect, bound)


main()
