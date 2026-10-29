import sys

sys.path.append("unused")


class Owner:
    calls: int

    def __init__(self):
        self.calls = 0

    def touch(self):
        self.calls += 1
        return 41


class Child(Owner):
    pass


class Counter:
    calls: int = 0

    @classmethod
    def touch(cls):
        cls.calls += 1
        return 41


owner = Owner()
child = Child()
values = [41, 41]
mapping = {"kept": 41}
targets = [owner.touch, Counter.touch, child.touch, values.pop, "retained".upper, mapping.copy]
shape = int(sys.argv[1])
target = targets[shape]
marker = "arm bound method argument refusal"
kind = "ok"
first = None
try:
    first = target()
except MemoryError:
    kind = "MemoryError"
print("kind", kind)
retry = target()
calls = 1 if kind == "MemoryError" else 2
preserved = True
valid = True
if shape == 0:
    preserved = owner.calls == calls
    valid = retry == 41 and (kind == "MemoryError" or first == 41)
if shape == 1:
    preserved = Counter.calls == calls
    valid = retry == 41 and (kind == "MemoryError" or first == 41)
if shape == 2:
    preserved = child.calls == calls
    valid = retry == 41 and (kind == "MemoryError" or first == 41)
if shape == 3:
    preserved = len(values) == 2 - calls
    valid = retry == 41 and (kind == "MemoryError" or first == 41)
if shape == 4:
    valid = retry == "RETAINED" and (kind == "MemoryError" or first == "RETAINED")
if shape == 5:
    preserved = mapping == {"kept": 41}
    valid = retry == mapping and (kind == "MemoryError" or first == mapping)
print("preserved", preserved)
print("retry", valid)
