optional: list[int | None] = list((1, None, 3))
for item in optional:
    if item is None:
        print("none")
    else:
        print(item)

first = [1]
second = ["two"]
for label, item in ((0, first), ("right", second)):
    print(label, item is first, item is second)
