from functools import lru_cache


def current():
    events = []

    class Key:
        def __init__(self, name, code):
            self.name = name
            self.code = code

        def __hash__(self):
            return self.code

        def __eq__(self, other):
            events.append((self.name, other.name))
            return self.name == other.name

    @lru_cache
    def cached(*args, **kwargs):
        events.append("call")
        return len(events)

    print(cached(Key("a", 1)), cached(Key("a", 1)))
    print(cached(Key("a", 2)))
    print(events)
    events.clear()
    cached(Key("x", 1), Key("y", 2))
    cached(Key("z", 3), Key("w", 4421810932546178461))
    print(events)
    events.clear()
    print(cached(Key("k", 4), named=Key("v", 5)))
    print(cached(Key("k", 4), named=Key("v", 5)))
    print(events)
