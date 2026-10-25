from collections import Counter, defaultdict


class Local:
    def clear(self) -> str:
        return "shadowed"


def shadow(dict: Local) -> str:
    return dict.clear()


def gather(mapping: dict[str, int], text: str) -> list[int]:
    return [mapping[c] for c in text]


counter = Counter('abba')
alias = counter
print(counter.clear())
print(alias)
print(alias['a'])
alias['z'] = 1 << 100
print(counter['z'])
print(dict.clear(alias))
print(len(counter))
Counter.clear(counter)
print(len(alias))

groups: defaultdict[str, list[int]] = defaultdict(list)
previous = groups['key']
previous.append(7)
print(groups.clear())
groups['key'].append(9)
print(previous)
print(groups['key'])
defaultdict.clear(groups)
print(len(groups))

values = {1: 10, 2: 20, 3: 30}
seen: list[int] = []
for key in values:
    seen.append(key)
    if key == 1:
        values.clear()
        values[4] = 40
        values[5] = 50
        values[6] = 60
print(seen)
print(list(values))

# Keep an iterator more than a table's width past the shorter refill, then
# force compaction before the next iteration checks its original size.
values = {index: index for index in range(128)}
try:
    for key in values:
        if key == 100:
            values.clear()
            for index in range(40):
                values[index] = index
            for index in range(37):
                del values[index]
except RuntimeError as error:
    print(error)
print(list(values))
print(shadow(Local()))

# A fused character lookup borrows the mapping's integer words. The saved
# result owns its old values; a new lookup must see the cleared table.
letters = {'a': 1 << 100}
saved = gather(letters, 'aa')
letters.clear()
try:
    gather(letters, 'a')
except KeyError as error:
    print(error)
letters['a'] = 8
print(gather(letters, 'aa'))
print(saved)
