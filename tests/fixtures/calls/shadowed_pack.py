def grow(row: list[int]) -> dict[str, int]:
    row.append(20)
    print("mapping", len(row))
    return {"size": len(row)}


def take(first: int, /, *values: int, **kw: int) -> int:
    print("take", first, len(values), kw["size"])
    return len(values)


def via(rows: list[list[int]], /, *items: int) -> list[int]:
    return [take(5, *items, **grow(items)) for items in rows]


print("result", via([[10]], 99))
