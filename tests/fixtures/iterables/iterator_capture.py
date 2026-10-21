def grow_mapping(values: dict[int, int]) -> int:
    print("mapping start")
    values[3] = 30
    return 100


def grow_set(values: set[int]) -> int:
    print("set start")
    values.add(3)
    return 100


mapping = {1: 10, 2: 20}
print(sum(mapping, grow_mapping(mapping)))
mapping = {1: 10, 2: 20}
try:
    print(sum(map(int, mapping), grow_mapping(mapping)))
except RuntimeError:
    print("map captured dictionary iterator before start")
print(sum(mapping))

unique = {1, 2}
print(sum(unique, grow_set(unique)))
unique = {1, 2}
try:
    print(sum(map(int, unique), grow_set(unique)))
except RuntimeError:
    print("map captured set iterator before start")
print(sum(unique))
