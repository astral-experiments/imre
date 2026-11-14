import gc
import weakref

events = []

class Decorator:
    def __init__(self, name):
        self.name = name
        events.append("create " + name)

    def __call__(self, cls):
        events.append("apply " + self.name)
        return cls

    def __del__(self):
        events.append("release " + self.name)

def last(fail):
    if fail:
        events.append("raise")
        raise LookupError("later decorator")
    return Decorator("last")

def make(fail):
    try:
        @Decorator("first")
        @(yield "ready")
        @last(fail)
        class Packet[T](dict):
            events.append("body")
        print("bound", Packet.__name__)
    except LookupError as error:
        print("caught", str(error), tuple(events))

for fail in (False, True):
    events.clear()
    g = make(fail)
    print(next(g), tuple(events))
    try:
        g.send(Decorator("middle"))
    except StopIteration:
        print("finished", tuple(events))


def check_consumed_method_decorator_is_released():
    def factory(decorator):
        class Owner:
            @decorator
            def owner(self):
                return __class__

        return Owner, weakref.ref(decorator)

    cls, receiver = factory(Decorator("method"))
    gc.collect()
    assert receiver() is None
    assert cls().owner() is cls


check_consumed_method_decorator_is_released()
