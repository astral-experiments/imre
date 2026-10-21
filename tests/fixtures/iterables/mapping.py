for value in list(map(int, "١２𝟛")):
    print(value)
for value in list(map(int, [1.9, -2.9, 0.0])):
    print(value)
for value in list(map(int, [True, False, True])):
    print(value)
for value in list(map(int, (1, "2", 3.75, True))):
    print(value)
for value in list(map(int, b"\x00\x80\xff")):
    print(value)
for value in list(map(int, {"12": 1, "34": 2})):
    print(value)
print(sum(map(int, {"12", "34"})))
print(sum(map(int, (1, "2", 3.75, True))))
print(sum(map(int, range(4)), start=5))
empty: list[str] = []
print(len(list(map(int, empty))))
print(sum(map(int, empty), 8))

huge = 1 << 100
converted: list[int] = []
total = 0
for turn in range(24):
    converted = list(map(int, [huge, -huge, huge]))
    total = sum(map(int, [str(huge), str(-huge), str(huge)]), huge)
print(converted[0])
print(converted[1])
print(converted[2])
print(total)

for text in ["1x", "2²", "3\t", "4\u0301", "5'"]:
    for turn in range(8):
        try:
            failed = list(map(int, text))
        except ValueError as error:
            if turn == 7:
                print(error)
