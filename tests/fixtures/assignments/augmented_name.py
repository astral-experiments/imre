def extend(values: list[str]) -> list[str]:
    values += (values := ["rhs"])
    return values


original = ["old"]
result = extend(original)
print(result is original, original)
