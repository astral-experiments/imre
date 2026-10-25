from collections import deque


def receiver(events: list[int], values: deque[int]) -> deque[int]:
    events.append(1)
    return values


def argument(events: list[int]) -> int:
    events.append(2)
    return 8


def failing(events: list[int]) -> int:
    events.append(3)
    raise ValueError("argument failed")


def main():
    alias = deque([1, 2, 3, 4], 4)
    values = alias
    saved = values.reverse
    values = deque([10 ** 80, -20])
    print(saved())
    print(list(alias))
    print(list(values))
    print(alias.maxlen == 4)
    print(deque.reverse(alias))
    print(list(alias))
    alias.append(5)
    alias.append(6)
    alias.reverse()
    print(list(alias))
    print(alias.maxlen == 4)
    print(values.reverse())
    print(list(values))

    forward = deque([1, 2, 3, 4])
    seen: list[int] = []
    for item in forward:
        seen.append(item)
        if len(seen) == 1:
            forward.reverse()
    print(seen)
    backward = deque([1, 2, 3, 4])
    seen.clear()
    for item in reversed(backward):
        seen.append(item)
        if len(seen) == 1:
            backward.reverse()
    print(seen)
    empty = deque([1], 0)
    print(empty.reverse())
    print(len(empty))
    single = deque(["α"])
    print(single.reverse())
    print(list(single))

    row = [1]
    other = [2]
    rows = deque([row, row, other])
    rows.reverse()
    items = list(rows)
    print(items[0] is other)
    print(items[1] is row)
    items[1].append(9)
    print(items[2])

    events: list[int] = []
    try:
        receiver(events, alias).reverse(argument(events))
    except TypeError as exc:
        print(str(exc))
    print(events)
    try:
        receiver(events, alias).reverse(bad=failing(events))
    except ValueError as exc:
        print(str(exc))
    print(events)
    try:
        deque.reverse(alias, argument(events), unused=argument(events))
    except TypeError as exc:
        print(str(exc))
    print(events)
    print(list(alias))
    try:
        deque.reverse()
    except TypeError as exc:
        print(str(exc))
    try:
        deque.reverse([], argument(events))
    except TypeError as exc:
        print(str(exc))
    print(events)
    try:
        deque.reverse({})
    except TypeError as exc:
        print(str(exc))
    try:
        deque.reverse(None)
    except TypeError as exc:
        print(str(exc))
    try:
        list.reverse(alias)
    except TypeError as exc:
        print(str(exc))
    print(list(alias))


main()
