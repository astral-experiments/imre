def labels(events: list[int]) -> list[str]:
    events.append(1)
    return ["a'b", 'c"d', "x\ny", "é🌍"]


events: list[int] = []
print(str(labels(events)))
print(len(events))
print(str([1, 2, -3]))
print(str([["a"], ["b", "c"]]))
