"""Set results never depend on iteration order; mutation cases use singletons."""

# ruff: noqa: PLC0208
# Direct iteration over set literals is part of the language coverage.


class Holder:
    numbers: set[int]
    names: set[str]

    def __init__(self, numbers: set[int], names: set[str]) -> None:
        self.numbers = numbers
        self.names = names


class MaybeHolder:
    numbers: set[int] | None

    def __init__(self, numbers: set[int] | None) -> None:
        self.numbers = numbers


def total(values: set[int]) -> int:
    result = 0
    for value in values:
        result += value
    return result


def text_size(values: set[str]) -> int:
    result = 0
    for value in values:
        result += len(value)
    return result


def empty() -> set[str]:
    return set()


def maybe_set(flag: bool) -> set[int] | None:
    if flag:
        return {7}
    return None


def literal_item(log: list[int], marker: int) -> str:
    log.append(marker)
    return "duplicate owned string literal value"


def basics() -> None:
    huge = 100000000000000000000000000000000000000000000000001
    values = {huge, huge + 0, -huge, -1, 0}
    print(len(values))
    print(total(values))
    print(huge + 0 in values)
    print(1 not in values)
    print(values.add(huge + 0))
    print(values.discard(2))
    print(len(values))
    alias = values
    alias.discard(huge + 0)
    alias.add(2)
    print(total(values))
    print(len(values))

    names = {"é", "e\u0301", "a\0b", "é"}  # noqa: B033 - literal duplicates must deduplicate.
    print(len(names))
    print(text_size(names))
    print("a\0b" in names)
    print("a" not in names)
    names.discard("é")
    names.add("é")
    print(text_size(names))
    print(len(empty()))
    print(total(set()))

    log: list[int] = []
    duplicates = {literal_item(log, 1), literal_item(log, 2), literal_item(log, 3)}
    print(len(duplicates))
    for event in log:
        print(event)


def containers() -> None:
    groups: list[set[int]] = [set(), {1}]
    groups[0].add(2)
    old = groups[1]
    groups[1] = {9}
    old.add(3)
    print(total(old))
    print(total(groups[0]))
    repeated = [old] * 2
    repeated[0].add(4)
    print(total(repeated[1]))
    popped = repeated.pop()
    repeated[0] = set()
    print(total(popped))

    mapped: dict[str, set[str]] = {"names": {"retained name"}}
    kept = mapped["names"]
    mapped["names"] = set()
    print(text_size(kept))
    fallback = mapped.get("missing", set())
    fallback.add("fallback")
    print(text_size(fallback))
    found = mapped.get("names")
    mapped["names"] = {"new"}
    if found is not None:
        found.add("detached")
        print(text_size(found))
    for key, value in mapped.items():
        mapped[key] = set()
        print(text_size(value))

    pair: tuple[set[int], str] = ({5}, "pair")
    numbers = pair[0]
    pair = (set(), "rebound")
    print(total(numbers))
    holder = Holder(numbers, kept)
    holder.numbers.add(6)
    print(total(numbers))
    holder.names = {"replacement"}
    print(text_size(kept))

    optional: set[int] | None = maybe_set(True)
    if optional is not None:
        retained = optional
        optional = None
        retained.add(8)
        print(total(retained))
    print(optional is None)
    optional = maybe_set(False)
    print(optional is None)
    possible: list[set[int] | None] = [None, {9}, set()]
    for possible_set in possible:
        if possible_set is None:
            print("absent")
        else:
            possible_set.add(10)
            print(total(possible_set))
    maybe_holder = MaybeHolder({11})
    snapshot = maybe_holder.numbers
    maybe_holder.numbers = None
    if snapshot is not None:
        snapshot.add(12)
        print(total(snapshot))


def take_number(values: set[int]) -> int:
    for value in values:
        values.discard(value)
        return value
    return 0


def take_text(values: set[str]) -> str:
    for value in values:
        values.discard(value)
        return value
    return "empty"


def find(values: set[int], wanted: int) -> int | None:
    for value in values:
        if value == wanted:
            return value
    return None


def iteration() -> None:
    print(take_number({100000000000000000000000000000000000000000000000003}))
    print(take_text({"a long owned string surviving removal and iterator cleanup"}))
    only = {100000000000000000000000000000000000000000000000005}
    for value in only:
        only.discard(value)
        print(value)
        break
    print(len(only))
    values = {1}
    alias = values
    for value in values:
        alias.add(value)
        alias.discard(99)
        print(value)

    result = 0
    visits = 0
    for outer in {1, 2, 3}:
        if outer == 2:
            continue
        for inner in {4, 5}:
            result += outer * inner
            visits += 1
    print(result)
    print(visits)
    for outer in {1, 2, 3}:
        for inner in {10}:
            result += outer + inner
            break
    print(result)
    for value in {1, 2}:
        for value in {10}:
            result += value
    print(result)
    found = find({1, 2, 3}, 2)
    if found is not None:
        print(found)
    print(find({1, 2, 3}, 4) is None)

    # Rebinding the field must not replace the captured source mid-iteration.
    holder = Holder({1, 2, 3}, set())
    result = 0
    for value in holder.numbers:
        result += value
        holder.numbers = {99}
    print(result)
    print(total(holder.numbers))


def membership_key(holder: Holder, log: list[int]) -> int:
    log.append(1)
    holder.numbers = {1}
    return 1


def receiver(holder: Holder, log: list[int]) -> set[int]:
    log.append(2)
    return holder.numbers


def argument(holder: Holder, log: list[int]) -> int:
    log.append(3)
    holder.numbers = {9}
    return 1


def changed_start(holder: Holder) -> int:
    holder.numbers.add(3)
    holder.numbers = {99}
    return 4611686018427387903


def evaluation_order() -> None:
    log: list[int] = []
    holder = Holder({0}, set())
    print(membership_key(holder, log) in receiver(holder, log))
    saved = holder.numbers
    receiver(holder, log).discard(argument(holder, log))
    print(len(saved))
    print(total(holder.numbers))
    # These receivers lose their only outside root while the argument runs.
    receiver(holder, log).add(argument(holder, log))
    receiver(holder, log).discard(argument(holder, log))
    print(total(holder.numbers))
    for event in log:
        print(event)

    holder.numbers = {1, 2}
    sum_indexes = 0
    sum_values = 0
    count = 0
    for index, value in enumerate(holder.numbers, changed_start(holder)):
        sum_indexes += index
        sum_values += value
        count += 1
        index = -99
        continue
    print(sum_indexes)
    print(sum_values)
    print(count)
    print(total(holder.numbers))
    sum_indexes = 0
    sum_lengths = 0
    for pair in enumerate({"a", "bb"}, -1000000000000000000000000000000):
        sum_indexes += pair[0]
        sum_lengths += len(pair[1])
    print(sum_indexes)
    print(sum_lengths)
    empty_names: set[str] = set()
    index = 123
    text = "unchanged"
    for index, text in enumerate(empty_names):
        print("unreachable")
    print(index)
    print(text)
    empty_numbers: set[int] = set()
    value = 321
    for value in empty_numbers:
        print("unreachable")
    print(value)


basics()
containers()
iteration()
evaluation_order()
