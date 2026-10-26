from collections import deque


def receiver(values: deque[str]) -> deque[str]:
    print("receiver")
    return values


def key() -> int:
    print("key")
    return -1


def replacement() -> str:
    print("rhs")
    return "replacement"


def plain_index(index: int) -> int:
    return index


def mixed_readers(values: deque[int], flag: bool) -> None:
    read = values.__getitem__
    selected = read if flag else plain_index
    if flag:
        read = plain_index
    print(selected(1))
    print(read(1))


values = deque("abcd", maxlen=4)
values.popleft()
values.append("e")
receiver(values)[key()] = replacement()
print(values[False])
print(values[True])
print(values[-1])
get = values.__getitem__
put = values.__setitem__
erase = values.__delitem__
for position in [False, True]:
    print(get(position))
    put(position, get(position))
put(-2, "third")
print(get(-2))
original = values
values = deque("other")
erase(1)
print(get(1))
print(len(original))
print(len(values))

for index in [-4, 3, -1267650600228229401496703205376, 1267650600228229401496703205376]:
    try:
        print(original[index])
    except IndexError as error:
        print(str(error))
    try:
        original[index] = "wrong"
    except IndexError as error:
        print(str(error))
    try:
        del original[index]
    except IndexError as error:
        print(str(error))

numbers = deque(range(6), maxlen=6)
for entry in numbers:
    numbers[-1] += 1
    print(entry)
try:
    for entry in numbers:
        del numbers[-2]
except RuntimeError as error:
    print(str(error))
for value in numbers:
    print(value)
print(2 in numbers)
print(4 not in numbers)

nested = deque([[1], [2]])
kept = nested[0]
nested[0] = nested[-1]
del nested[-1]
print(kept[0])
print(nested[0][0])

for oversized in [-9223372036854775809, 9223372036854775808]:
    try:
        get(oversized)
    except OverflowError as error:
        print(str(error))
    try:
        original.__setitem__(oversized, replacement())
    except OverflowError as error:
        print(str(error))
    try:
        erase(oversized)
    except OverflowError as error:
        print(str(error))

flags = deque([6, 7, 8])
remove = flags.__delitem__
remove(True)
print(flags[True])
read = flags.__getitem__
print(list(map(read, [True, False])))
print(list(filter(read, [False, True])))
mixed_readers(flags, True)
mixed_readers(flags, False)
