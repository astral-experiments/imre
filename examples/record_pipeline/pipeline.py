from typing import Callable

from .model import Record


class RiskPolicy:
    limit: int

    def __init__(self, limit: int) -> None:
        self.limit = limit

    def accepts(self, record: Record) -> bool:
        return record.risk <= self.limit


class FeePolicy:
    fee: int

    def __init__(self, fee: int) -> None:
        self.fee = fee

    def apply(self, record: Record) -> int:
        return record.amount - self.fee


def gross_amount(record: Record) -> int:
    return record.amount


def select_transform(mode: str, fee: int) -> Callable[[Record], int]:
    if mode == "gross":
        return gross_amount
    if mode == "net":
        policy = FeePolicy(fee)
        return policy.apply
    raise ValueError("unknown payout mode")


def select_predicate(limit: int) -> Callable[[Record], bool]:
    policy = RiskPolicy(limit)
    return policy.accepts


def summarize(
    records: list[Record],
    accept: Callable[[Record], bool],
    transform: Callable[[Record], int],
    positive: Callable[[int], int],
) -> tuple[int, int, int]:
    count = 0
    total = 0
    largest = 0
    for amount in filter(None, map(positive, map(transform, filter(accept, records)))):
        count += 1
        total += amount
        if amount > largest:
            largest = amount
    return count, total, largest
