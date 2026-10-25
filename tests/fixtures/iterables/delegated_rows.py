from collections import Counter
from collections.abc import Iterator
from dataclasses import dataclass, field


@dataclass
class Changes:
    first: Counter = field(default_factory=Counter)
    second: Counter = field(default_factory=Counter)

    def by_rule(self) -> Iterator[tuple[str, int]]:
        print("first pull")
        totals = Counter()
        totals.update(self.first)
        totals.update(self.second)
        yield from totals.items()

    def add(self) -> bool:
        print("reverse argument")
        self.second["added"] += 5
        return True


def optional(value: int) -> str | None:
    print(value)
    if value == 0:
        return None
    if value < 0:
        return ""
    return str(value)


cause = StopIteration("from guard")


class GuardedRows:
    def __iter__(self) -> Iterator[str]:
        fail_guard()
        yield from ["row"]


def fail_guard() -> None:
    raise cause


def get_rows(fail: bool) -> GuardedRows:
    if fail:
        raise cause
    return GuardedRows()


def iterator_errors() -> None:
    rows = get_rows(False)
    for sorting in [False, True]:
        try:
            if sorting:
                print(sorted(rows, key=lambda row: row[1:]))
            else:
                for row in rows:
                    print(row)
        except RuntimeError as error:
            print(str(error))
            print(error.__cause__ is cause)
    try:
        for row in get_rows(True):
            print(row)
    except StopIteration as error:
        print(error is cause)


def main() -> None:
    changes = Changes()
    changes.first["tie-one"] += 1
    changes.first["tie-two"] += 1
    changes.second["twice"] += 2
    for key, total in sorted(changes.by_rule(), key=lambda item: item[1], reverse=changes.add()):
        print(f"{key} {total}")
    rows: list[str] = list(filter(None, (optional(n) for n in [0, 1, -1, 2])))
    print(",".join(rows))
    table = []
    table.append("heading")
    table.extend(rows)
    print("|".join(table))
    iterator_errors()


main()
