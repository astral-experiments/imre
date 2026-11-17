events = []
def truth(self):
    events.append('truth')
    return True
Result = type('Result', (), {'__bool__': truth})
def left_eq(self, other):
    events.append('left')
    return NotImplemented
def right_eq(self, other):
    events.append('right')
    return Result()
Left = type('Left', (), {'__eq__': left_eq})
Right = type('Right', (), {'__eq__': right_eq})
def boom_eq(self, other):
    events.append('boom')
    raise RuntimeError('comparison failed')
Boom = type('Boom', (), {'__eq__': boom_eq})
def probe(callback, a, b):
    origin = callback.__module__
    if (a, True) == (b, 1):
        callback(origin, events)
    events.clear()
    try:
        if (Boom(), 1) == (Boom(), 1):
            callback('unreachable')
    except RuntimeError as error:
        callback(str(error), events)
getattr(probe, '__call__')(print, Left(), Right())
