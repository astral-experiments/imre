"""Compare explicit base attribute writes and user-defined assignment hooks."""


def current():
    events = []

    class Field:
        def __set__(self, instance, value):
            events.append(("field", value))
            object.__setattr__(instance, "stored", 2 * value)

    class Proxy:
        field = Field()

        def __init__(self):
            object.__setattr__(self, "stored", 1)

        def __setattr__(self, name, value):
            events.append((name, value))
            if name == "blocked":
                raise LookupError(name)
            object.__setattr__(self, name, value)
            return 17

    obj = Proxy()
    print(obj.stored, events, object.__setattr__ is object.__setattr__)
    obj.field = 6
    print(obj.stored, events)
    print(object.__setattr__(obj, "field", 7), obj.stored, events)
    print(setattr(obj, "next", 8), obj.next, events)
    try:
        setattr(obj, "blocked", 9)
    except LookupError as e:
        print(e.args)
    print(setattr(Proxy, "constant", 12), obj.constant)

    class Plain:
        __setattr__ = object.__setattr__

    plain = Plain()
    plain.x = 5
    print(plain.x)
    for write, args in (
        (setattr, (obj, 3, 1)),
        (object.__setattr__, (Proxy, "x", 1)),
        (object.__setattr__, (3, "new", 1)),
        (object.__setattr__, ()),
        (setattr, (obj,)),
        (object.__setattr__, (obj, "y")),
    ):
        try:
            write(*args)
        except (TypeError, AttributeError) as e:
            print(e.args)
