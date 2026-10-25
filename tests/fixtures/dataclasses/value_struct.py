from copy import deepcopy
from imre import struct as value
from imre.json import decode, encode


@value
class Reading:
    x: float
    valid: bool

    def twice(self) -> float:
        return 2.0 * self.x


@value
class Other:
    x: float
    valid: bool


def refill(rows: list[Reading], saved: Reading) -> Reading:
    rows.clear()
    for i in range(64):
        rows.append(Reading(valid=True, x=float(i)))
    return saved


def mutate(rows: list[Reading]) -> float:
    for item in rows:
        rows[0] = Reading(99.0, False)
        rows.clear()
        return item.twice()
    return 0.0


def rank(row: Reading) -> int:
    return int(row.x)


def optional(row: Reading | None) -> bool:
    return row is None


def mark(n: float) -> float:
    print(n)
    return n


def main() -> None:
    p = Reading(1.0, True)
    rows = [p, p]
    alias = rows
    saved = rows[0]
    rows[0] = Reading(3.0, False)
    print(alias[0].x)
    by_name = {"saved": saved}
    print(by_name["saved"].x)
    held = by_name["saved"]
    print(held.twice())
    print(saved == rows[1])
    print(p == saved)
    print(optional(p))
    print(optional(None))
    snapshot = refill(rows, rows[-1])
    print(snapshot)
    print(rows[63])
    print(rows[-2].twice())
    copied = rows[4:1:-1] * 2
    rows.clear()
    print(repr(copied))
    print(copied.pop())
    print(len(copied))
    print(len(copied * 0))
    print((copied * 5)[-1])
    copied[0] = copied[-1]
    print(mutate(copied))
    print(len(copied))
    n = Reading(float("nan"), True)
    n2 = Reading(-float("nan"), True)
    z = Reading(-0.0, False)
    zz = Reading(0.0, False)
    print(n == n2)
    print(z == zz)
    print(n.x == n.x)
    print(n == Other(n.x, True))
    samples = [n, z]
    print(n2 in samples)
    print(zz not in samples)
    print(samples == [n2, zz])
    codes = {n: 7, z: 9}
    print(hash(n))
    print(hash(z))
    print(codes[n2])
    print(codes[zz])
    print(len({n, n2, z, zz}))
    try:
        print(samples[-3].x)
    except IndexError:
        print("index error")
    try:
        samples[9] = n
    except IndexError:
        print("write error")
    print(samples[1])
    samples.clear()
    try:
        samples.pop()
    except IndexError:
        print("pop error")
    samples.append(Reading(x=mark(4.0), valid=False))
    print(repr(samples))
    more = [Reading(2.0, True), Reading(1.0, True), Reading(1.0, False)]
    print(sum(len([row]) for row in more))
    more.sort(key=rank)
    print(repr(more))
    more.extend(more[:2])
    clone = deepcopy(more)
    more[0] = Reading(3.0, False)
    print(repr(clone))
    print(encode(clone))
    decoded = decode('[{"x":4.0,"valid":true}]', list[Reading])
    decoded.append(p)
    print(encode(decoded))


main()
