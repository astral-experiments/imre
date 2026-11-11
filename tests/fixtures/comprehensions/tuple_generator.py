def even(value: int) -> bool:
    print(value)
    return value % 2 == 0


print(tuple(even(value) for value in (0, 1, 2) if value != 1))
print(tuple(even(value) for value in range(0)))
