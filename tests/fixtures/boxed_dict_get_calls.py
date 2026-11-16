"""Immediate mapping calls preserve receiver lifetime and custom attribute lookup."""


def current():
    events = []

    def argument(label, value):
        events.append(label)
        return value

    values = {"key": "nested", "nested": 12}
    print(values.get("key"), values.get("absent"), values.get("absent", 9))
    print(values.get(values.get("key"), values.get("absent", 13)))
    print(values.get(argument("key", "key"), argument("default", 8)), events)
    saved = values.get
    print(saved("key"), dict.get(values, "key"))

    owners = [{"key": "retained"}]

    def release_owner():
        owners.clear()
        return "key"

    print(owners[0].get(release_owner()), owners)

    class Override(dict):
        def get(self, *args):
            return ("override", args)

    class Lookup:
        @property
        def get(self):
            events.append("lookup")
            return lambda key: ("property", key)

    class Dynamic:
        def __getattr__(self, name):
            events.append(name)
            return lambda key: key

    events.clear()
    print(Override(key=1).get("key"), Lookup().get(argument("argument", 2)))
    print(Dynamic().get(argument("dynamic-argument", 3)), events)
    events.clear()
    try:
        object().get(argument("unexpected argument", 0))
    except AttributeError:
        print("missing", events)
    for thunk in [lambda: values.get(), lambda: values.get(1, 2, 3), lambda: values.get(key="key")]:
        try:
            thunk()
        except TypeError as exc:
            print(type(exc).__name__)
