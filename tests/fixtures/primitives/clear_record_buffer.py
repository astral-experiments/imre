from logging import LogRecord, makeLogRecord


def clear_records(records: list[LogRecord]) -> None:
    records.clear()


def clear_during_iteration(values: list[int]) -> None:
    saved = values
    for value in saved:
        print(value)
        values.clear()
    print(len(saved))


def alias(items: list[str]) -> list[str]:
    print("get receiver")
    return items


records: list[LogRecord] = []
first = makeLogRecord({"name": "first", "msg": "one"})
second = makeLogRecord({"name": "second", "msg": "two"})
records.append(first)
records.append(second)
again = records
clear_records(again)
print(again is records)
print(len(records))
records.append(second)
print(again[0] is second)
print(list.clear(records) is None)
print(len(again))

numbers = [1, 1 << 100, 3, 4]
clear_during_iteration(numbers)
numbers.append(12)
print(numbers[0])
labels = ["a", "b", "c", "d", "e"]
same = labels
alias(labels).clear()
print(len(same))
print(labels.clear() is None)
labels.append("after")
print(same[0])
nested: list[list[int]] = [numbers, numbers]
list.clear(nested)
print(len(nested))
print(numbers[0])
