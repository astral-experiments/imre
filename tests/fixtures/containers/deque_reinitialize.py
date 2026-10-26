from collections import deque


def receiver(events: list[int], d: deque[int]) -> deque[int]:
    events.append(1)
    return d


def source(events: list[int]) -> list[int]:
    events.append(2)
    return [8, 7, 9, 6]


def limit(events: list[int]) -> int:
    events.append(3)
    return 3


def pull(events: list[int], item: int) -> int:
    events.append(item)
    if item == 9:
        raise ValueError("third pull")
    return item


def fail_source(events: list[int]) -> list[int]:
    events.append(4)
    raise ValueError("source failed")


def show_bound(value: int | None):
    if value is None:
        print("None")
    else:
        print(value)


def main():
    d = deque([1, 2], 2)
    alias = d
    print(d.__init__(range(5)))
    print(alias is d)
    print(list(alias))
    show_bound(d.maxlen)
    print(d is not deque(list(d)))
    print(d.__init__(b"", maxlen=0))
    print(list(alias))
    show_bound(d.maxlen)
    print(d.__init__(iterable=[3, 4, 5], maxlen=2))
    print(list(alias))
    show_bound(d.maxlen)
    print(d.__init__(d))
    print(list(alias))
    show_bound(d.maxlen)
    print(d.__init__(maxlen=True))
    print(list(alias))
    show_bound(d.maxlen)
    d.extend([10, 11])
    print(list(alias))
    show_bound(alias.maxlen)
    events: list[int] = []
    try:
        receiver(events, d).__init__(maxlen=limit(events), iterable=(pull(events, x) for x in source(events)))
    except ValueError as error:
        print(str(error))
        print(events)
        print(list(alias))
        show_bound(alias.maxlen)
    events.clear()
    try:
        receiver(events, d).__init__(source(events), -1)
    except ValueError as error:
        print(str(error))
        print(events)
        print(list(alias))
        show_bound(alias.maxlen)
    try:
        d.__init__([1], 10**50)
    except OverflowError as error:
        print(str(error))
        print(list(alias))
        show_bound(alias.maxlen)
    try:
        d.__init__(None, 4)
    except TypeError as error:
        print(str(error))
        print(list(alias))
        show_bound(alias.maxlen)
    d.extend([2, 3])
    try:
        d.__init__(23, -2)
    except ValueError as error:
        print(str(error))
        print(list(alias))
        show_bound(alias.maxlen)
    try:
        d.__init__([], "3")
    except TypeError as error:
        print(str(error))
        print(list(alias))
        show_bound(alias.maxlen)
    try:
        d.__init__(maxlen=[])
    except TypeError as error:
        print(str(error))
        print(list(alias))
        show_bound(alias.maxlen)
    events.clear()
    try:
        receiver(events, d).__init__(source(events), limit(events), limit(events))
    except TypeError as error:
        print(str(error))
        print(events)
        print(list(alias))
        show_bound(alias.maxlen)
    events.clear()
    try:
        receiver(events, d).__init__(source(events), iterable=source(events))
    except TypeError as error:
        print(str(error))
        print(events)
        print(list(alias))
        show_bound(alias.maxlen)
    events.clear()
    try:
        receiver(events, d).__init__(bad=limit(events))
    except TypeError as error:
        print(str(error))
        print(events)
        print(list(alias))
        show_bound(alias.maxlen)
    events.clear()
    try:
        receiver(events, d).__init__(fail_source(events), maxlen=limit(events))
    except ValueError as error:
        print(str(error))
        print(events)
        print(list(alias))
        show_bound(alias.maxlen)
    print(d.__init__())
    print(list(alias))
    show_bound(alias.maxlen)
    text = deque("old", 5)
    text.__init__("£a£", 2)
    print(list(text))
    show_bound(text.maxlen)


main()
