import traceback

def factory(seed, rows, events):
    def mark(value):
        events.append(value)
        return value
    def change(value):
        nonlocal seed
        seed = value
    fn = lambda first=mark(seed), snap=[
        mark(row[0])
        for row in rows
    ], *, last=mark(seed + 1): (first, snap, last, seed)
    return fn, change

events = []
first, change = factory(5, [(20,), (30,)], events)
other, _ = factory(8, [(40,)], events)
print("bound", events)
change(99)
print("invoked", first(), first(1, [2], last=3), other())
print("unchanged events", events)
print("identity", first.__code__ is other.__code__, first.__closure__[0] is change.__closure__[0], first.__closure__[0] is other.__closure__[0])

failed = []
try:
    factory(10, [(7,), ()], failed)
except IndexError as exc:
    frame = traceback.extract_tb(exc.__traceback__)[-1]
    print("failure", failed, frame.name, (frame.lineno, frame.end_lineno, frame.colno, frame.end_colno))


def naïve():
    pass


for fn in (factory, naïve, lambda: None):
    print("module name", fn.__name__, fn.__name__ is fn.__qualname__)
