from collections import ChainMap, Counter


def mapping(log: list[int], tag: int) -> dict[int, int]:
    log.append(tag)
    return {tag: tag * 10}


def owner(log: list[int], value: ChainMap[int, int]) -> ChainMap[int, int]:
    log.append(5)
    return value


def replacement(log: list[int]) -> list[dict[int, int]]:
    log.append(4)
    return [{9: 90}]


left = ChainMap()
right = ChainMap()
print(left.maps)
print(bool(left))
print(left is right)
print(left.maps is right.maps)
left.maps[0]['a'] = 2
print(left.maps)
print(right.maps)
print(bool(left))
print(bool(ChainMap({}, {})))
print(bool(ChainMap({1: 2}, {})))
print(bool(ChainMap({}, {1: 2})))

first = {1: 10}
last = {2: 20}
chain = ChainMap({}, first, last)
alias = chain
print(repr(chain))
print(alias is chain)
print(alias is chain.maps)
maps = chain.maps
print(maps is chain.maps)
print(maps[1] is first)
print(maps[2] is last)
first[3] = 30
maps[0][4] = 40
print(alias.maps)
maps.pop()
print(chain.maps)
print(last)
chain.maps = []
print(chain.maps)
print(maps)
print(bool(alias))
print(repr(alias))
chain.maps = maps
print(alias.maps is maps)
print(bool(alias))
log: list[int] = []
created = ChainMap(mapping(log, 1), {}, mapping(log, 2))
print(log)
print(created.maps)
owner(log, created).maps = replacement(log)
print(log)
print(created.maps)
typed: ChainMap[int, int] = ChainMap()
typed.maps[0][1] = 11
print(typed.maps)
projected: list[dict[int, str]] = ChainMap().maps
projected[0][2] = 'two'
print(projected)
counts = Counter('aba')
layers = ChainMap(counts)
counts['b'] = 7
print(repr(layers))
print(layers.maps[0]['b'])
print(layers.maps[0]['absent'])
print('absent' in counts)
