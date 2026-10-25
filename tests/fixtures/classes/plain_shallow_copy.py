from copy import copy
from dataclasses import dataclass
from functools import cached_property


def record_construction(log: list[int]) -> int:
    log.append(1)
    return 1000000000000000000000000000000000007


class Base:
    log: list[int]
    payload: list[int]
    number: int
    label: str
    optional: str | None
    ratio: float

    def __init__(self, log: list[int], payload: list[int], optional: str | None) -> None:
        self.log = log
        self.payload = payload
        self.number = record_construction(log)
        self.label = "λ"
        self.optional = optional
        self.ratio = 2.5

    def extra(self) -> list[int]:
        return self.payload

    @cached_property
    def cached(self) -> list[int]:
        self.log.append(3)
        return [self.number]


class Derived(Base):
    other: list[int]

    def __init__(self, log: list[int], payload: list[int], other: list[int]) -> None:
        super().__init__(log, payload, None)
        self.other = other

    def extra(self) -> list[int]:
        return self.other


@dataclass(frozen=True)
class Frozen:
    value: list[int]


@dataclass
class Envelope:
    child: Frozen
    mapping: dict[str, list[int]]


class Empty:
    pass


def snapshot(source: Base) -> Base:
    return copy(source)


def main() -> None:
    log: list[int] = []
    elems = [1]
    extra = [4]
    source = Derived(log, elems, extra)
    cold = snapshot(source)
    print(cold is not source)
    print(cold.extra() is extra)
    print(cold.payload is elems)
    print(repr(log))
    print(cold.cached is source.cached)
    hot = snapshot(source)
    print(hot.cached is source.cached)
    print(repr(log))
    hot.payload.append(9)
    hot.number = 8
    print(repr(source.payload))
    print(source.number)
    print(hot.number)
    print(hot.label)
    print(hot.ratio)
    print(hot.optional is None)
    present = Base(log, elems, "present")
    print(repr(copy(present).optional))
    frozen = Frozen(elems)
    frozen_copy = copy(frozen)
    print(frozen_copy is not frozen)
    print(frozen_copy.value is elems)
    envelope = copy(Envelope(Frozen(elems), {"key": elems}))
    print(envelope.child.value is elems)
    print(envelope.mapping["key"] is elems)
    empty = Empty()
    print(copy(empty) is not empty)

main()
