huge = 1 << 120
print(sum([]))
print(sum((), start=7))
print(sum([1, -2, 3]))
print(sum([1, -2, 3], 9))
print(sum([1, -2, 3], start=-9))
print(sum((4, 5, 6)))
print(sum(divmod(1000, 256)))
print(sum(range(1, 9, 2)))
print(sum(range(7, -2, -2)))
print(sum(range(0)))
print(sum(range(huge, huge + 3), -huge))
print(sum(b"\x00\x80\xff"))
print(sum({2: "two", 5: "five"}))
print(sum({2, 5, 7}))
print(sum([], huge))
print(sum([huge, -huge, huge], huge))
print(sum(list(range(20))))
values: list[int] = []
total = 0
for turn in range(24):
    values = [huge, -huge, huge]
    total = sum(values, huge)
print(total)
print(values[0])
