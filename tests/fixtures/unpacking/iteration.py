# ruff: noqa: PLW0120
# Successful unpacking must reach the loop's exhaustion suite.

rows = [[1, 2, 3], [4], [5, 6]]
for first, *rest in rows:
    print(first)
    print(len(rest))
else:
    print("exhausted")

totals = [first + len(rest) for first, *rest in rows]
for total in totals:
    print(total)
headings = {first: len(rest) for first, *rest in rows}
for key, value in headings.items():
    print(key)
    print(value)
selected = {first for first, *rest in rows if rest}
print(len(selected))
print(1 in selected)
print(5 in selected)

pairs = [[10, 11], [12, 13]]
values = [left + right for left, right in pairs]
for value in values:
    print(value)
tuple_values = [first for first, *rest in [(14, 15)]]
print(tuple_values[0])

for alpha, beta in ["ab", "é🙂"]:
    print(alpha)
    print(beta)
byte_sums = [left + right for left, right in [b"AB", b"\x00\xff"]]
for total in byte_sums:
    print(total)

for index, (first, *rest) in enumerate(rows):
    print(index)
    print(first)
    print(len(rest))

rest = [90]
private = [first for first, *rest in rows]
print(rest[0])
print(len(private))
