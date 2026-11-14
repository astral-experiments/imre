from uuid import UUID


def number(value: UUID) -> int:
    return int(value.int)


truth = UUID(int=True)
print(number(truth), truth.int is True)
print(number(UUID(int=False)), number(UUID(int=41)), number(UUID(int=1 << 120)))
