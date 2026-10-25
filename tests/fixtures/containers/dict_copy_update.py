from collections import Counter, defaultdict


def update_counts(target: dict[str, int], source: dict[str, int]) -> None:
    target.update(source, last=3)


def receiver(events: list[int], target: dict[str, int]) -> dict[str, int]:
    events.append(1)
    return target


def observe(events: list[int], target: dict[str, int], event: int) -> int:
    events.append(event)
    return target['a']


def incoming(events: list[int], target: dict[str, int]) -> dict[str, int]:
    events.append(2)
    return {'a': 10, 'second': target['a']}


def stop() -> int:
    raise ValueError("no update")


def main() -> None:
    original = {'music': 'bach', 'art': 'rembrandt'}
    combined = original.copy()
    print(combined.update({'art': 'van gogh', 'opera': 'carmen'}))
    print(list(combined))
    print(combined)
    print(original)
    print(combined is original)
    print(combined.update(combined))
    print(combined.update({}))
    print(combined.update())
    print(combined)

    events: list[int] = []
    counts = {'a': 4, 'last': 2}
    receiver(events, counts).update(incoming(events, counts),
                                    a=observe(events, counts, 3),
                                    final=observe(events, counts, 4))
    print(events)
    print(counts)
    try:
        counts.update({'a': 0}, extra=stop())
    except ValueError as error:
        print(error)
    print(counts)
    counter = Counter('aab')
    update_counts(counts, counter)
    print(counts)
    update_counts(counter, counter)
    print(counter)
    copied = counter.copy()
    copied['a'] = 0
    print(copied['unseen'])
    print(len(copied))
    print(counter)

    nested = {'first': [1], 'second': [2]}
    copied_rows = nested.copy()
    copied_rows['first'].append(3)
    print(copied_rows['first'] is nested['first'])
    print(nested)
    del copied_rows['first']
    nested.update(copied_rows, third=copied_rows['second'])
    print(nested)
    print(nested['third'] is copied_rows['second'])
    groups: defaultdict[str, list[int]] = defaultdict(list)
    groups.update(nested)
    group_copy = groups.copy()
    group_copy['missing'].append(5)
    print('missing' in groups)
    print(group_copy['missing'])
    print(groups['first'] is nested['first'])
    print(group_copy['first'] is groups['first'])

    binary = {b'x': 1}
    binary.update({b'y': 2, b'x': 9})
    print(binary)
    pairs = {(2, 4): 'even', (1, 3): 'odd'}
    changed = pairs.copy()
    changed.update({(2, 4): 'both', (0, 0): 'zero'})
    print(changed)
    print(pairs)


main()
