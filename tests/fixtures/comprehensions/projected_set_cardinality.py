from dataclasses import dataclass


@dataclass
class Point:
    number: int


@dataclass
class Row:
    point: Point


def distinct(rows: list[Row]) -> int:
    return len({row.point.number for row in rows})


def integers(numbers: list[int]) -> int:
    return len({number for number in numbers})


@dataclass
class Counter:
    calls: int

    @property
    def observed(self) -> int:
        self.calls += 1
        if self.calls == 2:
            raise ValueError('second read')
        return 8


def read(counters: list[Counter]) -> int:
    return len({counter.observed for counter in counters})


def extend(numbers: list[int], number: int) -> int:
    if number < 3:
        numbers.append(number + 1)
    else:
        raise ValueError('appended element')
    return number


def mutate(numbers: list[int]) -> int:
    return len({extend(numbers, number) for number in numbers})


huge = 1 << 100
rows: list[Row] = []
print(distinct(rows))
rows.append(Row(Point(huge)))
print(distinct(rows))
rows.append(Row(Point(int(str(huge)))))
print(distinct(rows))
rows[0].point.number += 1
print(distinct(rows))
print(integers([huge, -huge, int(str(huge))]))

counter = Counter(0)
counters = [counter]
print(read(counters))
try:
    print(read(counters))
except ValueError as error:
    print(str(error))
print(counter.calls)

numbers = [1]
try:
    print(mutate(numbers))
except ValueError as error:
    print(str(error))
print(repr(numbers))
print(len({number for number in numbers if number > 10}))
