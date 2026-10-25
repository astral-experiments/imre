from collections import Counter


def show(counts: Counter[str]) -> None:
    for key, count in counts.items():
        print(key)
        print(count)
    print("/")


a = Counter(z=-3, y=1, x=0)
b = Counter(x=5, y=-2, w=2, v=-4)
show(a + b)
show(a - b)
show(a | b)
show(a & b)
show(+a)
show(-b)

original = Counter(a)
alias = a
a += b
show(alias)
a -= a
show(alias)
a |= b
show(alias)
a &= original
show(alias)

# The same object can supply both operands. Values retained by the result
# must survive replacement of the only owner of a heap integer in the source.
large = 1 << 132
numbers = Counter({1: large, 2: -large})
numbers += numbers
print(numbers[1])
print(len(numbers))
numbers &= numbers
print(numbers[1])
numbers -= numbers
print(len(numbers))

# Pruning most of an insertion-ordered table can compact its backing storage.
many: Counter[int] = Counter()
for i in range(150):
    many[i] = 1 if i % 31 == 0 else -1
many |= Counter[int]()
for k in many:
    print(k)
