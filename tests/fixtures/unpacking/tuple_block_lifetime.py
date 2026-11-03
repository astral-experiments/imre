def build(count: int) -> list[tuple[int, int]]:
    rows: list[tuple[int, int]] = []
    for i in range(count):
        rows.append((i, i + 2**66))
    return rows


rows = build(65)
other = build(17)
first = rows[0]
last = rows[64]
assert first == rows[0]
assert first == other[0]
rows.clear()
other.clear()
print(first[1])
print(last[1])

kept: list[tuple[int, str]] = []
for text in ["2", "bad", "4"]:
    try:
        kept.append((int(text), text))
    except ValueError:
        print("invalid")
print(kept[0][0])
print(kept[1][1])

# A late conversion failure must release the earlier owned heap integer.
for text in ["bad", "2"]:
    try:
        pair = (2**90 + len(text), int(text))
        print(pair[0])
    except ValueError:
        print("late conversion")

# Both records retain the same mutable list borrowed from a local.
shared = [3]
pairs = [(shared, 1), (shared, 2)]
shared.append(8)
print(pairs[0][0][1] + pairs[1][0][1])
