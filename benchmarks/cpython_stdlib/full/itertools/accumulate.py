from itertools import accumulate, batched, chain, count, cycle, islice, tee
from csv import reader, Error

def guarded(total: int, value: int) -> int:
    if value == 2:
        raise ValueError('refuse two')
    return total * 10 + value

items = [1, 2]
cursor = accumulate(items)
print(repr((iter(cursor) is cursor, next(cursor))))
items[1] = 4
items.append(7)
print(repr((list(cursor), next(cursor, -1))))
items.append(9)
print(next(cursor, -1))
source = count(10)
seeded = accumulate(source, initial=9)
print(repr((next(seeded), next(source), next(seeded), next(seeded))))
print(repr((list(accumulate([], initial=12)), list(accumulate([], initial=None)))))
print(repr((list(accumulate('AΩ𐀀')), list(accumulate([0.5, 1.25, -2.0])))))
print(list(accumulate([2**71, 2**71, -(2**72)])))
print(repr((list(accumulate([3, 1, 4, 0], min)), list(accumulate([3, 1, 4, 0], max)))))
filtered = accumulate([1, 2, 3, 4], guarded)
print(next(filtered))
try:
    next(filtered)
except ValueError as exc:
    print(str(exc))
print(list(filtered))
rows = reader(['A', '"bad"x', 'B', 'C'], strict=True)
joined = accumulate(rows)
print(next(joined))
try:
    next(joined)
except Error as exc:
    print(str(exc))
print(repr((next(joined), next(rows), next(joined, ['end']))))
invalid = accumulate([4, 5, 6], 2)
print(next(invalid))
try:
    next(invalid)
except TypeError as exc:
    print(str(exc))
try:
    next(invalid)
except TypeError as exc:
    print(str(exc))
print(next(invalid, -1))
print(list(islice(accumulate(count(1)), 5)))

def stop_two(total: int, value: int) -> int:
    if value == 2:
        raise StopIteration('paused')
    return total + value

stopped = accumulate([1, 2, 3], stop_two)
print(next(stopped))
try:
    next(stopped)
except StopIteration as exc:
    print(str(exc))
print(repr((next(stopped), next(stopped, -1))))
again = accumulate([1, 2, 3], stop_two)
print(repr((next(again), next(again, -1), next(again))))
as_list = accumulate([1, 2, 3], stop_two)
print(repr((list(as_list), next(as_list))))

# Each consumer follows its own rule for a source that raises StopIteration.
chained = accumulate([1, 2, 3], stop_two)
print(repr((list(chain(chained, [8])), next(chained))))
batched_source = accumulate([1, 2, 3], stop_two)
print(repr((list(batched(batched_source, 3)), next(batched_source, -1))))
cycle_source = accumulate([1, 2, 3], stop_two)
print(repr((list(islice(cycle(cycle_source), 4)), next(cycle_source))))
tee_source = accumulate([1, 2, 3], stop_two)
left, right = tee(tee_source)
print(repr((next(left), next(left, -1), list(left), list(right))))
slice_source = accumulate([1, 2, 3], stop_two)
sliced = islice(slice_source, 5)
print(next(sliced))
try:
    next(sliced)
except StopIteration as exc:
    print(str(exc))
print(repr((next(sliced, -1), next(slice_source))))


class Mutate:
    def __init__(self, values: list[int]) -> None:
        self.values = values

    def add(self, left: int, right: int) -> int:
        self.values[0] = 2**70
        return left + right


shared = [1]
changing = accumulate([3, 4, 5], Mutate(shared).add)
print(repr((sum(shared), next(changing), sum(shared))))
print(repr((next(changing), sum(shared))))
print(repr((list(changing), sum(shared))))
