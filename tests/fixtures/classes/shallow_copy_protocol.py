import copy
from copy import copy as duplicate


class Packet:
    payload: list[int]

    def __init__(self, payload: list[int]) -> None:
        self.payload = payload

    def tagged(self) -> bool:
        return False

    def __copy__(self) -> Packet:
        self.payload.append(1)
        return Packet(self.payload)


class Tagged(Packet):
    def tagged(self) -> bool:
        return True

    def __copy__(self) -> Packet:
        self.payload.append(2)
        return Tagged(self.payload)


class Other:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def __copy__(self) -> str:
        if self.value == 0:
            raise ValueError("unavailable")
        return str(self.value)


def choose(obj: Packet) -> Packet:
    return duplicate(obj)


def make(log: list[int]) -> Tagged:
    log.append(0)
    return Tagged(log)


def main() -> None:
    calls: list[int] = []
    obj = make(calls)
    clone = choose(obj)
    print(clone.tagged())
    print(clone.payload is calls)
    print(repr(calls))
    ordinary = choose(Packet(calls))
    print(ordinary.tagged())
    print(repr(ordinary.payload))
    direct = obj.__copy__()
    print(repr(direct.payload))
    copy.copy(make(calls))
    print(repr(calls))
    print(copy.copy(x=Other(42)))
    try:
        copy.copy(Other(0))
    except ValueError as exc:
        print(str(exc))

main()
