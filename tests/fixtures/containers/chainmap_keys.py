from collections import ChainMap, Counter, defaultdict


def show(value: int | None) -> None:
    if value is None:
        print('None')
    else:
        print(value)


def layer(log: list[int], owner: ChainMap[str, int]) -> ChainMap[str, int]:
    log.append(1)
    return owner


def key(log: list[int]) -> str:
    log.append(2)
    return 'same'


def logged_value(log: list[int]) -> int:
    log.append(3)
    return 17


first = {'same': 10}
last = {'same': 20, 'late': 30}
chain = ChainMap(first, last)
print(chain['same'])
print(chain['late'])
print('late' in chain)
print('absent' not in chain)
show(chain.get('same'))
print(chain.get('late', 99))
show(chain.get('absent'))
print(chain.get('absent', 99))
try:
    print(chain['absent'])
except KeyError as exc:
    print(str(exc))
chain['late'] = 40
print(first)
print(last)
print(chain['late'])
first.clear()
print(chain['late'])
original = chain.maps
chain.maps = []
show(chain.get('old'))
print('old' in chain)
try:
    chain['x'] = 1
except IndexError as exc:
    print(str(exc))
try:
    print(chain['x'])
except KeyError as exc:
    print(str(exc))
chain.maps = original
log: list[int] = []
layer(log, chain)[key(log)] = logged_value(log)
print(log)
print(first)
log = []
print(layer(log, chain).get(key(log), logged_value(log)))
print(log)
log = []
print(layer(log, chain).get(default=logged_value(log), key=key(log)))
print(log)
log = []
print(layer(log, chain).get(key(log), default=logged_value(log)))
print(log)
log = []
show(chain.get(key='absent', default=None))
print(key(log) in layer(log, chain))
print(log)

# Counter returns zero for a missing subscription without inserting a key.
counts = Counter('ab')
count_layers = ChainMap(counts, {'only_later': 29})
print(count_layers['only_later'])
print(count_layers['nowhere'])
print('only_later' in count_layers)
print('nowhere' in count_layers)
print(count_layers.get('only_later', 33))
print(count_layers.get('nowhere', 33))
print('only_later' in counts)
print('nowhere' in counts)
count_layers['new'] = 5
print(counts['new'])

# A default factory must run even when a later layer has the requested key,
# but get() with no matching key anywhere must leave the factory untouched.
defaults: defaultdict[str, list[int]] = defaultdict(list)
later = {'present': [5]}
lists = ChainMap(defaults, later)
print(lists.get('absent') is None)
print('absent' in defaults)
fallback = [8]
print(lists.get('absent', fallback) is fallback)
found = lists.get('present', fallback)
print(found)
print(found is defaults['present'])
print(later['present'])
found.append(7)
print(lists['present'])
print(later['present'])
fresh = lists['new']
print(fresh)
print(fresh is defaults['new'])
print('new' in lists)

# A present null or false value is not a miss; keys retain their original
# Python representation in KeyError even when the public layer list is empty.
nullable: dict[int, int | None] = {1: None, 2: 0}
numbers = ChainMap(nullable)
show(numbers.get(1, 99))
show(numbers.get(2, 99))
show(numbers.get(3, 99))
numbers.maps = []
try:
    show(numbers[123])
except KeyError as exc:
    print(str(exc))
