seen: list[object] = []


def record(value: object) -> int:
    seen.append(value)
    return 0


def constant_key(value: object) -> int:
    return 0


def shared_key(value):
    return 0


source: list[str] = 'omega:alpha:alpha'.split(':')
last = source[0]
first = source[1]
second = source[2]
keyed: list[object] = sorted(source, key=record, reverse=True)
assert len(seen) == 3
assert seen[0] is last and seen[1] is first and seen[2] is second
assert keyed[0] is last and keyed[1] is first and keyed[2] is second
alias = keyed
alias.append(1)
assert alias is keyed and len(keyed) == 4

assert first == second and first is not second

plain: list[object] = sorted([last, first, second])
assert plain[0] is first and plain[1] is second and plain[2] is last
distinct: list[object] = sorted({last, first})
assert distinct[0] is first and distinct[1] is last
pair: list[object] = sorted((last, first), key=None)
assert pair[0] is first and pair[1] is last
chars: list[object] = sorted('az', reverse=True)
assert chars[0] == 'z' and chars[1] == 'a'
empty_source: list[str] = []
empty: list[object] = sorted(empty_source)
assert len(empty) == 0


mixed: list[object] = sorted(['a', 1], key=constant_key)
assert len(mixed) == 2 and mixed[0] == 'a' and mixed[1] == 1
integers: list[object] = sorted([2, 1], key=shared_key)
texts: list[object] = sorted(['b', 'a'], key=shared_key)
assert len(integers) == 2 and len(texts) == 2
