from collections import Counter, defaultdict


def mark(events: list[int], event: int) -> int:
    events.append(event)
    return event


def incoming(events: list[int], source: dict[str, int]) -> dict[str, int]:
    events.append(1)
    return source


def change(events: list[int], source: dict[str, int]) -> int:
    events.append(2)
    source['x'] = 7
    source['added'] = 3
    return 9


def before_pull(rows: list[tuple[str, int]], events: list[int]) -> int:
    rows.append(('third', mark(events, 3)))
    return 4


def with_context() -> dict[str, list[int]]:
    return dict(first=[], second=[2])


def main() -> None:
    events: list[int] = []
    print(dict(first=mark(events, 1), second=mark(events, 2)))
    print(events)
    print(dict({}, first=11))
    source = {'x': 1, 'second': 2}
    log: list[int] = []
    result = dict(incoming(log, source), x=change(log, source), last=mark(log, 3))
    print(log)
    print(result)
    print(list(result))
    print(source)
    print(result is source)
    source['x'] = 20
    print(result['x'])
    print(dict(source.items(), second=9))
    print(with_context())

    counts = Counter(a=-5, b=0, c=5, d=10, e=15, g=40)
    print(dict(+counts))
    print(dict(-counts))
    plain = dict(counts)
    print(plain)
    print(counts['missing'])
    try:
        print(plain['missing'])
    except KeyError as error:
        print(error)
    groups: defaultdict[str, list[int]] = defaultdict(list)
    groups['a'].append(6)
    copied = dict(groups)
    copied['a'].append(7)
    print(groups['a'])
    print(copied['a'] is groups['a'])
    try:
        print(copied['b'])
    except KeyError as error:
        print(error)
    print('b' in groups)
    print('b' in copied)

    pairs = [('first', 1), ('second', 2), ('first', 3)]
    print(dict(pairs))
    print(dict((('z', 0), ('x', 2), ('z', 3))))
    order: list[int] = []
    made = dict(((name, mark(order, number)) for name, number in pairs),
                second=before_pull(pairs, order))
    print(made)
    print(order)
    print(dict(zip([1, 2, 1], ['a', 'b', 'c'])))
    print(dict({(1, 'a'): 3}))


main()
