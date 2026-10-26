"""Compare dict subclass storage, inherited methods, and explicit base dispatch."""


def current():
    class Data(dict):
        pass

    empty = Data()
    print(isinstance(empty, dict), bool(empty), empty.get("visits", 0), len(empty))
    source = {"visits": 1}
    data = Data(source, greeting="hello")
    data.visits = "attribute"
    keys = data.keys()
    data["visits"] += 1
    data["more"] = "key"
    print(data.visits, dict.get(data, "visits"), list(keys), len(data))
    print(dict(data), {**data}, data == dict(data), source)
    print("visits" in data, "absent" in data, data.get("absent"))
    print(Data.get is dict.get, Data.__setitem__ is dict.__setitem__)

    class Tracked(Data):
        def __init__(self, values):
            self.writes = []
            dict.__init__(self, values)

        def __setitem__(self, key, value):
            self.writes.append(key)
            dict.__setitem__(self, key, value)

        def __missing__(self, key):
            return "missing " + key

        def __bool__(self):
            return False

    session = Tracked({"visits": 1})
    values = session.values()
    print(bool(session), bool(values), session.writes, session["absent"], session.get("absent"))
    session["visits"] = session.get("visits", 0) + 1
    print(session.writes, list(values), session["visits"])
    print(dict.__setitem__(session, "base", 9), dict.__init__(session, {"another": 10}))
    dict.update(session, [("key", 11)])
    print(session.writes, session)
    data.get = lambda key: "shadow"
    print(data.get("visits"), dict.get(data, "visits"))

    class Filtered(Data):
        def __iter__(self):
            raise AssertionError("dict copying uses keys")

        def keys(self):
            return ("visits",)

        def __getitem__(self, key):
            return dict.__getitem__(self, key) + 10

    filtered = Filtered(data)
    print(dict(filtered), {**filtered})

    class FailsLater(Filtered):
        def keys(self):
            return ("visits", "absent")

    received = {}
    try:
        received.update(FailsLater(data))
    except KeyError as error:
        print(received, error.args)

    def only_view():
        owner = Data(held=["live"])
        return owner.items()

    view = only_view()
    for _ in range(6000):
        []
    print(list(view), list(values), session.writes)

    try:
        empty[()]
    except KeyError as error:
        print(error.args)
    for method, args in ((dict.get, ()), (dict.get, (1, "x")), (dict.__setitem__, (1, "x", 2))):
        try:
            method(*args)
        except TypeError as error:
            print(error.args)
