def add(first: int, second: int = 2) -> int:
    return first + second
print(add(**{str("first"): 1}))
