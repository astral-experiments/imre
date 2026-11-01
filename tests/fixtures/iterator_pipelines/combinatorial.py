from itertools import combinations, combinations_with_replacement, permutations, product


def exercise(width: int) -> None:
    pool = [1, 2]
    rows = product(pool, "ab")
    pool.append(3)
    other = iter(rows)
    print(next(rows))
    print(next(other))
    print(list(rows))
    print(list(other))
    print(list(product()))
    print(list(product('', repeat=0)))
    print(list(product('ab', '')))
    print(list(map(tuple, "Ax Å🙂 Bx".split())))
    print(tuple("🙂"))
    repeated = product(range(2), repeat=80)
    print(next(repeated))
    print(list(product('ab', repeat=width)))
    print(list(permutations('aba', width)))
    print(list(permutations('', None)))
    print(list(permutations('ab', 3)))
    print(list(permutations('abc', 0)))
    print(list(combinations('abcd', 2)))
    print(list(combinations_with_replacement('abc', 2)))


exercise(2)
