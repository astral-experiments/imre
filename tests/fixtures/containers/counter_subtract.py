from collections import Counter


def keyword(c: Counter[str]) -> int:
    print(c['a'])
    return c['a']


def invalid(label: int) -> int:
    print(label)
    return label


c = Counter(a=2)
c.subtract("aaa", other=keyword(c))
c.subtract({"missing": 10, "a": -5}, self=42, iterable=42)
for key, value in c.items():
    print(key)
    print(value)
c.subtract(c)
print(len(c))
print(c.total())
print("a" in c)
c.subtract(None, more=3)
Counter.subtract(c, {"a": 9})
Counter.update(c, more=2)
print(c)
d: Counter[int] = Counter()
d.subtract([1, 2, 1])
d.subtract({2: -11111111111111111111111111111111111111111})
print(d[1])
print(d[2])
try:
    c.subtract(invalid(42), last=invalid(7))
except TypeError as error:
    print(str(error))
try:
    c.subtract({}, {}, last=invalid(9))
except TypeError as error:
    print(str(error))
try:
    Counter.subtract()
except TypeError as error:
    print(str(error))
print(c.total())
