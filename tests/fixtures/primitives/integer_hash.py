def base_hash(value: int) -> int:
    return int.__hash__(value)


def bound_hash(value: int | bool) -> int:
    return value.__hash__()


modulus = (1 << 61) - 1
for value in [0, 41, -1, modulus, -modulus - 1, 1 << 130, -(1 << 130)]:
    print(base_hash(value))
    print(value.__hash__())
print(int.__hash__(True))
print(bound_hash(False))
print(bound_hash(1 << 130))
