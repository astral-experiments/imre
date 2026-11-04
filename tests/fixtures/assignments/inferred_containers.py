def ordered(text):
    values = set()
    for item in text:
        values.add(item)
    original = values
    values = sorted(values)
    return len(original), values


def numbers(data):
    found = set()
    for value in data:
        found.add(value)
    return len(found), sorted(found)


def packed(data):
    count, values = numbers(data)
    original = values
    values = bytes(values)
    print(count, original, values.hex())


print(ordered("éA界éaA"))
print(ordered(""))
packed(b"\x00baba\xff")
packed(b"")
