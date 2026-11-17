import gc
events = []
def truth(self):
    events.append('truth')
    gc.collect()
    return True
Result = type('Result', (), {'__bool__': truth})
def left_eq(self, other):
    events.append('left')
    return NotImplemented
def right_eq(self, other):
    events.append('right')
    gc.collect()
    return Result()
Left = type('Left', (), {'__eq__': left_eq})
Right = type('Right', (), {'__eq__': right_eq})
def identity(value): return value
a = identity(Left())
b = identity(Right())
def boom_eq(self, other):
    events.append('boom')
    raise RuntimeError('comparison failed')
Boom = type('Boom', (), {'__eq__': boom_eq})
def tuple_eq(self, other):
    events.append('tuple-eq')
    return Result()
TupleSubclass = type('TupleSubclass', (tuple,), {'__eq__': tuple_eq})
def left_release(self): events.append('left-release')
def right_release(self): events.append('right-release')
EphemeralLeft = type('EphemeralLeft', (), {'__eq__': left_eq, '__del__': left_release})
EphemeralRight = type('EphemeralRight', (), {'__eq__': right_eq, '__del__': right_release})
def run_checks():
    buffer = bytearray()
    if (a, True) == (b, 1):
        print(True, events)
    else:
        print(False, events)
    events.clear()
    if (a, False) != (b, 1):
        print(True, events)
    else:
        print(False, events)
    events.clear()
    if (a,) == (b, 8):
        print(True, events)
    else:
        print(False, events)
    events.clear()
    if (a, True) == (a, 1):
        print(True, events)
    else:
        print(False, events)
    events.clear()
    if (False, a) == (1, b):
        print(True, events)
    else:
        print(False, events)
    events.clear()
    if a == b:
        print('direct', events)
    events.clear()
    try:
        if (Boom(), 1) == (Boom(), 1):
            print('unreachable')
    except RuntimeError as error:
        print(str(error), events)
    events.clear()
    value = TupleSubclass((1,))
    if value == (1,):
        print('subclass', events)
    events.clear()
    if (EphemeralLeft(), 1) == (EphemeralRight(), True):
        events.append('matched')
    print(events)
run_checks()
