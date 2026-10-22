"""Exercise fixed fields, nominal values, method calls, and receiver lifetimes."""


class Record:
    number: int
    enabled: bool
    label: str
    empty: None
    items: list[int]

    def __init__(self, number: int, enabled: bool, label: str, empty: None, items: list[int]) -> None:
        self.number = number
        self.enabled = enabled
        self.label = label
        self.empty = empty
        self.items = items

    def bump(self, amount: int) -> int:
        self.number += amount
        return self.number

    def values(self) -> list[int]:
        return self.items

    def same(self: "Record") -> "Record":
        return self

    def redirect(self, other: "Record") -> "Record":
        self = other
        return self

    def set_label(self, value: str) -> str:
        self.label = value
        return self.label

    def accumulate(self, count: int) -> int:
        for value in range(count):
            self.number += value
        return self.number


class Envelope:
    record: Record
    title: str

    def __init__(self, record: Record, title: str) -> None:
        self.record = record
        self.title = title

    def get(self) -> Record:
        return self.record


class Empty:
    def __init__(self) -> None:
        pass

    def answer(self) -> int:
        return 42


def identity(value: Record) -> Record:
    return value


def make_envelope(number: int) -> Envelope:
    return Envelope(Record(number, True, "temporary", None, [number]), "outer")


def overwrite(value: Record) -> int:
    value.number = 7
    return 3


def reset_target(value: Record) -> Record:
    value.number = 9
    return value


def target(value: Record, log: list[int]) -> Record:
    log.append(2)
    return value


def rhs(value: Record, log: list[int]) -> int:
    log.append(1)
    value.number = 9
    return 5


def replace_record(value: Envelope) -> int:
    value.record = Record(11, False, "replacement", None, [11])
    return 4


def first(value: int, ignored: int) -> int:
    return value


def first_item(items: list[int], ignored: list[int]) -> int:
    return items[0]


def optional_field_rebind() -> None:
    record: Record | None = Record(1, True, "original", None, [31])
    if record is not None:
        # The second argument drops the only owner of the original record.
        print(first_item(record.items, (record := Record(2, False, "new", None, [7])).items))
        print(record.items[0])

    envelope = make_envelope(37)
    print(bool(envelope.record.items))
    print(len(envelope.record.items))
    # A sibling argument replaces the whole nested record after its field is read.
    print(first_item(envelope.record.items, [replace_record(envelope)]))
    print(envelope.record.items[0])


def published_accumulator(rows: list[Record]) -> int:
    fresh = Record(1, False, "fresh", None, [41])
    rows.append(fresh)
    for row in rows:
        original = row.items
        fresh.items = [99]
        return original[0]
    return -1


def main() -> None:
    rows: list[Record] = []
    print(published_accumulator(rows))
    print(rows[0].items[0])
    optional_field_rebind()
    value = Record(1000000000000000000000000000000, True, "λ record", None, [1, 2])
    print(value.number)
    print(value.enabled)
    print(value.label)
    print(value.empty)
    print(value.items[1])
    print(value.set_label("changed"))

    # A returned nominal value aliases the same mutable object.
    alias = identity(value.same())
    alias.number = 1000000000000000000000000000001
    print(value.number)
    alias.items.append(3)
    print(len(value.items))
    saved = value.number
    print(first(value.number, overwrite(value)))
    print(saved)
    print(value.number)

    # Augmented assignment keeps the old BigInt alive across a field replacement.
    value.number = 1000000000000000000000000000000
    value.number += overwrite(value)
    print(value.number)

    # The RHS field value must survive replacement while its target is evaluated.
    value.number = 1000000000000000000000000000000
    reset_target(value).number = value.number
    print(value.number)

    # Ordinary assignment evaluates RHS then receiver; augmented assignment reverses them.
    log: list[int] = []
    target(value, log).number = rhs(value, log)
    print(log[0])
    print(log[1])
    print(value.number)
    log = []
    value.number = 1000000000000000000000000000000
    target(value, log).number += rhs(value, log)
    print(log[0])
    print(log[1])
    print(value.number)

    # A method receiver acquired from a field must survive later argument evaluation.
    envelope = make_envelope(1000000000000000000000000000000)
    print(envelope.record.bump(replace_record(envelope)))
    print(envelope.record.number)
    print(envelope.record.enabled)
    print(envelope.get().label)

    envelope = make_envelope(1000000000000000000000000000000)
    envelope.record.number += replace_record(envelope)
    print(envelope.record.number)

    # Returned fields outlive temporary receivers, including nested objects and lists.
    held = make_envelope(1000000000000000000000000000000).get()
    print(held.number)
    values = Record(0, False, "list owner", None, [1000000000000000000000000000000]).values()
    values.append(2)
    print(values[0])
    print(values[-1])
    print(make_envelope(17).get().number)
    print(Record(0, True, "loop", None, []).accumulate(5))
    print(Empty().answer())
    print(Record(1, True, "receiver", None, []).redirect(Record(23, False, "result", None, [])).number)
    print(Record(29, True, "returned receiver", None, []).same().number)
    print(alias == value)
    print(Record(0, True, "different", None, []) != value)


main()
